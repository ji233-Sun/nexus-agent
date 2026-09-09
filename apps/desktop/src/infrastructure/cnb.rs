use crate::{
    i18n::LocalizedText,
    model::cnb::{Cli, Issue, IssueFilter, IssuePage, PAGE_SIZE},
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
}

pub(crate) enum Response {
    Inspection {
        repository: Option<String>,
        cli: Result<Cli, LocalizedText>,
    },
    List(Result<IssuePage, LocalizedText>),
    Detail(Result<Issue, LocalizedText>),
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
                            ensure!(number.parse::<u64>().is_ok(), "CNB Issue 编号无效。");
                            let args = vec![
                                "issues".into(),
                                "get-issue".into(),
                                "--repo".into(),
                                repository,
                                "--number".into(),
                                number,
                                "--verbose".into(),
                            ];
                            let output = runtime
                                .as_ref()
                                .map_err(|error| anyhow::anyhow!(error.to_string()))?
                                .block_on(run(&cli.path, &args, CLI_TIMEOUT))?;
                            let (issue, _) = parse_response(&output)?;
                            Ok(issue)
                        })()
                        .map_err(localized_error);
                        Response::Detail(result)
                    }
                };
                let _ = sender.send(Event { id, response });
            })?;
        Ok(())
    }
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

fn parse_response<T: DeserializeOwned>(output: &str) -> Result<(T, Value)> {
    let response: Value =
        serde_json::from_str(output).context("CNB CLI 返回了无效 JSON，请检查 CLI 版本。")?;
    let status = response["status"]
        .as_u64()
        .context("CNB CLI 返回格式不受支持，请更新 CLI。")?;
    ensure!(
        (200..300).contains(&status),
        "HTTP {status} · {}",
        match status {
            401 | 403 => "请运行 cnb login 并确认拥有仓库 Issue 读取权限。",
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
    let total = response["total"]
        .as_u64()
        .or_else(|| response["header"]["x-cnb-total"].as_str()?.parse().ok())
        .context("CNB CLI 未返回分页总数，请更新 CLI。")?;
    Ok(IssuePage {
        issues,
        total: total.try_into()?,
    })
}

async fn run(path: &std::path::Path, args: &[String], timeout: Duration) -> Result<String> {
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
