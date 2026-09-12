use crate::{
    i18n::LocalizedText,
    model::issues::{Cli, Comment, Issue, IssueAction, IssueFilter, IssuePage, IssueProvider},
};
use anyhow::{Context as _, Result, ensure};
use nexus_harness_core::executable_search_paths;
use std::{path::PathBuf, process::Stdio, sync::mpsc, time::Duration};
use tokio::{io::AsyncReadExt as _, process::Command};
use uuid::Uuid;

pub(crate) enum Request {
    Inspect(Option<PathBuf>),
    List {
        cli: Cli,
        repository: String,
        page: usize,
        filter: IssueFilter,
        cursor: Option<String>,
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
    pub(crate) provider: IssueProvider,
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

    pub(crate) fn request(
        &self,
        id: Uuid,
        provider: IssueProvider,
        request: Request,
    ) -> Result<()> {
        #[cfg(test)]
        if self.fake {
            return Ok(());
        }
        let sender = self.sender.clone();
        std::thread::Builder::new()
            .name(format!("nexus-{}", provider.key()))
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                let response = match provider {
                    IssueProvider::Cnb => super::cnb::handle(request, &runtime),
                    IssueProvider::GitHub => super::github::handle(request, &runtime),
                };
                let _ = sender.send(Event {
                    id,
                    provider,
                    response,
                });
            })?;
        Ok(())
    }
}

pub(crate) fn repository_from_remote(provider: IssueProvider, remote: &str) -> Option<String> {
    let url = if let Some((host, path)) = remote.split_once(':').filter(|_| !remote.contains("://"))
    {
        let host = host.rsplit('@').next()?;
        reqwest::Url::parse(&format!("ssh://{host}/{path}")).ok()?
    } else {
        reqwest::Url::parse(remote).ok()?
    };
    if !matches!(url.scheme(), "https" | "http" | "ssh")
        || url.host_str()? != provider.host()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let path = url.path().trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let parts = path.split('/').collect::<Vec<_>>();
    ((if provider == IssueProvider::Cnb {
        parts.len() >= 2
    } else {
        parts.len() == 2
    }) && parts.iter().all(|part| {
        !part.is_empty()
            && !matches!(*part, "." | ".." | "-")
            && part
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    }))
    .then(|| path.to_owned())
}

pub(super) fn repository_from_remotes(provider: IssueProvider, remotes: &str) -> Option<String> {
    let mut candidates = remotes
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let name = parts.next()?;
            let repository = repository_from_remote(provider, parts.next()?)?;
            (parts.next()? == "(fetch)").then_some((name, repository))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(name, _)| *name != "origin");
    candidates
        .into_iter()
        .next()
        .map(|(_, repository)| repository)
}

pub(super) async fn run_cli(
    provider: IssueProvider,
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
    if provider == IssueProvider::GitHub {
        command
            .env("GH_PROMPT_DISABLED", "1")
            .env_remove("GH_DEBUG");
    }
    if let Some(directory) = directory {
        // CNB CLI writes binary responses to os.tmpdir(). Isolate each download.
        for variable in ["TMPDIR", "TMP", "TEMP"] {
            command.env(variable, directory);
        }
    }
    nexus_runner::configure_child_process(&mut command);
    let mut child = command.spawn().context(match provider {
        IssueProvider::Cnb => "无法启动 CNB CLI，请检查安装。",
        IssueProvider::GitHub => "无法启动 GitHub CLI，请检查 gh 安装。",
    })?;
    let pid = child.id().context("无法获取 CLI 进程。")?;
    let mut stdout = child.stdout.take().context("无法读取 CLI 输出。")?;
    let mut stderr = child.stderr.take().context("无法读取 CLI 诊断。")?;
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
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                match provider {
                    IssueProvider::Cnb => "CNB CLI 请求超时，请检查网络后重试。",
                    IssueProvider::GitHub => "GitHub CLI 请求超时，请检查网络后重试。",
                },
            )
            .into());
        }
    }?;
    ensure!(
        result.0.success(),
        match provider {
            IssueProvider::Cnb => "CNB CLI 执行失败，请运行 cnb status 检查登录状态。",
            IssueProvider::GitHub =>
                "GitHub CLI 执行失败，请运行 gh auth status --hostname github.com 检查登录状态及仓库权限。",
        }
    );
    String::from_utf8(output).context("CLI 输出不是 UTF-8。")
}
