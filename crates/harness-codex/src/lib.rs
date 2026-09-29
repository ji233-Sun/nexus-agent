mod catalog;
mod decoder;
pub use catalog::discover_models;
pub use decoder::{EventDecoder, TitleEventDecoder};

use std::{
    collections::{HashMap, HashSet},
    env,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use nexus_domain::{
    HarnessKind, ModelDescriptor, ModelReasoningEffort, PermissionMode, ThinkingEffort,
    UserAskAnswer, UserAskAnswerMode, UserAskAnswerValue, UserAskOption, UserAskQuestion,
    UserAskStatus,
};
use nexus_harness_core::{
    ApprovalOption, ApprovalPrompt, InputFrame, LineDecoder, UserAskRequest, hide_console_window,
    resolve_executable, summarize_text, tool_content,
};
pub use nexus_harness_core::{DecodedEvent, LaunchSpec, ModelCatalogError};
use nexus_protocol::{EnvironmentVariable, HarnessProbe, StartRun};
use serde_json::{Map, Value, json};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::watch,
    time::timeout,
};

pub fn prepare_run(request: &StartRun, cwd: &Path) -> (LaunchSpec, EventDecoder) {
    let api_key = request
        .environment
        .iter()
        .find(|variable| variable.name == "CODEX_API_KEY")
        .cloned()
        .or_else(|| {
            env::var("CODEX_API_KEY")
                .ok()
                .map(|value| EnvironmentVariable {
                    name: "CODEX_API_KEY".into(),
                    value,
                })
        })
        .filter(|variable| !variable.value.trim().is_empty());
    let mut args = vec!["app-server".into()];
    if api_key.is_some() {
        // app-server does not read CODEX_API_KEY. Keep its API-key login in memory.
        args.extend([
            "--config".into(),
            "cli_auth_credentials_store=\"ephemeral\"".into(),
        ]);
    }
    (
        LaunchSpec {
            executable: PathBuf::from(&request.executable),
            args,
            cwd: cwd.to_path_buf(),
            stdin: format!(
                "{}\n",
                json!({"id": 0, "method": "initialize", "params": {
                    "clientInfo": {"name": "nexus_agent", "version": env!("CARGO_PKG_VERSION")}
                }})
            ),
        },
        EventDecoder::new(request, cwd, api_key),
    )
}

pub fn build_title_launch_spec(
    executable: &str,
    cwd: &Path,
    prompt: &str,
    model: Option<&str>,
    effort: ThinkingEffort,
) -> LaunchSpec {
    let mut args = vec![
        "exec".into(),
        "--ignore-rules".into(),
        "--skip-git-repo-check".into(),
        "--json".into(),
        "--sandbox".into(),
        "read-only".into(),
        "--ephemeral".into(),
        "--color".into(),
        "never".into(),
    ];
    if !effort.is_default() {
        args.extend([
            "--config".into(),
            format!("model_reasoning_effort=\"{}\"", effort.as_str()),
        ]);
    }
    if let Some(model) = model {
        args.extend(["--model".into(), model.into()]);
    }
    args.push("-".into());

    LaunchSpec {
        executable: PathBuf::from(executable),
        args,
        cwd: cwd.to_path_buf(),
        stdin: prompt.to_owned(),
    }
}

pub async fn probe(configured_executable: &str) -> HarnessProbe {
    let executable = resolve_executable(configured_executable);
    let Some(executable) = executable else {
        return HarnessProbe {
            harness: HarnessKind::Codex,
            available: false,
            authenticated: false,
            executable: configured_executable.to_owned(),
            version: None,
            message: "未找到 Codex CLI。请安装后在设置中填写 codex 可执行文件路径。".into(),
        };
    };

    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let mut version_command = nexus_harness_core::probe::command(&executable);
    let version =
        nexus_harness_core::probe::output(version_command.arg("--version"), deadline).await;
    let version = match version {
        Ok(version) => version,
        Err(error) => {
            return HarnessProbe {
                harness: HarnessKind::Codex,
                available: false,
                authenticated: false,
                executable: executable.display().to_string(),
                version: None,
                message: if error.kind() == std::io::ErrorKind::TimedOut {
                    "Codex CLI 版本探测超时，请重试。".into()
                } else {
                    "Codex CLI 存在，但无法执行。请检查文件权限。".into()
                },
            };
        }
    };
    if !version.status.success() {
        return HarnessProbe {
            harness: HarnessKind::Codex,
            available: false,
            authenticated: false,
            executable: executable.display().to_string(),
            version: None,
            message: "Codex CLI 版本探测失败。".into(),
        };
    }
    let version = String::from_utf8_lossy(&version.stdout).trim().to_owned();

    let mut auth_command = nexus_harness_core::probe::command(&executable);
    let auth =
        nexus_harness_core::probe::output(auth_command.args(["login", "status"]), deadline).await;
    let authenticated = auth.is_ok_and(|output| output.status.success())
        || env::var_os("CODEX_API_KEY").is_some_and(|value| !value.is_empty());

    HarnessProbe {
        harness: HarnessKind::Codex,
        available: true,
        authenticated,
        executable: executable.display().to_string(),
        version: Some(version),
        message: if authenticated {
            "Codex CLI 已就绪".into()
        } else {
            "Codex CLI 尚未登录，请在终端运行 `codex login`。".into()
        },
    }
}

#[cfg(test)]
mod tests;
