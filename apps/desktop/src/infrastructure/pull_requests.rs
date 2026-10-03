use super::{
    cnb::page_total,
    issues::{repository_from_remote, run_cli},
};
use crate::{
    i18n::{Language, LocalizedText},
    model::{
        issues::{Cli, Comment, IssueFilter, IssueProvider, PAGE_SIZE, User},
        pull_requests::{
            Check, PullAction, PullDetail, PullPage, PullRequest, Review, ReviewThread,
        },
    },
};
use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;
use std::{collections::HashSet, time::Duration};

pub(crate) enum Request {
    List {
        filter: IssueFilter,
        page: usize,
        cursor: Option<String>,
    },
    Detail(String),
    Action {
        pull: Box<PullRequest>,
        action: PullAction,
    },
}

pub(crate) enum Response {
    List(Result<PullPage, LocalizedText>),
    Detail(Result<Box<PullDetail>, LocalizedText>),
    Action(Result<Box<PullRequest>, LocalizedText>),
}

pub(super) fn handle(
    provider: IssueProvider,
    cli: &Cli,
    repository: &str,
    request: Request,
) -> Response {
    let runtime = (|| -> Result<tokio::runtime::Runtime> {
        ensure!(
            repository_from_remote(provider, &provider.repository_url(repository)).as_deref()
                == Some(repository),
            "PR 仓库路径无效。"
        );
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("无法启动 PR 请求。")
    })();
    let runtime = match runtime {
        Ok(runtime) => runtime,
        Err(error) => {
            let error = localized_error(error);
            return match request {
                Request::List { .. } => Response::List(Err(error)),
                Request::Detail(_) => Response::Detail(Err(error)),
                Request::Action { .. } => Response::Action(Err(error)),
            };
        }
    };
    runtime.block_on(async {
        match request {
            Request::List {
                filter,
                page,
                cursor,
            } => Response::List(
                load_page(provider, cli, repository, filter, page, cursor.as_deref())
                    .await
                    .map_err(localized_error),
            ),
            Request::Detail(number) => Response::Detail(
                load_detail(provider, cli, repository, &number)
                    .await
                    .map(Box::new)
                    .map_err(localized_error),
            ),
            Request::Action { pull, action } => Response::Action(
                perform_action(provider, cli, repository, &pull, action)
                    .await
                    .map(Box::new)
                    .map_err(localized_error),
            ),
        }
    })
}

fn localized_error(error: anyhow::Error) -> LocalizedText {
    LocalizedText::new("PR 请求失败：{error}", &[("error", error.to_string())])
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).context("PR 数据格式不受支持，请更新 CLI 后重试。")
}

fn validate_number(number: &str) -> Result<()> {
    ensure!(
        number.parse::<u64>().is_ok_and(|number| number > 0),
        "PR 编号无效。"
    );
    Ok(())
}

async fn github_api(
    cli: &Cli,
    endpoint: &str,
    method: &str,
    fields: &[String],
    paginate: bool,
) -> Result<Value> {
    super::github::api(cli, endpoint, method, fields, paginate)
        .await
        .map_err(|error| anyhow::anyhow!(error.render(Language::Chinese).to_owned()))
}

const PULL_FIELDS: &str = "number title body state author { login } headRepository { nameWithOwner } headRefName baseRefName headRefOid baseRefOid isDraft mergeable mergeStateStatus";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitHubPull {
    number: u64,
    title: String,
    body: String,
    state: String,
    author: Option<GitHubUser>,
    head_repository: Option<GitHubRepository>,
    head_ref_name: String,
    base_ref_name: String,
    head_ref_oid: String,
    base_ref_oid: String,
    is_draft: bool,
    mergeable: String,
    merge_state_status: String,
}

#[derive(Deserialize)]
struct GitHubUser {
    login: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitHubRepository {
    name_with_owner: String,
}

fn github_pull(value: Value) -> Result<PullRequest> {
    let pull: GitHubPull = decode(value)?;
    let state = pull.state.to_ascii_lowercase();
    ensure!(
        pull.number > 0 && matches!(state.as_str(), "open" | "closed" | "merged"),
        "GitHub PR 数据无效。"
    );
    Ok(PullRequest {
        number: pull.number.to_string(),
        title: pull.title,
        body: pull.body,
        state,
        author: pull.author.map(|user| user.login).unwrap_or_default(),
        head_repository: pull
            .head_repository
            .map(|repo| repo.name_with_owner)
            .unwrap_or_default(),
        head_branch: pull.head_ref_name,
        base_branch: pull.base_ref_name,
        head_sha: pull.head_ref_oid,
        base_sha: pull.base_ref_oid,
        draft: pull.is_draft,
        mergeable: pull.mergeable == "MERGEABLE"
            && matches!(
                pull.merge_state_status.as_str(),
                "CLEAN" | "HAS_HOOKS" | "UNSTABLE"
            ),
        merge_status: format!("{} · {}", pull.mergeable, pull.merge_state_status),
    })
}

#[derive(Deserialize)]
struct CnbRef {
    #[serde(rename = "ref")]
    branch: String,
    sha: String,
    repo: CnbRepository,
}

#[derive(Deserialize)]
struct CnbRepository {
    path: String,
}

#[derive(Deserialize)]
struct CnbPull {
    number: String,
    title: String,
    #[serde(default)]
    body: String,
    state: String,
    #[serde(default)]
    author: User,
    head: CnbRef,
    base: CnbRef,
    #[serde(default)]
    is_wip: bool,
    mergeable_state: String,
    #[serde(default)]
    blocked_on: String,
}

fn cnb_pull(value: Value) -> Result<PullRequest> {
    let pull: CnbPull = decode(value)?;
    validate_number(&pull.number)?;
    ensure!(
        matches!(pull.state.as_str(), "open" | "closed" | "merged"),
        "CNB PR 状态无效。"
    );
    Ok(PullRequest {
        number: pull.number,
        title: pull.title,
        body: pull.body,
        state: pull.state,
        author: pull.author.name().to_owned(),
        head_repository: pull.head.repo.path,
        head_branch: pull
            .head
            .branch
            .trim_start_matches("refs/heads/")
            .to_owned(),
        base_branch: pull
            .base
            .branch
            .trim_start_matches("refs/heads/")
            .to_owned(),
        head_sha: pull.head.sha,
        base_sha: pull.base.sha,
        draft: pull.is_wip,
        mergeable: pull.mergeable_state == "mergeable" && pull.blocked_on.is_empty(),
        merge_status: if pull.blocked_on.is_empty() {
            pull.mergeable_state
        } else {
            format!("{} · {}", pull.mergeable_state, pull.blocked_on)
        },
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

fn next_cursor(value: &Value) -> Result<Option<String>> {
    let info: PageInfo = decode(value["pageInfo"].clone())?;
    if info.has_next_page {
        Ok(Some(
            info.end_cursor
                .filter(|cursor| !cursor.is_empty())
                .context("PR 分页信息不完整，请刷新后重试。")?,
        ))
    } else {
        Ok(None)
    }
}

fn connection_nodes(value: &Value) -> Result<Vec<Value>> {
    decode(value["nodes"].clone())
}

async fn github_query(
    cli: &Cli,
    repository: &str,
    query: String,
    extra: &[String],
) -> Result<Value> {
    let (owner, name) = repository
        .split_once('/')
        .context("GitHub 仓库路径无效。")?;
    let mut fields = vec![
        format!("query={query}"),
        format!("owner={owner}"),
        format!("name={name}"),
    ];
    fields.extend_from_slice(extra);
    github_api(cli, "graphql", "POST", &fields, false).await
}

async fn load_page(
    provider: IssueProvider,
    cli: &Cli,
    repository: &str,
    filter: IssueFilter,
    page: usize,
    cursor: Option<&str>,
) -> Result<PullPage> {
    ensure!(page > 0, "PR 页码无效。");
    match provider {
        IssueProvider::GitHub => {
            let states = if filter == IssueFilter::Open {
                "OPEN"
            } else {
                "CLOSED,MERGED"
            };
            let query = format!(
                "query($owner:String!, $name:String!, $cursor:String) {{ repository(owner:$owner,name:$name) {{ pullRequests(first:{PAGE_SIZE},after:$cursor,states:[{states}],orderBy:{{field:UPDATED_AT,direction:DESC}}) {{ totalCount pageInfo {{ hasNextPage endCursor }} nodes {{ {PULL_FIELDS} }} }} }} }}"
            );
            let fields = cursor
                .map(|cursor| vec![format!("cursor={cursor}")])
                .unwrap_or_default();
            let value = github_query(cli, repository, query, &fields).await?;
            let connection = &value["data"]["repository"]["pullRequests"];
            Ok(PullPage {
                pulls: connection_nodes(connection)?
                    .into_iter()
                    .map(github_pull)
                    .collect::<Result<_>>()?,
                total: decode(connection["totalCount"].clone())?,
                next_cursor: next_cursor(connection)?,
            })
        }
        IssueProvider::Cnb => {
            let (data, response) = cnb_request(
                cli,
                repository,
                "list-pulls",
                None,
                &[
                    "--page".into(),
                    page.to_string(),
                    "--page-size".into(),
                    PAGE_SIZE.to_string(),
                    "--state".into(),
                    filter.state().into(),
                    "--order-by=-updated_at".into(),
                ],
            )
            .await?;
            Ok(PullPage {
                pulls: decode::<Vec<Value>>(data)?
                    .into_iter()
                    .map(cnb_pull)
                    .collect::<Result<_>>()?,
                total: page_total(&response).context("CNB 未返回 PR 分页总数。")?,
                next_cursor: None,
            })
        }
    }
}

async fn load_pull(
    provider: IssueProvider,
    cli: &Cli,
    repository: &str,
    number: &str,
) -> Result<PullRequest> {
    validate_number(number)?;
    let pull = match provider {
        IssueProvider::GitHub => {
            // The number is validated before being inserted as an integer literal.
            let query = format!(
                "query($owner:String!, $name:String!) {{ repository(owner:$owner,name:$name) {{ pullRequest(number:{number}) {{ {PULL_FIELDS} }} }} }}"
            );
            let value = github_query(cli, repository, query, &[]).await?;
            github_pull(value["data"]["repository"]["pullRequest"].clone())?
        }
        IssueProvider::Cnb => cnb_pull(
            cnb_request(cli, repository, "get-pull", Some(number), &[])
                .await?
                .0,
        )?,
    };
    ensure!(pull.number == number, "平台返回了不同的 PR，请刷新后重试。");
    Ok(pull)
}

async fn load_detail(
    provider: IssueProvider,
    cli: &Cli,
    repository: &str,
    number: &str,
) -> Result<PullDetail> {
    let pull = load_pull(provider, cli, repository, number).await?;
    let (comments, reviews, threads, checks, stack) = match provider {
        IssueProvider::GitHub => {
            let comments = super::github::load_comments(cli, repository, number)
                .await
                .map_err(|error| anyhow::anyhow!(error.render(Language::Chinese).to_owned()))?;
            let reviews = github_reviews(cli, repository, number).await?;
            let threads = github_threads(cli, repository, number).await?;
            let checks = github_checks(cli, repository, &pull.head_sha).await?;
            let mut all = Vec::new();
            let mut cursor = None;
            loop {
                let page = load_page(
                    provider,
                    cli,
                    repository,
                    IssueFilter::Open,
                    1,
                    cursor.as_deref(),
                )
                .await?;
                all.extend(page.pulls);
                match page.next_cursor {
                    Some(next) => cursor = Some(next),
                    None => break,
                }
            }
            let stack = discover_stack(repository, &pull, all);
            (comments, reviews, threads, checks, stack)
        }
        IssueProvider::Cnb => {
            let comments = cnb_all(cli, repository, number, "list-pull-comments", &[])
                .await?
                .into_iter()
                .map(decode)
                .collect::<Result<Vec<Comment>>>()?;
            let values = cnb_all(cli, repository, number, "list-pull-reviews", &[]).await?;
            let mut reviews = Vec::new();
            let mut comments = (comments, Vec::new());
            for value in values {
                let review: CnbReview = decode(value)?;
                validate_number(&review.id)?;
                comments.1.extend(
                    cnb_all(
                        cli,
                        repository,
                        number,
                        "list-pull-review-comments",
                        &["--review-id".into(), review.id.clone()],
                    )
                    .await?,
                );
                reviews.push(Review {
                    id: review.id,
                    author: review.author.name().to_owned(),
                    state: review.state,
                    body: review.body,
                });
            }
            let threads = cnb_threads(comments.1)?;
            let data = cnb_request(
                cli,
                repository,
                "list-pull-commit-statuses",
                Some(number),
                &[],
            )
            .await?
            .0;
            let statuses: CnbStatuses = decode(data)?;
            ensure!(
                statuses.sha == pull.head_sha,
                "CNB 返回的 CI 不属于当前 PR Head，请刷新后重试。"
            );
            let checks = statuses
                .statuses
                .into_iter()
                .map(|status| Check {
                    name: status.context,
                    state: status.state,
                    description: status.description,
                    url: status.target_url,
                })
                .collect();
            (comments.0, reviews, threads, checks, Vec::new())
        }
    };
    Ok(PullDetail {
        pull,
        comments,
        reviews,
        threads,
        checks,
        stack,
    })
}

#[derive(Deserialize)]
struct GitHubReview {
    id: u64,
    user: Option<GitHubUser>,
    state: String,
    body: Option<String>,
}

async fn github_reviews(cli: &Cli, repository: &str, number: &str) -> Result<Vec<Review>> {
    let pages: Vec<Vec<GitHubReview>> = decode(
        github_api(
            cli,
            &format!("repos/{repository}/pulls/{number}/reviews?per_page=100"),
            "GET",
            &[],
            true,
        )
        .await?,
    )?;
    ensure!(!pages.is_empty(), "GitHub 审查分页不完整。");
    Ok(pages
        .into_iter()
        .flatten()
        .map(|review| Review {
            id: review.id.to_string(),
            author: review.user.map(|user| user.login).unwrap_or_default(),
            state: review.state,
            body: review.body.unwrap_or_default(),
        })
        .collect())
}

const COMMENT_FIELDS: &str = "id body author { login } createdAt";

fn thread_comments(connection: &Value) -> Result<Vec<Comment>> {
    connection_nodes(connection)?
        .into_iter()
        .map(|value| {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct ThreadComment {
                id: String,
                body: String,
                author: Option<GitHubUser>,
                created_at: String,
            }
            let comment: ThreadComment = decode(value)?;
            Ok(Comment {
                id: comment.id,
                body: comment.body,
                author: User {
                    username: comment.author.map(|user| user.login).unwrap_or_default(),
                    nickname: String::new(),
                },
                created_at: comment.created_at,
                statuses: None,
            })
        })
        .collect()
}

async fn github_threads(cli: &Cli, repository: &str, number: &str) -> Result<Vec<ReviewThread>> {
    let mut result = Vec::new();
    let mut cursor = None;
    loop {
        let query = format!(
            "query($owner:String!, $name:String!, $cursor:String) {{ repository(owner:$owner,name:$name) {{ pullRequest(number:{number}) {{ reviewThreads(first:100,after:$cursor) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ id path line isResolved isOutdated comments(first:100) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ {COMMENT_FIELDS} }} }} }} }} }} }} }}"
        );
        let fields = cursor
            .as_ref()
            .map(|cursor| vec![format!("cursor={cursor}")])
            .unwrap_or_default();
        let value = github_query(cli, repository, query, &fields).await?;
        let connection = &value["data"]["repository"]["pullRequest"]["reviewThreads"];
        for value in connection_nodes(connection)? {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Thread {
                id: String,
                path: String,
                line: Option<u64>,
                is_resolved: bool,
                is_outdated: bool,
                comments: Value,
            }
            let thread: Thread = decode(value)?;
            let mut comments = thread_comments(&thread.comments)?;
            let mut comment_cursor = next_cursor(&thread.comments)?;
            while let Some(cursor) = comment_cursor {
                let query = format!(
                    "query($id:ID!, $cursor:String!) {{ node(id:$id) {{ ... on PullRequestReviewThread {{ comments(first:100,after:$cursor) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ {COMMENT_FIELDS} }} }} }} }} }}"
                );
                let value = github_api(
                    cli,
                    "graphql",
                    "POST",
                    &[
                        format!("query={query}"),
                        format!("id={}", thread.id),
                        format!("cursor={cursor}"),
                    ],
                    false,
                )
                .await?;
                let connection = &value["data"]["node"]["comments"];
                comments.extend(thread_comments(connection)?);
                comment_cursor = next_cursor(connection)?;
            }
            result.push(ReviewThread {
                id: thread.id,
                path: thread.path,
                line: thread.line,
                resolved: thread.is_resolved,
                outdated: thread.is_outdated,
                comments,
            });
        }
        match next_cursor(connection)? {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    Ok(result)
}

#[derive(Deserialize)]
struct GitHubCheck {
    name: String,
    status: String,
    conclusion: Option<String>,
    html_url: Option<String>,
    output: CheckOutput,
}

#[derive(Deserialize)]
struct CheckOutput {
    title: Option<String>,
}

#[derive(Deserialize)]
struct GitHubStatus {
    context: String,
    state: String,
    description: Option<String>,
    target_url: Option<String>,
}

async fn github_checks(cli: &Cli, repository: &str, sha: &str) -> Result<Vec<Check>> {
    ensure!(
        sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "GitHub PR Head SHA 无效。"
    );
    let pages: Vec<Value> = decode(
        github_api(
            cli,
            &format!("repos/{repository}/commits/{sha}/check-runs?per_page=100&filter=latest"),
            "GET",
            &[],
            true,
        )
        .await?,
    )?;
    ensure!(!pages.is_empty(), "GitHub CI 分页不完整。");
    let mut checks = Vec::new();
    for page in pages {
        let runs: Vec<GitHubCheck> = decode(page["check_runs"].clone())?;
        checks.extend(runs.into_iter().map(|check| Check {
            name: check.name,
            state: check.conclusion.unwrap_or(check.status),
            description: check.output.title.unwrap_or_default(),
            url: check.html_url.unwrap_or_default(),
        }));
    }
    let pages: Vec<Vec<GitHubStatus>> = decode(
        github_api(
            cli,
            &format!("repos/{repository}/commits/{sha}/statuses?per_page=100"),
            "GET",
            &[],
            true,
        )
        .await?,
    )?;
    ensure!(!pages.is_empty(), "GitHub CI 分页不完整。");
    let mut seen = HashSet::new();
    // Commit statuses are newest first; a previous failed attempt must not mask a rerun.
    for status in pages.into_iter().flatten() {
        if seen.insert(status.context.clone()) {
            checks.push(Check {
                name: status.context,
                state: status.state,
                description: status.description.unwrap_or_default(),
                url: status.target_url.unwrap_or_default(),
            });
        }
    }
    Ok(checks)
}

async fn cnb_request(
    cli: &Cli,
    repository: &str,
    command: &str,
    number: Option<&str>,
    extra: &[String],
) -> Result<(Value, Value)> {
    let mut args = vec![
        "pulls".into(),
        command.into(),
        "--repo".into(),
        repository.into(),
        "--verbose".into(),
    ];
    if let Some(number) = number {
        validate_number(number)?;
        args.extend(["--number".into(), number.into()]);
    }
    args.extend_from_slice(extra);
    let output = run_cli(
        IssueProvider::Cnb,
        &cli.path,
        &args,
        Duration::from_secs(30),
        None,
    )
    .await?;
    super::cnb::parse_response(&output)
}

async fn cnb_all(
    cli: &Cli,
    repository: &str,
    number: &str,
    command: &str,
    extra: &[String],
) -> Result<Vec<Value>> {
    let mut items = Vec::new();
    let mut total = None;
    for page in 1.. {
        let mut fields = vec![
            "--page".into(),
            page.to_string(),
            "--page-size".into(),
            PAGE_SIZE.to_string(),
        ];
        fields.extend_from_slice(extra);
        let (data, response) = cnb_request(cli, repository, command, Some(number), &fields).await?;
        let batch: Vec<Value> = decode(data)?;
        let count = batch.len();
        items.extend(batch);
        total = page_total(&response).or(total);
        if let Some(total) = total {
            if items.len() >= total {
                break;
            }
            ensure!(count > 0, "CNB PR 讨论分页不完整，请刷新后重试。");
        } else if count < PAGE_SIZE {
            break;
        }
    }
    Ok(items)
}

#[derive(Deserialize)]
struct CnbReview {
    id: String,
    #[serde(default)]
    author: User,
    state: String,
    #[serde(default)]
    body: String,
}

#[derive(Deserialize)]
struct CnbStatuses {
    sha: String,
    statuses: Vec<CnbStatus>,
}

#[derive(Deserialize)]
struct CnbStatus {
    context: String,
    state: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    target_url: String,
}

fn cnb_threads(values: Vec<Value>) -> Result<Vec<ReviewThread>> {
    #[derive(Deserialize)]
    struct ReviewComment {
        #[serde(flatten)]
        comment: Comment,
        #[serde(default)]
        path: String,
        end_line: Option<u64>,
        reply_to_comment_id: Option<String>,
    }
    let comments = values
        .into_iter()
        .map(decode)
        .collect::<Result<Vec<ReviewComment>>>()?;
    let mut roots = std::collections::BTreeMap::new();
    for item in &comments {
        let mut root = item.comment.id.clone();
        let mut seen = HashSet::new();
        while seen.insert(root.clone()) {
            let Some(parent) = comments
                .iter()
                .find(|item| item.comment.id == root)
                .and_then(|item| item.reply_to_comment_id.as_deref())
                .filter(|parent| !parent.is_empty())
            else {
                break;
            };
            root = parent.to_owned();
        }
        roots.insert(item.comment.id.clone(), root);
    }
    let mut threads: Vec<ReviewThread> = Vec::new();
    for item in comments {
        let root = &roots[&item.comment.id];
        if let Some(thread) = threads.iter_mut().find(|thread| &thread.id == root) {
            if item.comment.id == *root {
                thread.path = item.path;
                thread.line = item.end_line;
            }
            thread.comments.push(item.comment);
        } else {
            threads.push(ReviewThread {
                id: root.clone(),
                path: item.path,
                line: item.end_line,
                resolved: false,
                outdated: false,
                comments: vec![item.comment],
            });
        }
    }
    Ok(threads)
}

pub(crate) fn discover_stack(
    repository: &str,
    selected: &PullRequest,
    mut pulls: Vec<PullRequest>,
) -> Vec<PullRequest> {
    pulls.retain(|pull| pull.number != selected.number);
    pulls.push(selected.clone());
    let parent_of = |parent: &PullRequest, child: &PullRequest| {
        parent.number != child.number
            && parent.head_repository.eq_ignore_ascii_case(repository)
            && !parent.head_branch.is_empty()
            && parent.head_branch == child.base_branch
    };
    let mut included = HashSet::from([selected.number.clone()]);
    loop {
        let before = included.len();
        for pull in &pulls {
            if pulls.iter().any(|other| {
                included.contains(&other.number)
                    && (parent_of(pull, other) || parent_of(other, pull))
            }) {
                included.insert(pull.number.clone());
            }
        }
        if included.len() == before {
            break;
        }
    }
    if included.len() < 2 {
        return Vec::new();
    }
    pulls.retain(|pull| included.contains(&pull.number));
    let mut ordered = Vec::new();
    while !pulls.is_empty() {
        // Preserve all members even for a malformed cyclic stack; never loop indefinitely.
        let index = pulls
            .iter()
            .position(|child| !pulls.iter().any(|parent| parent_of(parent, child)))
            .unwrap_or(0);
        ordered.push(pulls.remove(index));
    }
    ordered
}

async fn perform_action(
    provider: IssueProvider,
    cli: &Cli,
    repository: &str,
    expected: &PullRequest,
    action: PullAction,
) -> Result<PullRequest> {
    let current = load_pull(provider, cli, repository, &expected.number).await?;
    ensure!(current.state == "open", "PR 已关闭或合并，请刷新后重试。");
    ensure!(
        current.head_sha == expected.head_sha && current.base_sha == expected.base_sha,
        "PR 提交已变化，请刷新并重新确认操作。"
    );
    if matches!(action, PullAction::Merge(_)) {
        ensure!(
            current.can_merge(),
            "PR 暂不可合并，请检查草稿、冲突、审查与 CI 状态。"
        );
    }
    match (provider, action) {
        (IssueProvider::GitHub, PullAction::Merge(method)) => {
            let args = vec![
                "pr".into(),
                "merge".into(),
                expected.number.clone(),
                "--repo".into(),
                IssueProvider::GitHub.repository_url(repository),
                format!("--{}", method.key()),
                "--match-head-commit".into(),
                current.head_sha,
            ];
            run_cli(provider, &cli.path, &args, Duration::from_secs(30), None).await?;
        }
        (IssueProvider::GitHub, PullAction::Close) => {
            github_api(
                cli,
                &format!("repos/{repository}/pulls/{}", expected.number),
                "PATCH",
                &["state=closed".into()],
                false,
            )
            .await?;
        }
        (IssueProvider::Cnb, PullAction::Merge(method)) => {
            cnb_request(
                cli,
                repository,
                "merge-pull",
                Some(&expected.number),
                &["--merge-style".into(), method.key().into()],
            )
            .await?;
        }
        (IssueProvider::Cnb, PullAction::Close) => {
            cnb_request(
                cli,
                repository,
                "patch-pull",
                Some(&expected.number),
                &["--state".into(), "closed".into()],
            )
            .await?;
        }
    }
    let result = load_pull(provider, cli, repository, &expected.number).await?;
    let state = if matches!(action, PullAction::Merge(_)) {
        "merged"
    } else {
        "closed"
    };
    ensure!(
        result.state == state,
        "平台尚未完成 PR 操作（可能正在合并队列中），请刷新状态。"
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::pull_requests::MergeMethod;
    use serde_json::json;

    fn gh_pull(number: u64, head: &str, base: &str) -> Value {
        json!({"number":number,"title":format!("PR {number}"),"body":"PR body","state":"OPEN",
            "author":{"login":"author"},"headRepository":{"nameWithOwner":"team/project"},
            "headRefName":head,"baseRefName":base,"headRefOid":"b".repeat(40),"baseRefOid":"a".repeat(40),
            "isDraft":false,"mergeable":"MERGEABLE","mergeStateStatus":"CLEAN"})
    }

    fn cnb_value() -> Value {
        json!({"number":"2","title":"CNB PR","body":"body","state":"open",
            "author":{"username":"author"},"head":{"ref":"refs/heads/child","sha":"b".repeat(40),"repo":{"path":"team/project"}},
            "base":{"ref":"refs/heads/main","sha":"a".repeat(40),"repo":{"path":"team/project"}},
            "is_wip":false,"mergeable_state":"mergeable","blocked_on":""})
    }

    #[test]
    fn pull_requests_decode_native_states_and_gate_merge_for_drafts_and_conflicts() {
        let mut value = gh_pull(2, "child", "main");
        assert!(github_pull(value.clone()).unwrap().can_merge());
        value["mergeable"] = json!("CONFLICTING");
        assert!(!github_pull(value.clone()).unwrap().can_merge());
        value["mergeable"] = json!("MERGEABLE");
        value["mergeStateStatus"] = json!("BLOCKED");
        assert!(!github_pull(value.clone()).unwrap().can_merge());
        value["mergeStateStatus"] = json!("UNKNOWN");
        assert!(!github_pull(value).unwrap().can_merge());
        let mut value = cnb_value();
        let pull = cnb_pull(value.clone()).unwrap();
        assert_eq!(pull.head_branch, "child");
        assert!(pull.can_merge());
        value["blocked_on"] = json!("status_check");
        assert!(!cnb_pull(value.clone()).unwrap().can_merge());
        value["blocked_on"] = json!("");
        value["is_wip"] = json!(true);
        assert!(!cnb_pull(value.clone()).unwrap().can_merge());
        value["state"] = json!("merged");
        assert_eq!(cnb_pull(value.clone()).unwrap().state, "merged");
        value["number"] = json!("0");
        assert!(cnb_pull(value).is_err());
        assert!(github_pull(json!({"number":2})).is_err());
        assert!(next_cursor(&json!({"pageInfo":{"hasNextPage":true,"endCursor":null}})).is_err());
    }

    #[test]
    fn pull_requests_stack_orders_dependencies_and_excludes_unrelated_and_fork_branches() {
        let parent = github_pull(gh_pull(1, "parent", "main")).unwrap();
        let selected = github_pull(gh_pull(2, "child", "parent")).unwrap();
        let child = github_pull(gh_pull(3, "leaf", "child")).unwrap();
        let unrelated = github_pull(gh_pull(4, "other", "main")).unwrap();
        let mut fork = gh_pull(5, "child", "main");
        fork["headRepository"]["nameWithOwner"] = json!("someone/project");
        let fork = github_pull(fork).unwrap();
        let stack = discover_stack(
            "team/project",
            &selected,
            vec![child.clone(), unrelated, fork, selected.clone(), parent],
        );
        assert_eq!(
            stack
                .iter()
                .map(|pull| pull.number.as_str())
                .collect::<Vec<_>>(),
            vec!["1", "2", "3"]
        );
        assert!(discover_stack("team/project", &selected, vec![child.clone()]).len() == 2);
        let mut cycle = selected.clone();
        cycle.base_branch = child.head_branch.clone();
        assert_eq!(discover_stack("team/project", &cycle, vec![child]).len(), 2);
    }

    #[cfg(unix)]
    fn cli_fixture() -> (tempfile::TempDir, Cli) {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fake CLI 空格");
        std::fs::write(&path, r#"#!/bin/sh
printf '%s\n' "$@" >> "$0.calls"
directory="$(dirname "$0")"
if [ -f "$directory/fail" ]; then
    printf 'private-diagnostic\n' >&2
    exit 1
fi
if [ "$1" = "pulls" ]; then
    operation="$2"
    page=1
    for arg in "$@"; do
        if [ "$previous" = "--page" ]; then page="$arg"; fi
        if [ "$previous" = "--review-id" ]; then review="$arg"; fi
        previous="$arg"
    done
    case "$operation" in
        list-pull-review-comments) file="$operation-$review-$page" ;;
        list-pull-comments|list-pull-reviews) file="$operation-$page" ;;
        merge-pull|patch-pull) cp "$directory/cnb-terminal.json" "$directory/get-pull.json"; file="$operation" ;;
        *) file="$operation" ;;
    esac
elif [ "$1" = "pr" ]; then
    cp "$directory/gh-terminal.json" "$directory/pull.json"
    exit 0
else
    [ "$GH_PROMPT_DISABLED" = "1" ] || exit 2
    cursor=first
    for arg in "$@"; do
        case "$arg" in cursor=*) cursor="${arg#cursor=}" ;; esac
    done
    case "$*" in
        *PullRequestReviewThread*) file="replies-$cursor" ;;
        *reviewThreads*) file="threads-$cursor" ;;
        *pullRequests*) file="page-$cursor" ;;
        *graphql*) file=pull ;;
        *check-runs*) file=checks ;;
        *statuses*) file=statuses ;;
        *reviews*) file=reviews ;;
        *comments*) file=comments ;;
        *PATCH*) cp "$directory/gh-terminal.json" "$directory/pull.json"; file=close ;;
        *) exit 2 ;;
    esac
fi
cat "$directory/$file.json"
"#).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        (
            directory,
            Cli {
                path,
                version: "test".into(),
            },
        )
    }

    #[cfg(unix)]
    fn response(directory: &std::path::Path, name: &str, value: Value) {
        std::fs::write(directory.join(format!("{name}.json")), value.to_string()).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn github_pull_requests_load_all_threads_replies_checks_and_stack_pages() {
        let (directory, cli) = cli_fixture();
        let directory = directory.path();
        let selected = gh_pull(2, "child", "parent");
        response(
            directory,
            "pull",
            json!({"data":{"repository":{"pullRequest":selected.clone()}}}),
        );
        for (name, nodes, next) in [
            ("page-first", vec![selected], Some("next")),
            ("page-next", vec![gh_pull(1, "parent", "main")], None),
        ] {
            response(
                directory,
                name,
                json!({"data":{"repository":{"pullRequests":{"totalCount":2,"nodes":nodes,
                "pageInfo":{"hasNextPage":next.is_some(),"endCursor":next}}}}}),
            );
        }
        response(
            directory,
            "comments",
            json!([[{"id":1,"body":"discussion","user":{"login":"human"},"created_at":"now"}],
            [{"id":2,"body":"last comment","user":null,"created_at":"later"}]]),
        );
        response(
            directory,
            "reviews",
            json!([[{"id":9,"body":"other Agent finding","state":"CHANGES_REQUESTED","user":{"login":"other-agent"}}]]),
        );
        let comment =
            |id, body| json!({"id":id,"body":body,"author":{"login":"reviewer"},"createdAt":"now"});
        for (name, id, comments, reply_cursor, next) in [
            (
                "threads-first",
                "T1",
                vec![comment("C1", "root finding")],
                Some("reply-next"),
                Some("thread-next"),
            ),
            (
                "threads-thread-next",
                "T2",
                vec![comment("C3", "last thread")],
                None,
                None,
            ),
        ] {
            response(
                directory,
                name,
                json!({"data":{"repository":{"pullRequest":{"reviewThreads":{
                    "pageInfo":{"hasNextPage":next.is_some(),"endCursor":next},"nodes":[{
                        "id":id,"path":"src/main.rs","line":9,"isResolved":false,"isOutdated":false,
                        "comments":{"nodes":comments,"pageInfo":{"hasNextPage":reply_cursor.is_some(),"endCursor":reply_cursor}}
                    }]
                }}}}}),
            );
        }
        response(
            directory,
            "replies-reply-next",
            json!({"data":{"node":{"comments":{
                "nodes":[comment("C2","last reply")],"pageInfo":{"hasNextPage":false,"endCursor":null}
            }}}}),
        );
        response(
            directory,
            "checks",
            json!([{"check_runs":[{"name":"test","status":"completed","conclusion":"failure","html_url":"https://github.com/team/project/actions/runs/8","output":{"title":"failing test"}}]}, {"check_runs":[]} ]),
        );
        response(
            directory,
            "statuses",
            json!([[{"context":"lint","state":"success","description":null,"target_url":null}],
            [{"context":"lint","state":"failure","description":"old attempt","target_url":null}]]),
        );
        let detail = load_detail(IssueProvider::GitHub, &cli, "team/project", "2")
            .await
            .unwrap();
        assert_eq!(detail.comments.len(), 2);
        assert_eq!(detail.reviews[0].author, "other-agent");
        assert_eq!(detail.threads.len(), 2);
        assert_eq!(detail.threads[0].comments[1].body, "last reply");
        assert_eq!(detail.threads[1].comments[0].body, "last thread");
        assert!(detail.checks[0].failed());
        assert_eq!(detail.checks[1].state, "success");
        assert_eq!(detail.checks.len(), 2);
        assert_eq!(detail.stack[0].number, "1");
        let calls = std::fs::read_to_string(cli.path.with_extension("calls")).unwrap();
        for expected in [
            "--paginate",
            "--slurp",
            "cursor=thread-next",
            "cursor=reply-next",
            "id=T1",
            "cursor=next",
            &format!("commits/{}/check-runs", "b".repeat(40)),
        ] {
            assert!(calls.contains(expected), "{expected}");
        }
        response(
            directory,
            "replies-reply-next",
            json!({"errors":[{"message":"denied"}]}),
        );
        assert!(
            load_detail(IssueProvider::GitHub, &cli, "team/project", "2")
                .await
                .is_err()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cnb_pull_requests_paginate_reviews_and_preserve_nested_conversations() {
        let (directory, cli) = cli_fixture();
        let directory = directory.path();
        response(
            directory,
            "get-pull",
            json!({"status":200,"data":cnb_value()}),
        );
        response(
            directory,
            "list-pulls",
            json!({"status":200,"header":{"x-cnb-total":"31"},"data":[cnb_value()]}),
        );
        response(
            directory,
            "list-pull-comments-1",
            json!({"status":200,"total":0,"data":[]}),
        );
        response(
            directory,
            "list-pull-reviews-1",
            json!({"status":200,"total":2,"data":[{"id":"9","author":{"username":"other-agent"},"state":"changes_requested","body":"finding"}]}),
        );
        response(
            directory,
            "list-pull-reviews-2",
            json!({"status":200,"total":2,"data":[{"id":"10","state":"commented","body":"another review"}]}),
        );
        response(
            directory,
            "list-pull-review-comments-9-1",
            json!({"status":200,"total":3,"data":[
                {"id":"1","body":"root","path":"src/main.rs","end_line":5},
                {"id":"2","body":"reply","reply_to_comment_id":"1"}
            ]}),
        );
        response(
            directory,
            "list-pull-review-comments-9-2",
            json!({"status":200,"total":3,"data":[{"id":"3","body":"last nested reply","reply_to_comment_id":"2"}]}),
        );
        response(
            directory,
            "list-pull-review-comments-10-1",
            json!({"status":200,"total":0,"data":[]}),
        );
        response(
            directory,
            "list-pull-commit-statuses",
            json!({"status":200,"data":{"sha":"b".repeat(40),"state":"failure",
            "statuses":[{"context":"test","state":"failure","description":"failing test","target_url":"https://cnb.cool/team/project/-/build/logs/1"}]}}),
        );
        let page = load_page(
            IssueProvider::Cnb,
            &cli,
            "team/project",
            IssueFilter::Open,
            1,
            None,
        )
        .await
        .unwrap();
        assert_eq!(page.total, 31);
        let detail = load_detail(IssueProvider::Cnb, &cli, "team/project", "2")
            .await
            .unwrap();
        assert_eq!(detail.reviews.len(), 2);
        assert_eq!(detail.reviews[0].author, "other-agent");
        assert_eq!(detail.threads.len(), 1);
        assert_eq!(detail.threads[0].path, "src/main.rs");
        assert_eq!(
            detail.threads[0].comments.last().unwrap().body,
            "last nested reply"
        );
        assert!(detail.checks[0].failed());
        response(
            directory,
            "list-pull-review-comments-9-2",
            json!({"status":200,"total":3,"data":[]}),
        );
        assert!(
            load_detail(IssueProvider::Cnb, &cli, "team/project", "2")
                .await
                .is_err()
        );
        response(
            directory,
            "list-pull-review-comments-9-2",
            json!({"status":200,"total":3,"data":[{"id":"3","body":"reply","reply_to_comment_id":"2"}]}),
        );
        response(
            directory,
            "list-pull-commit-statuses",
            json!({"status":200,"data":{"sha":"stale","statuses":[]}}),
        );
        assert!(
            load_detail(IssueProvider::Cnb, &cli, "team/project", "2")
                .await
                .is_err()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pull_requests_actions_use_native_commands_pin_github_head_and_reject_stale_heads() {
        for provider in IssueProvider::ALL {
            for action in [PullAction::Close, PullAction::Merge(MergeMethod::Squash)] {
                let (directory, cli) = cli_fixture();
                let directory = directory.path();
                let terminal_state = if action == PullAction::Close {
                    "closed"
                } else {
                    "merged"
                };
                let expected = match provider {
                    IssueProvider::GitHub => {
                        let open = gh_pull(2, "child", "main");
                        let mut terminal = open.clone();
                        terminal["state"] = json!(terminal_state.to_ascii_uppercase());
                        response(
                            directory,
                            "pull",
                            json!({"data":{"repository":{"pullRequest":open.clone()}}}),
                        );
                        response(
                            directory,
                            "gh-terminal",
                            json!({"data":{"repository":{"pullRequest":terminal}}}),
                        );
                        response(directory, "close", json!({"state":"closed"}));
                        github_pull(open).unwrap()
                    }
                    IssueProvider::Cnb => {
                        let open = cnb_value();
                        let mut terminal = open.clone();
                        terminal["state"] = json!(terminal_state);
                        response(
                            directory,
                            "get-pull",
                            json!({"status":200,"data":open.clone()}),
                        );
                        response(
                            directory,
                            "cnb-terminal",
                            json!({"status":200,"data":terminal}),
                        );
                        response(directory, "merge-pull", json!({"status":200,"data":{}}));
                        response(directory, "patch-pull", json!({"status":200,"data":{}}));
                        cnb_pull(open).unwrap()
                    }
                };
                let mut stale = expected.clone();
                stale.head_sha = "stale".into();
                assert!(
                    perform_action(provider, &cli, "team/project", &stale, action)
                        .await
                        .is_err()
                );
                let result = perform_action(provider, &cli, "team/project", &expected, action)
                    .await
                    .unwrap();
                assert_eq!(result.state, terminal_state);
                let calls =
                    std::fs::read_to_string(format!("{}.calls", cli.path.display())).unwrap();
                if provider == IssueProvider::GitHub && matches!(action, PullAction::Merge(_)) {
                    assert!(calls.contains("--match-head-commit\n"));
                    assert!(calls.contains("--squash\n"));
                    assert!(calls.contains(&expected.head_sha));
                } else if provider == IssueProvider::Cnb && action == PullAction::Close {
                    assert!(calls.contains("patch-pull\n"));
                    assert!(calls.contains("--state\nclosed\n"));
                }
                std::fs::write(directory.join("fail"), "").unwrap();
                let error = perform_action(provider, &cli, "team/project", &expected, action)
                    .await
                    .unwrap_err();
                assert!(!error.to_string().contains("private-diagnostic"));
            }
        }
    }
}
