use super::issues::{ActionResult, Request, Response, repository_from_remotes, run_cli};
use crate::{
    i18n::LocalizedText,
    model::issues::{
        Cli, Comment, Issue, IssueAction, IssueFilter, IssuePage, IssueProvider, Label, PAGE_SIZE,
        User,
    },
};
use nexus_harness_core::resolve_executable;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;
use std::time::Duration;

type Result<T> = std::result::Result<T, LocalizedText>;
const CLI_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn handle(
    request: Request,
    runtime: &std::result::Result<tokio::runtime::Runtime, std::io::Error>,
) -> Response {
    // Repository detection remains available even when the CLI cannot start.
    if let Request::Inspect(path) = request {
        let repository = path.and_then(|path| {
            super::git::git(&path, &["remote", "-v"])
                .ok()
                .and_then(|remotes| repository_from_remotes(IssueProvider::GitHub, &remotes))
        });
        let cli = (|| {
            let path = resolve_executable("gh").ok_or_else(|| {
                LocalizedText::from(
                    "未检测到 GitHub CLI，请先安装 gh 并运行 gh auth login --hostname github.com。",
                )
            })?;
            let runtime = runtime
                .as_ref()
                .map_err(|_| LocalizedText::from("无法启动 GitHub 请求，请重试。"))?;
            let version = runtime.block_on(run(&path, &["--version".into()]))?;
            let version = version.lines().next().unwrap_or_default().trim().to_owned();
            if version.is_empty() {
                return Err("GitHub CLI 未返回版本，请检查安装。".into());
            }
            Ok(Cli { path, version })
        })();
        return Response::Inspection { repository, cli };
    }
    let runtime = match runtime {
        Ok(runtime) => runtime,
        Err(_) => {
            let error = LocalizedText::from("无法启动 GitHub 请求，请重试。");
            return match request {
                Request::List { .. } => Response::List(Err(error)),
                Request::Detail { .. } => Response::Detail(Err(error)),
                Request::Comments { .. } => Response::Comments(Err(error)),
                Request::Action { .. } => Response::Action(Err(error)),
                Request::NpcAction { .. } => Response::NpcAction(Err(error)),
                Request::Inspect(_) => unreachable!(),
            };
        }
    };
    runtime.block_on(async {
        match request {
            Request::List {
                cli,
                repository,
                filter,
                cursor,
                ..
            } => Response::List(load_issues(&cli, &repository, filter, cursor.as_deref()).await),
            Request::Detail {
                cli,
                repository,
                number,
            } => Response::Detail(load_issue(&cli, &repository, &number).await),
            Request::Comments {
                cli,
                repository,
                number,
            } => Response::Comments(load_comments(&cli, &repository, &number).await),
            Request::Action {
                cli,
                repository,
                number,
                action,
            } => Response::Action(perform_action(&cli, &repository, &number, action).await),
            Request::NpcAction { .. } => {
                Response::NpcAction(Err("GitHub Issue 请使用内置 AI 处理。".into()))
            }
            Request::Inspect(_) => unreachable!(),
        }
    })
}

async fn run(path: &std::path::Path, args: &[String]) -> Result<String> {
    run_cli(IssueProvider::GitHub, path, args, CLI_TIMEOUT, None)
        .await
        .map_err(|error| {
            if error.downcast_ref::<std::io::Error>().is_some_and(|error| error.kind() == std::io::ErrorKind::TimedOut) {
                "GitHub CLI 请求超时，请检查网络后重试。".into()
            } else {
                "GitHub CLI 执行失败，请运行 gh auth status --hostname github.com 检查登录状态及仓库权限。".into()
            }
        })
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value)
        .map_err(|_| "GitHub 返回了无效数据，请检查 gh 版本后重试。".into())
}

async fn api(
    cli: &Cli,
    endpoint: &str,
    method: &str,
    fields: &[String],
    paginate: bool,
) -> Result<Value> {
    let mut args = vec![
        "api".into(),
        "--hostname".into(),
        "github.com".into(),
        "--method".into(),
        method.into(),
        endpoint.into(),
    ];
    for field in fields {
        args.extend(["--raw-field".into(), field.clone()]);
    }
    if paginate {
        args.extend(["--paginate".into(), "--slurp".into()]);
    }
    let output = run(&cli.path, &args).await?;
    let value: Value = serde_json::from_str(&output)
        .map_err(|_| LocalizedText::from("GitHub 返回了无效数据，请检查 gh 版本后重试。"))?;
    if value.get("errors").is_some() || value.get("message").is_some() {
        return Err("GitHub 请求失败，请检查登录状态、仓库权限或 API 限额后重试。".into());
    }
    Ok(value)
}

fn repository_parts(repository: &str) -> Result<(&str, &str)> {
    if super::issues::repository_from_remote(
        IssueProvider::GitHub,
        &format!("https://github.com/{repository}"),
    )
    .as_deref()
        != Some(repository)
    {
        return Err("GitHub 仓库路径无效。".into());
    }
    repository
        .split_once('/')
        .ok_or_else(|| "GitHub 仓库路径无效。".into())
}

fn issue_endpoint(repository: &str, number: &str) -> Result<String> {
    repository_parts(repository)?;
    if !number.parse::<u64>().is_ok_and(|number| number > 0) {
        return Err("GitHub Issue 编号无效。".into());
    }
    Ok(format!("repos/{repository}/issues/{number}"))
}

#[derive(Deserialize)]
struct GitHubUser {
    login: String,
}

impl From<GitHubUser> for User {
    fn from(user: GitHubUser) -> Self {
        Self {
            username: user.login,
            nickname: String::new(),
        }
    }
}

#[derive(Deserialize)]
struct GitHubIssue {
    number: u64,
    title: String,
    state: String,
    body: Option<String>,
    user: Option<GitHubUser>,
    assignees: Vec<GitHubUser>,
    labels: Vec<Label>,
    comments: u64,
    created_at: String,
    updated_at: String,
}

fn issue(value: Value) -> Result<Issue> {
    if value.get("pull_request").is_some() {
        return Err("GitHub 返回的内容不是 Issue，请刷新后重试。".into());
    }
    let mut issue: GitHubIssue = decode(value)?;
    if issue.number == 0 || !matches!(issue.state.to_ascii_lowercase().as_str(), "open" | "closed")
    {
        return Err("GitHub 返回了无效数据，请检查 gh 版本后重试。".into());
    }
    for label in &mut issue.labels {
        if !label.color.starts_with('#') {
            label.color.insert(0, '#');
        }
    }
    Ok(Issue {
        number: issue.number.to_string(),
        title: issue.title,
        state: issue.state.to_ascii_lowercase(),
        body: issue.body.unwrap_or_default(),
        author: issue.user.map(User::from).unwrap_or_default(),
        assignees: issue.assignees.into_iter().map(User::from).collect(),
        labels: issue.labels,
        priority: String::new(),
        comment_count: issue.comments,
        created_at: issue.created_at,
        updated_at: issue.updated_at,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connection {
    total_count: usize,
    nodes: Vec<Value>,
    page_info: PageInfo,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

async fn load_issues(
    cli: &Cli,
    repository: &str,
    filter: IssueFilter,
    cursor: Option<&str>,
) -> Result<IssuePage> {
    let (owner, name) = repository_parts(repository)?;
    let query = format!(
        r#"query($owner:String!, $name:String!, $state:IssueState!, $cursor:String) {{
        repository(owner:$owner, name:$name) {{
            issues(first:{PAGE_SIZE}, after:$cursor, states:[$state], orderBy:{{field:UPDATED_AT,direction:DESC}}) {{
                totalCount pageInfo {{ hasNextPage endCursor }}
                nodes {{ number title state body user:author {{ login }}
                    assignees(first:100) {{ nodes {{ login }} }} labels(first:100) {{ nodes {{ name color }} }}
                    comments {{ totalCount }} created_at:createdAt updated_at:updatedAt }}
            }}
        }}
    }}"#
    );
    let mut fields = vec![
        format!("query={query}"),
        format!("owner={owner}"),
        format!("name={name}"),
        format!("state={}", filter.state().to_ascii_uppercase()),
    ];
    if let Some(cursor) = cursor {
        fields.push(format!("cursor={cursor}"));
    }
    let value = api(cli, "graphql", "POST", &fields, false).await?;
    parse_page(value)
}

fn parse_page(value: Value) -> Result<IssuePage> {
    let connection: Connection = decode(value["data"]["repository"]["issues"].clone())?;
    let next_cursor = if connection.page_info.has_next_page {
        Some(
            connection
                .page_info
                .end_cursor
                .filter(|cursor| !cursor.is_empty())
                .ok_or_else(|| LocalizedText::from("GitHub 分页数据不完整，请刷新后重试。"))?,
        )
    } else {
        None
    };
    let issues = connection
        .nodes
        .into_iter()
        .map(|mut node| {
            for (key, nested) in [
                ("assignees", "nodes"),
                ("labels", "nodes"),
                ("comments", "totalCount"),
            ] {
                let value = node
                    .get_mut(key)
                    .and_then(|value| value.get_mut(nested))
                    .map(Value::take)
                    .ok_or_else(|| {
                        LocalizedText::from("GitHub 返回了无效数据，请检查 gh 版本后重试。")
                    })?;
                node[key] = value;
            }
            issue(node)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(IssuePage {
        issues,
        total: connection.total_count,
        next_cursor,
    })
}

async fn load_issue(cli: &Cli, repository: &str, number: &str) -> Result<Issue> {
    issue(api(cli, &issue_endpoint(repository, number)?, "GET", &[], false).await?)
}

#[derive(Deserialize)]
struct GitHubComment {
    id: u64,
    body: String,
    user: Option<GitHubUser>,
    created_at: String,
}

async fn load_comments(cli: &Cli, repository: &str, number: &str) -> Result<Vec<Comment>> {
    let endpoint = format!(
        "{}/comments?per_page=100",
        issue_endpoint(repository, number)?
    );
    // gh follows every Link header; any failed page rejects the entire result.
    let pages: Vec<Vec<GitHubComment>> = decode(api(cli, &endpoint, "GET", &[], true).await?)?;
    if pages.is_empty() {
        return Err("GitHub 分页数据不完整，请刷新后重试。".into());
    }
    Ok(pages
        .into_iter()
        .flatten()
        .map(|comment| Comment {
            id: comment.id.to_string(),
            body: comment.body,
            author: comment.user.map(User::from).unwrap_or_default(),
            created_at: comment.created_at,
            statuses: None,
        })
        .collect())
}

async fn perform_action(
    cli: &Cli,
    repository: &str,
    number: &str,
    action: IssueAction,
) -> Result<ActionResult> {
    let mut endpoint = issue_endpoint(repository, number)?;
    let (method, fields) = match action {
        IssueAction::AssignSelf => {
            let user: GitHubUser = decode(api(cli, "user", "GET", &[], false).await?)?;
            if user.login.trim().is_empty() {
                return Err(
                    "GitHub 未返回当前用户名，请运行 gh auth login --hostname github.com。".into(),
                );
            }
            endpoint.push_str("/assignees");
            ("POST", vec![format!("assignees[]={}", user.login)])
        }
        IssueAction::SetState(state) => (
            "PATCH",
            vec![
                format!("state={}", state.state()),
                format!(
                    "state_reason={}",
                    if state == IssueFilter::Closed {
                        "completed"
                    } else {
                        "reopened"
                    }
                ),
            ],
        ),
        IssueAction::StartNpc => return Err("GitHub Issue 请使用内置 AI 处理。".into()),
    };
    issue(api(cli, &endpoint, method, &fields, false).await?)
        .map(|issue| ActionResult::Updated(Box::new(issue)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::i18n::Language;
    use serde_json::json;

    fn rest_issue() -> Value {
        json!({"number":42, "title":"GitHub 集成", "state":"open",
            "body":"## 描述\n\n```rust\nfn main() {}\n```", "user":{"login":"author"},
            "assignees":[{"login":"existing"},{"login":"me"}],
            "labels":[{"name":"bug", "color":"FF0000"}], "comments":101,
            "created_at":"2026-09-12T00:00:00Z", "updated_at":"2026-09-12T01:00:00Z"})
    }

    fn graphql_page() -> Value {
        let mut node = rest_issue();
        node["state"] = json!("OPEN");
        for key in ["assignees", "labels"] {
            node[key] = json!({"nodes":node[key].take()});
        }
        node["comments"] = json!({"totalCount":101});
        json!({"data":{"repository":{"issues":{
            "totalCount":1005, "nodes":[node],
            "pageInfo":{"hasNextPage":true,"endCursor":"NEXT"}
        }}}})
    }

    #[test]
    fn github_remotes_require_the_correct_host_and_prefer_origin_fetch() {
        use super::super::issues::repository_from_remote;
        for remote in [
            "git@github.com:team/repo.git",
            "https://github.com/team/repo/",
            "ssh://git@github.com/team/repo.git",
            "ssh://git@github.com:22/team/repo",
        ] {
            assert_eq!(
                repository_from_remote(IssueProvider::GitHub, remote).as_deref(),
                Some("team/repo")
            );
        }
        for remote in [
            "https://cnb.cool/team/repo",
            "https://github.com.evil.test/team/repo",
            "https://enterprise.test/team/repo",
            "https://github.com/team/repo/issues/1",
            "file:///team/repo",
            "https://github.com/team",
            "https://github.com/team/repo?token=secret",
        ] {
            assert!(
                repository_from_remote(IssueProvider::GitHub, remote).is_none(),
                "{remote}"
            );
        }
        assert_eq!(
            repository_from_remotes(
                IssueProvider::GitHub,
                "upstream https://github.com/team/upstream (fetch)\norigin git@github.com:team/repo.git (fetch)\norigin https://github.com/team/push (push)"
            ),
            Some("team/repo".into())
        );
        assert_eq!(
            repository_from_remotes(
                IssueProvider::GitHub,
                "origin https://cnb.cool/team/repo (fetch)\nupstream https://github.com/team/upstream (fetch)"
            ),
            Some("team/upstream".into())
        );
    }

    #[test]
    fn github_responses_preserve_metadata_and_reject_prs_and_invalid_connections() {
        let page = parse_page(graphql_page()).unwrap();
        assert_eq!(page.total, 1005);
        assert_eq!(page.next_cursor.as_deref(), Some("NEXT"));
        let detail = &page.issues[0];
        assert_eq!(detail.number, "42");
        assert_eq!(detail.state, "open");
        assert_eq!(detail.author.name(), "author");
        assert_eq!(detail.labels[0].color, "#FF0000");
        assert_eq!(detail.assignees.len(), 2);
        assert_eq!(detail.comment_count, 101);
        assert!(detail.body.contains("```rust"));
        let mut nullable = rest_issue();
        nullable["body"] = Value::Null;
        nullable["user"] = Value::Null;
        let empty = issue(nullable).unwrap();
        assert!(empty.body.is_empty());
        assert!(empty.author.name().is_empty());
        let mut pr = rest_issue();
        pr["pull_request"] = json!({"url":"https://api.github.com/repos/team/repo/pulls/42"});
        assert!(issue(pr).is_err());
        for invalid in [
            Value::Null,
            json!(true),
            json!({"assignees":false}),
            json!({"number":42}),
        ] {
            let mut page = graphql_page();
            page["data"]["repository"]["issues"]["nodes"] = json!([invalid]);
            assert!(parse_page(page).is_err());
        }
        let mut page = graphql_page();
        page["data"]["repository"]["issues"]["pageInfo"]["endCursor"] = Value::Null;
        assert!(parse_page(page).is_err());
        for invalid in [
            json!({"errors":[{"message":"denied"}]}),
            json!({"data":{"repository":null}}),
        ] {
            assert!(parse_page(invalid).is_err());
        }
        let empty = parse_page(json!({"data":{"repository":{"issues":{
            "nodes":[],"totalCount":0,"pageInfo":{"hasNextPage":false,"endCursor":null}
        }}}}))
        .unwrap();
        assert!(empty.issues.is_empty());
        assert_eq!(empty.total, 0);
    }

    #[cfg(unix)]
    fn github_fixture() -> (tempfile::TempDir, Cli) {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fake gh 空格");
        std::fs::write(
            &path,
            r#"#!/bin/sh
printf '%s\n' "$@" >> "$0.calls"
printf '\n' >> "$0.calls"
directory="$(dirname "$0")"
[ "$GH_PROMPT_DISABLED" = "1" ] || exit 2
case "$6" in
    graphql) file=graphql ;;
    user) file=user ;;
    *comments*) file=comments ;;
    *) file=issue ;;
esac
cat "$directory/$file.json"
if [ -f "$directory/fail" ]; then
    printf 'HTTP 403 private-diagnostic\n' >&2
    exit 1
fi
"#,
        )
        .unwrap();
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
    async fn github_cli_uses_native_pagination_and_reads_all_comments_before_returning() {
        let (directory, cli) = github_fixture();
        let directory = directory.path();
        response(directory, "graphql", graphql_page());
        let first = load_issues(&cli, "team/repo", IssueFilter::Open, None)
            .await
            .unwrap();
        load_issues(
            &cli,
            "team/repo",
            IssueFilter::Closed,
            first.next_cursor.as_deref(),
        )
        .await
        .unwrap();
        let comment = |id| {
            json!({"id":id,"user":{"login":"reviewer"},
            "created_at":"2026-09-12T00:00:00Z", "body":format!("验收 {id}：**完整评论**\n\n```rust\nfn main() {{}}\n```")})
        };
        response(
            directory,
            "comments",
            json!([(1..=100).map(comment).collect::<Vec<_>>(), [comment(101)]]),
        );
        let comments = load_comments(&cli, "team/repo", "42").await.unwrap();
        assert_eq!(comments.len(), 101);
        assert_eq!(comments[100].id, "101");
        assert_eq!(comments[100].author.name(), "reviewer");
        assert!(comments[100].body.contains("```rust"));
        let calls = std::fs::read_to_string(cli.path.with_extension("calls")).unwrap();
        assert!(calls.contains("api\n--hostname\ngithub.com\n--method\nPOST\ngraphql\n"));
        assert!(calls.contains("issues(first:30, after:$cursor"));
        assert!(calls.contains("state=OPEN"));
        assert!(calls.contains("state=CLOSED"));
        assert!(calls.contains("cursor=NEXT"));
        assert!(
            calls.contains("repos/team/repo/issues/42/comments?per_page=100\n--paginate\n--slurp")
        );
        let mut partial = graphql_page();
        partial["errors"] = json!([{"message":"denied"}]);
        response(directory, "graphql", partial);
        assert!(
            load_issues(&cli, "team/repo", IssueFilter::Open, None)
                .await
                .is_err()
        );
        std::fs::write(directory.join("fail"), "").unwrap();
        let error = load_comments(&cli, "team/repo", "42").await.unwrap_err();
        assert!(error.render(Language::English).contains("gh auth status"));
        assert!(
            !error
                .render(Language::English)
                .contains("private-diagnostic")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn github_actions_append_the_current_user_and_use_explicit_issue_states() {
        let (directory, cli) = github_fixture();
        let directory = directory.path();
        response(directory, "user", json!({"login":"me"}));
        response(directory, "issue", rest_issue());
        let detail = load_issue(&cli, "team/repo", "42").await.unwrap();
        assert_eq!(detail.number, "42");
        let ActionResult::Updated(assigned) =
            perform_action(&cli, "team/repo", "42", IssueAction::AssignSelf)
                .await
                .unwrap()
        else {
            panic!("updated issue")
        };
        assert_eq!(assigned.assignees[0].username, "existing");
        assert_eq!(assigned.assignees[1].username, "me");
        for state in [IssueFilter::Closed, IssueFilter::Open] {
            perform_action(&cli, "team/repo", "42", IssueAction::SetState(state))
                .await
                .unwrap();
        }
        let calls = std::fs::read_to_string(cli.path.with_extension("calls")).unwrap();
        assert!(calls.contains("GET\nuser\n"));
        assert!(
            calls
                .contains("POST\nrepos/team/repo/issues/42/assignees\n--raw-field\nassignees[]=me")
        );
        assert!(calls.contains("PATCH\nrepos/team/repo/issues/42\n--raw-field\nstate=closed\n--raw-field\nstate_reason=completed"));
        assert!(calls.contains("state=open\n--raw-field\nstate_reason=reopened"));
        assert!(
            perform_action(&cli, "team/repo", "42", IssueAction::StartNpc)
                .await
                .is_err()
        );
        for number in ["0", "../42", "42?state=closed", "--help"] {
            assert!(load_issue(&cli, "team/repo", number).await.is_err());
        }
        assert_eq!(
            std::fs::read_to_string(cli.path.with_extension("calls")).unwrap(),
            calls
        );
        response(directory, "user", json!({"login":""}));
        assert!(
            perform_action(&cli, "team/repo", "42", IssueAction::AssignSelf)
                .await
                .is_err()
        );
        response(
            directory,
            "issue",
            json!({"message":"Not Found","status":"404"}),
        );
        assert!(load_issue(&cli, "team/repo", "42").await.is_err());
    }
}
