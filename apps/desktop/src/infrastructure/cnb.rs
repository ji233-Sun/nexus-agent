use crate::{
    i18n::LocalizedText,
    model::cnb::{Cli, Comment, Issue, IssueAction, IssueFilter, IssuePage, PAGE_SIZE, User},
};
use anyhow::{Context as _, Result, ensure};
use nexus_harness_core::{executable_search_paths, resolve_executable};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{path::PathBuf, process::Stdio, sync::mpsc, time::Duration};
use tokio::{io::AsyncReadExt as _, process::Command};
use uuid::Uuid;

pub(crate) const DOCUMENTATION: &str = "https://docs.cnb.cool/en/develops/cnb-cli.html";
const CLI_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) enum Request {
    Inspect(Option<PathBuf>),
    List {
        cli: Cli,
        repository: String,
        page: usize,
        filter: IssueFilter,
    },
    Detail {
        cli: Cli,
        repository: String,
        number: String,
    },
    Comments {
        cli: Cli,
        repository: String,
        number: String,
    },
    Action {
        cli: Cli,
        repository: String,
        number: String,
        action: IssueAction,
    },
    NpcAction {
        cli: Cli,
        repository: String,
        number: String,
        comment_id: String,
    },
}

pub(crate) enum ActionResult {
    Updated(Box<Issue>),
    Npc(Comment),
}

pub(crate) enum Response {
    Inspection {
        repository: Option<String>,
        cli: Result<Cli, LocalizedText>,
    },
    List(Result<IssuePage, LocalizedText>),
    Detail(Result<Issue, LocalizedText>),
    Comments(Result<Vec<Comment>, LocalizedText>),
    Action(Result<ActionResult, LocalizedText>),
    NpcAction(Result<Comment, LocalizedText>),
}

pub(crate) struct Event {
    pub(crate) id: Uuid,
    pub(crate) response: Response,
}

pub(crate) struct Client {
    sender: mpsc::Sender<Event>,
    pub(crate) events: mpsc::Receiver<Event>,
    #[cfg(test)]
    fake: bool,
}

impl Default for Client {
    fn default() -> Self {
        let (sender, events) = mpsc::channel();
        Self {
            sender,
            events,
            #[cfg(test)]
            fake: false,
        }
    }
}

impl Client {
    #[cfg(test)]
    pub(crate) fn fake() -> Self {
        Self {
            fake: true,
            ..Self::default()
        }
    }

    pub(crate) fn request(&self, id: Uuid, request: Request) -> Result<()> {
        #[cfg(test)]
        if self.fake {
            return Ok(());
        }
        let sender = self.sender.clone();
        std::thread::Builder::new()
            .name("nexus-cnb".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                let response = match request {
                    Request::Inspect(path) => {
                        let repository = path.and_then(|path| {
                            super::git::git(&path, &["remote", "-v"])
                                .ok()
                                .and_then(|remotes| repository_from_remotes(&remotes))
                        });
                        let cli = (|| {
                            let path = resolve_executable("cnb")
                                .context("未检测到 CNB CLI，请先安装并运行 cnb login。")?;
                            let version = runtime
                                .as_ref()
                                .map_err(|error| anyhow::anyhow!(error.to_string()))?
                                .block_on(run(&path, &["--version".into()], CLI_TIMEOUT))?;
                            ensure!(
                                !version.trim().is_empty(),
                                "CNB CLI 未返回版本，请检查安装。"
                            );
                            Ok(Cli {
                                path,
                                version: version.trim().to_owned(),
                            })
                        })()
                        .map_err(localized_error);
                        Response::Inspection { repository, cli }
                    }
                    Request::List {
                        cli,
                        repository,
                        page,
                        filter,
                    } => {
                        let result = (|| {
                            let args = list_args(&repository, page, filter);
                            let output = runtime
                                .as_ref()
                                .map_err(|error| anyhow::anyhow!(error.to_string()))?
                                .block_on(run(&cli.path, &args, CLI_TIMEOUT))?;
                            parse_page(&output)
                        })()
                        .map_err(localized_error);
                        Response::List(result)
                    }
                    Request::Detail {
                        cli,
                        repository,
                        number,
                    } => {
                        let result = (|| {
                            runtime
                                .as_ref()
                                .map_err(|error| anyhow::anyhow!(error.to_string()))?
                                .block_on(issue_request(
                                    &cli,
                                    &repository,
                                    &number,
                                    "get-issue",
                                    &[],
                                ))
                        })()
                        .map_err(localized_error);
                        Response::Detail(result)
                    }
                    Request::Comments {
                        cli,
                        repository,
                        number,
                    } => {
                        let result = (|| {
                            runtime
                                .as_ref()
                                .map_err(|error| anyhow::anyhow!(error.to_string()))?
                                .block_on(load_comments(&cli, &repository, &number))
                        })()
                        .map_err(localized_error);
                        Response::Comments(result)
                    }
                    Request::Action {
                        cli,
                        repository,
                        number,
                        action,
                    } => {
                        let result = (|| {
                            runtime
                                .as_ref()
                                .map_err(|error| anyhow::anyhow!(error.to_string()))?
                                .block_on(perform_action(&cli, &repository, &number, action))
                        })()
                        .map_err(localized_error);
                        Response::Action(result)
                    }
                    Request::NpcAction {
                        cli,
                        repository,
                        number,
                        comment_id,
                    } => {
                        let result = (|| {
                            runtime
                                .as_ref()
                                .map_err(|error| anyhow::anyhow!(error.to_string()))?
                                .block_on(load_npc_action(&cli, &repository, &number, &comment_id))
                        })()
                        .map_err(localized_error);
                        Response::NpcAction(result)
                    }
                };
                let _ = sender.send(Event { id, response });
            })?;
        Ok(())
    }
}

fn issue_args(command: &str, repository: &str, number: &str) -> Result<Vec<String>> {
    ensure!(
        number.parse::<u64>().is_ok_and(|number| number > 0),
        "CNB Issue 编号无效。"
    );
    Ok(vec![
        "issues".into(),
        command.into(),
        "--repo".into(),
        repository.into(),
        "--number".into(),
        number.into(),
        "--verbose".into(),
    ])
}

async fn issue_request<T: DeserializeOwned>(
    cli: &Cli,
    repository: &str,
    number: &str,
    command: &str,
    extra: &[String],
) -> Result<T> {
    let mut args = issue_args(command, repository, number)?;
    args.extend_from_slice(extra);
    let output = run(&cli.path, &args, CLI_TIMEOUT).await?;
    Ok(parse_response(&output)?.0)
}

async fn load_comments(cli: &Cli, repository: &str, number: &str) -> Result<Vec<Comment>> {
    let mut comments = Vec::new();
    let mut expected_total = None;
    for page in 1.. {
        let mut args = issue_args("list-issue-comments", repository, number)?;
        args.extend([
            "--page".into(),
            page.to_string(),
            "--page-size".into(),
            PAGE_SIZE.to_string(),
            "--sort".into(),
            "created".into(),
        ]);
        let output = run(&cli.path, &args, CLI_TIMEOUT).await?;
        let (batch, response): (Vec<Comment>, _) = parse_response(&output)?;
        let count = batch.len();
        comments.extend(batch);
        expected_total = page_total(&response).or(expected_total);
        if let Some(total) = expected_total {
            if comments.len() >= total {
                break;
            }
            ensure!(count > 0, "CNB 评论分页不完整，请刷新评论后重试。");
        } else if count < PAGE_SIZE {
            break;
        }
    }
    Ok(comments)
}

async fn perform_action(
    cli: &Cli,
    repository: &str,
    number: &str,
    action: IssueAction,
) -> Result<ActionResult> {
    let (command, data) = match action {
        IssueAction::AssignSelf => {
            let output = run(
                &cli.path,
                &["users".into(), "get-user-info".into(), "--verbose".into()],
                CLI_TIMEOUT,
            )
            .await?;
            let (user, _): (User, _) = parse_response(&output)?;
            ensure!(
                !user.username.trim().is_empty(),
                "CNB 未返回当前用户名，请运行 cnb login。"
            );
            (
                "post-issue-assignees",
                serde_json::json!({"assignees": [user.username]}),
            )
        }
        IssueAction::SetState(state) => (
            "update-issue",
            serde_json::json!({
                "state": state.state(),
                "state_reason": if state == IssueFilter::Closed { "completed" } else { "reopened" },
            }),
        ),
        IssueAction::StartNpc => (
            "post-issue-comment",
            serde_json::json!({
                "body": "@CodeBuddy 请处理这个 Issue，阅读完整描述和全部评论，完成实现与验证，创建 PR 并在此回复结果及 PR 链接。",
                "work_mode": true,
            }),
        ),
    };
    let extra = ["--data".into(), data.to_string()];
    if action == IssueAction::StartNpc {
        issue_request(cli, repository, number, command, &extra)
            .await
            .map(ActionResult::Npc)
    } else {
        issue_request(cli, repository, number, command, &extra)
            .await
            .map(ActionResult::Updated)
    }
}

async fn load_npc_action(
    cli: &Cli,
    repository: &str,
    number: &str,
    comment_id: &str,
) -> Result<Comment> {
    ensure!(
        comment_id.parse::<u64>().is_ok_and(|id| id > 0),
        "CNB 评论编号无效。"
    );
    // Follow the exact triggering comment. Retrying this read never creates another NPC run.
    for attempt in 0..5 {
        let comment: Comment = issue_request(
            cli,
            repository,
            number,
            "get-issue-comment",
            &["--comment-id".into(), comment_id.into()],
        )
        .await?;
        ensure!(
            comment.id == comment_id,
            "CNB 返回了不同的评论，请刷新状态。"
        );
        if comment.action_url().is_some() || comment.npc_failure().is_some() || attempt == 4 {
            return Ok(comment);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    unreachable!()
}

fn localized_error(error: anyhow::Error) -> LocalizedText {
    LocalizedText::new("CNB 请求失败：{error}", &[("error", error.to_string())])
}

pub(crate) fn repository_from_remote(remote: &str) -> Option<String> {
    let url = if let Some((host, path)) = remote.split_once(':').filter(|_| !remote.contains("://"))
    {
        let host = host.rsplit('@').next()?;
        reqwest::Url::parse(&format!("ssh://{host}/{path}")).ok()?
    } else {
        reqwest::Url::parse(remote).ok()?
    };
    if !matches!(url.scheme(), "https" | "http" | "ssh")
        || url.host_str()? != "cnb.cool"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let path = url.path().trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let parts = path.split('/').collect::<Vec<_>>();
    (parts.len() >= 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && !matches!(*part, "." | ".." | "-")
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        }))
    .then(|| path.to_owned())
}

fn repository_from_remotes(remotes: &str) -> Option<String> {
    let mut candidates = remotes
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let name = parts.next()?;
            let repository = repository_from_remote(parts.next()?)?;
            (parts.next()? == "(fetch)").then_some((name, repository))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(name, _)| *name != "origin");
    candidates
        .into_iter()
        .next()
        .map(|(_, repository)| repository)
}

fn list_args(repository: &str, page: usize, filter: IssueFilter) -> Vec<String> {
    vec![
        "issues".into(),
        "list-issues".into(),
        "--repo".into(),
        repository.into(),
        "--page".into(),
        page.to_string(),
        "--page-size".into(),
        PAGE_SIZE.to_string(),
        "--order-by=-updated_at".into(),
        "--verbose".into(),
        "--state".into(),
        filter.state().into(),
    ]
}

pub(super) fn parse_response<T: DeserializeOwned>(output: &str) -> Result<(T, Value)> {
    let response: Value =
        serde_json::from_str(output).context("CNB CLI 返回了无效 JSON，请检查 CLI 版本。")?;
    let status = response["status"]
        .as_u64()
        .context("CNB CLI 返回格式不受支持，请更新 CLI。")?;
    ensure!(
        (200..300).contains(&status),
        "HTTP {status} · {}",
        match status {
            401 | 403 => "请运行 cnb login 并确认拥有此操作所需的仓库权限。",
            404 => "仓库或 Issue 不存在，或当前账号没有访问权限。",
            _ => "CNB 服务暂不可用，请稍后重试。",
        }
    );
    let data =
        serde_json::from_value(response["data"].clone()).context("CNB Issue 数据格式不受支持。")?;
    Ok((data, response))
}

fn parse_page(output: &str) -> Result<IssuePage> {
    let (issues, response) = parse_response(output)?;
    let total = page_total(&response).context("CNB CLI 未返回分页总数，请更新 CLI。")?;
    Ok(IssuePage { issues, total })
}

fn page_total(response: &Value) -> Option<usize> {
    response["total"]
        .as_u64()
        .or_else(|| response["header"]["x-cnb-total"].as_str()?.parse().ok())?
        .try_into()
        .ok()
}

async fn run(path: &std::path::Path, args: &[String], timeout: Duration) -> Result<String> {
    run_with_temp_dir(path, args, timeout, None).await
}

pub(super) async fn run_with_temp_dir(
    path: &std::path::Path,
    args: &[String],
    timeout: Duration,
    directory: Option<&std::path::Path>,
) -> Result<String> {
    let mut command = Command::new(path);
    command
        .args(args)
        .env("NO_COLOR", "1")
        .env_remove("FORCE_COLOR")
        .env("PATH", std::env::join_paths(executable_search_paths())?)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(directory) = directory {
        // CNB CLI writes binary responses to os.tmpdir(). Isolate each download.
        for variable in ["TMPDIR", "TMP", "TEMP"] {
            command.env(variable, directory);
        }
    }
    nexus_runner::configure_child_process(&mut command);
    let mut child = command.spawn().context("无法启动 CNB CLI，请检查安装。")?;
    let pid = child.id().context("无法获取 CNB CLI 进程。")?;
    let mut stdout = child.stdout.take().context("无法读取 CNB CLI 输出。")?;
    let mut stderr = child.stderr.take().context("无法读取 CNB CLI 诊断。")?;
    let mut output = Vec::new();
    let mut diagnostic = Vec::new();
    let result = tokio::time::timeout(timeout, async {
        tokio::try_join!(
            child.wait(),
            stdout.read_to_end(&mut output),
            stderr.read_to_end(&mut diagnostic)
        )
    })
    .await;
    let result = match result {
        Ok(result) => result,
        Err(_) => {
            let _ = nexus_runner::terminate_child_process(&mut child, pid).await;
            anyhow::bail!("CNB CLI 请求超时，请检查网络后重试。");
        }
    }?;
    ensure!(
        result.0.success(),
        "CNB CLI 执行失败，请运行 cnb status 检查登录状态。"
    );
    String::from_utf8(output).context("CNB CLI 输出不是 UTF-8。")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn cnb_fixture() -> (tempfile::TempDir, Cli) {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fake cnb");
        std::fs::write(
            &path,
            r#"#!/bin/sh
printf '%s\n' "$@" >> "$0.calls"
printf '\n' >> "$0.calls"
directory="$(dirname "$0")"
operation="$2"
shift 2
page=1
while [ "$#" -gt 0 ]; do
    if [ "$1" = "--page" ]; then shift; page="$1"; fi
    shift
done
if [ "$operation" = "list-issue-comments" ]; then
    cat "$directory/comments-$page.json"
elif [ "$operation" = "get-issue-comment" ] && [ ! -f "$directory/polled" ]; then
    touch "$directory/polled"
    cat "$directory/post-issue-comment.json"
else
    cat "$directory/$operation.json"
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
    fn cnb_response(directory: &std::path::Path, file: &str, response: Value) {
        std::fs::write(directory.join(format!("{file}.json")), response.to_string()).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cnb_issue_actions_use_current_identity_state_reasons_and_native_npc_comment() {
        let (directory, cli) = cnb_fixture();
        let directory = directory.path();
        let issue = serde_json::json!({"number":"144", "title":"Issue", "state":"open",
            "assignees":[{"username":"existing"}, {"username":"signed-in-user"}]});
        for (file, data) in [
            (
                "get-user-info",
                serde_json::json!({"username":"signed-in-user"}),
            ),
            ("post-issue-assignees", issue.clone()),
            ("update-issue", issue),
            (
                "post-issue-comment",
                serde_json::json!({"id":"987", "body":"@CodeBuddy", "statuses":null}),
            ),
            (
                "get-issue-comment",
                serde_json::json!({"id":"987", "statuses":{"npc":[{"statuses":[
                    {"target_url":"https://cnb.cool/team/repo/-/build/logs/cnb-123"}
                ]}]}}),
            ),
        ] {
            cnb_response(
                directory,
                file,
                serde_json::json!({"status":201,"data":data}),
            );
        }
        let assigned = perform_action(&cli, "team/repo", "144", IssueAction::AssignSelf)
            .await
            .unwrap();
        let ActionResult::Updated(assigned) = assigned else {
            panic!("updated issue")
        };
        assert_eq!(assigned.assignees.len(), 2);
        for state in [IssueFilter::Closed, IssueFilter::Open] {
            perform_action(&cli, "team/repo", "144", IssueAction::SetState(state))
                .await
                .unwrap();
        }
        let ActionResult::Npc(comment) =
            perform_action(&cli, "team/repo", "144", IssueAction::StartNpc)
                .await
                .unwrap()
        else {
            panic!("NPC triggering comment")
        };
        assert_eq!(comment.id, "987");
        assert!(comment.action_url().is_none());
        let action = load_npc_action(&cli, "team/repo", "144", &comment.id)
            .await
            .unwrap();
        assert_eq!(
            action.action_url(),
            Some("https://cnb.cool/team/repo/-/build/logs/cnb-123")
        );
        let calls = std::fs::read_to_string(cli.path.with_extension("calls")).unwrap();
        let calls = calls
            .trim()
            .split("\n\n")
            .map(|call| call.lines().collect::<Vec<_>>())
            .collect::<Vec<_>>();
        assert_eq!(calls[0], ["users", "get-user-info", "--verbose"]);
        assert_eq!(
            calls[1][..7],
            [
                "issues",
                "post-issue-assignees",
                "--repo",
                "team/repo",
                "--number",
                "144",
                "--verbose"
            ]
        );
        let payload = |index: usize| {
            assert_eq!(calls[index][7], "--data");
            serde_json::from_str::<Value>(calls[index][8]).unwrap()
        };
        for call in &calls[1..] {
            assert_eq!(
                call[2..7],
                ["--repo", "team/repo", "--number", "144", "--verbose"]
            );
        }
        assert_eq!(calls[2][1], "update-issue");
        assert_eq!(calls[3][1], "update-issue");
        assert_eq!(
            payload(1),
            serde_json::json!({"assignees":["signed-in-user"]})
        );
        assert_eq!(
            payload(2),
            serde_json::json!({"state":"closed", "state_reason":"completed"})
        );
        assert_eq!(
            payload(3),
            serde_json::json!({"state":"open", "state_reason":"reopened"})
        );
        assert_eq!(calls[4][1], "post-issue-comment");
        assert_eq!(payload(4)["work_mode"], true);
        assert!(
            payload(4)["body"]
                .as_str()
                .unwrap()
                .starts_with("@CodeBuddy ")
        );
        assert_eq!(calls.len(), 7);
        for call in &calls[5..] {
            assert_eq!(
                call,
                &[
                    "issues",
                    "get-issue-comment",
                    "--repo",
                    "team/repo",
                    "--number",
                    "144",
                    "--verbose",
                    "--comment-id",
                    "987"
                ]
            );
        }
        cnb_response(
            directory,
            "get-issue-comment",
            serde_json::json!({"status":200,"data":{"id":"other"}}),
        );
        assert!(
            load_npc_action(&cli, "team/repo", "144", "987")
                .await
                .is_err()
        );
        cnb_response(
            directory,
            "update-issue",
            serde_json::json!({"status":403,"data":{}}),
        );
        assert!(
            perform_action(
                &cli,
                "team/repo",
                "144",
                IssueAction::SetState(IssueFilter::Closed)
            )
            .await
            .is_err()
        );
        cnb_response(
            directory,
            "get-user-info",
            serde_json::json!({"status":200,"data":{}}),
        );
        assert!(
            perform_action(&cli, "team/repo", "144", IssueAction::AssignSelf)
                .await
                .is_err()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cnb_comments_load_every_page_and_reject_partial_results() {
        let (directory, cli) = cnb_fixture();
        let directory = directory.path();
        let comments: Vec<_> = (1..=PAGE_SIZE)
            .map(|id| serde_json::json!({"id":id.to_string(), "body":"评论"}))
            .collect();
        for metadata in [false, true] {
            let mut first = serde_json::json!({"status":200,"data":comments});
            if metadata {
                first["header"] = serde_json::json!({"x-cnb-total":"31"});
            }
            cnb_response(directory, "comments-1", first);
            cnb_response(
                directory,
                "comments-2",
                serde_json::json!({"status":200,"data":[{
                    "id":"31", "body":"最终验收：**完整上下文**\n\n```rust\nfn main() {}\n```", "author":{"username":"reviewer"}
                }]}),
            );
            let result = load_comments(&cli, "team/repo", "144").await.unwrap();
            assert_eq!(result.len(), 31);
            assert_eq!(result[30].author.username, "reviewer");
            assert!(result[30].body.contains("```rust"));
            cnb_response(
                directory,
                "comments-2",
                serde_json::json!({"status":403,"data":{}}),
            );
            assert!(load_comments(&cli, "team/repo", "144").await.is_err());
        }
        cnb_response(
            directory,
            "comments-2",
            serde_json::json!({"status":200,"data":[]}),
        );
        assert!(load_comments(&cli, "team/repo", "144").await.is_err());
        cnb_response(
            directory,
            "comments-1",
            serde_json::json!({"status":200,"data":[],"total":0}),
        );
        assert!(
            load_comments(&cli, "team/repo", "144")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cnb_remotes_recognize_nested_organizations_and_prefer_origin_fetch() {
        for remote in [
            "https://cnb.cool/team/sub/repo.git",
            "git@cnb.cool:team/sub/repo.git",
            "ssh://git@cnb.cool/team/sub/repo",
            "https://cnb.cool/team/sub/repo/",
        ] {
            assert_eq!(
                repository_from_remote(remote).as_deref(),
                Some("team/sub/repo")
            );
        }
        for remote in [
            "https://github.com/team/repo.git",
            "https://cnb.cool.evil.test/team/repo",
            "file:///team/repo",
            "/tmp/team/repo",
            "https://cnb.cool/team",
            "https://cnb.cool/team/repo/-/issues/1",
        ] {
            assert!(repository_from_remote(remote).is_none(), "{remote}");
        }
        assert_eq!(
            repository_from_remotes(
                "upstream https://cnb.cool/team/other (fetch)\norigin git@cnb.cool:team/repo.git (fetch)\norigin https://cnb.cool/team/push (push)"
            ),
            Some("team/repo".into())
        );
        assert_eq!(
            repository_from_remotes(
                "origin https://github.com/team/repo (fetch)\nupstream https://cnb.cool/team/repo (fetch)"
            ),
            Some("team/repo".into())
        );
    }

    #[test]
    fn cnb_verbose_responses_preserve_pagination_metadata_and_markdown() {
        let issue = serde_json::json!({"number":"42", "title":"中文 Issue", "state":"closed",
            "body":"## 描述\n\n```rust\nfn main() {}\n```", "labels":[{"name":"bug", "color":"#FF0000"}],
            "author":{"username":"author", "nickname":"作者"}, "assignees":[{"username":"owner"}],
            "comment_count":3, "priority":"P1", "created_at":"2026-09-09T00:00:00Z"});
        let page = parse_page(
            &serde_json::json!({"status":200,"data":[issue.clone()],"total":61}).to_string(),
        )
        .unwrap();
        assert_eq!(page.total, 61);
        assert_eq!(page.issues[0].number, "42");
        assert_eq!(page.issues[0].author.name(), "作者");
        assert_eq!(page.issues[0].assignees[0].name(), "owner");
        let (detail, _): (Issue, _) =
            parse_response(&serde_json::json!({"status":200,"data":issue}).to_string()).unwrap();
        assert!(detail.body.contains("```rust"));
        assert_eq!(detail.labels[0].name, "bug");
        let empty = parse_page(r#"{"status":200,"data":[],"header":{"x-cnb-total":"0"}}"#).unwrap();
        assert_eq!(empty.total, 0);
        assert!(empty.issues.is_empty());
    }

    #[test]
    fn cnb_http_errors_are_not_treated_as_empty_successful_lists() {
        for status in [401, 403, 404, 429, 500] {
            let error = parse_page(
                &serde_json::json!({"status":status,"data":{"errmsg":"failure"}}).to_string(),
            )
            .unwrap_err();
            assert!(error.to_string().contains(&format!("HTTP {status}")));
        }
        for invalid in [
            "not JSON",
            "[]",
            r#"{"status":200,"data":{}}"#,
            r#"{"status":200,"data":[],"total":null}"#,
        ] {
            assert!(parse_page(invalid).is_err());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cnb_cli_uses_official_flags_and_terminates_timed_out_processes() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().unwrap();
        let cli = directory.path().join("fake cnb");
        std::fs::write(
            &cli,
            "#!/bin/sh\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\"; done\n",
        )
        .unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        for filter in [IssueFilter::Open, IssueFilter::Closed] {
            let args = list_args("team/sub/repo", 3, filter);
            let output = run(&cli, &args, Duration::from_secs(5)).await.unwrap();
            assert_eq!(
                output.lines().collect::<Vec<_>>(),
                [
                    "issues",
                    "list-issues",
                    "--repo",
                    "team/sub/repo",
                    "--page",
                    "3",
                    "--page-size",
                    "30",
                    "--order-by=-updated_at",
                    "--verbose",
                    "--state",
                    filter.state()
                ]
            );
        }
        std::fs::write(&cli, "#!/bin/sh\nexit 1\n").unwrap();
        assert!(
            run(&cli, &[], Duration::from_secs(5))
                .await
                .unwrap_err()
                .to_string()
                .contains("cnb status")
        );
        std::fs::write(&cli, "#!/bin/sh\nsleep 60\n").unwrap();
        let started = std::time::Instant::now();
        let error = run(&cli, &[], Duration::from_millis(50)).await.unwrap_err();
        assert!(error.to_string().contains("超时"));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(
            run(
                &directory.path().join("missing-cnb"),
                &[],
                Duration::from_secs(1)
            )
            .await
            .is_err()
        );
    }
}
