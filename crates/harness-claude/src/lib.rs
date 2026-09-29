mod catalog;
mod decoder;
pub use catalog::discover_models;
pub use decoder::EventDecoder;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

use nexus_domain::{
    ClaudeModel, HarnessKind, ModelAvailability, ModelDescriptor, ModelSource, PermissionMode,
    ThinkingEffort, UserAskAnswer, UserAskAnswerMode, UserAskAnswerValue, UserAskOption,
    UserAskQuestion, UserAskStatus,
};
use nexus_harness_core::{
    ApprovalOption, ApprovalPrompt, InputFrame, LineDecoder, ModelCatalogError, UserAskRequest,
    resolve_executable, tool_content,
};
pub use nexus_harness_core::{DecodedEvent, LaunchSpec};
use nexus_protocol::{EnvironmentVariable, HarnessProbe, StartRun};
use serde_json::{Value, json};
use tokio::sync::watch;
pub fn build_launch_spec(
    executable: &str,
    cwd: &Path,
    prompt: &str,
    model: Option<&str>,
    effort: ThinkingEffort,
    session_id: Option<&str>,
    permission_mode: PermissionMode,
) -> LaunchSpec {
    let mut args = vec![
        "--print".into(),
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--include-partial-messages".into(),
        "--replay-user-messages".into(),
        "--permission-mode".into(),
        match permission_mode {
            PermissionMode::Ask => "default",
            PermissionMode::AutoEdit => "acceptEdits",
            PermissionMode::Yolo => "bypassPermissions",
        }
        .into(),
        "--permission-prompt-tool".into(),
        "stdio".into(),
    ];
    if permission_mode == PermissionMode::Yolo {
        args.push("--allow-dangerously-skip-permissions".into());
    }
    if !effort.is_default() {
        args.extend(["--effort".into(), effort.as_str().into()]);
    }
    if let Some(model) = model {
        args.push("--model".into());
        args.push(model.into());
    }
    if let Some(session_id) = session_id {
        args.push("--resume".into());
        args.push(session_id.into());
    }

    LaunchSpec {
        executable: PathBuf::from(executable),
        args,
        cwd: cwd.to_path_buf(),
        stdin: format!("{}\n", user_input(prompt, None)),
    }
}

pub fn prepare_run(request: &StartRun, cwd: &Path) -> Result<LaunchSpec, String> {
    use base64::Engine as _;
    let mut spec = build_launch_spec(
        &request.executable,
        cwd,
        &request.prompt,
        request.model.as_deref(),
        request.effort,
        request.session_id.as_deref(),
        request.permission_mode,
    );
    let mut frame = user_input(&request.prompt, Some(&request.run_id.to_string()));
    if !request.attachments.is_empty() {
        let mut content = vec![json!({"type": "text", "text": request.prompt})];
        for image in &request.attachments {
            let bytes = nexus_harness_core::read_image_attachment(image)?;
            content.push(json!({"type": "text", "text": image.label()}));
            content.push(json!({"type": "image", "source": {
                "type": "base64", "media_type": nexus_harness_core::image_media_type(&bytes).expect("validated image"),
                "data": base64::engine::general_purpose::STANDARD.encode(bytes)
            }}));
        }
        frame["message"]["content"] = content.into();
    }
    spec.stdin = format!("{frame}\n");
    Ok(spec)
}

pub fn build_title_launch_spec(
    executable: &str,
    cwd: &Path,
    prompt: &str,
    model: Option<&str>,
    effort: ThinkingEffort,
) -> LaunchSpec {
    let mut spec = build_launch_spec(
        executable,
        cwd,
        prompt,
        model,
        effort,
        None,
        PermissionMode::AutoEdit,
    );
    if let Some(permission_mode) = spec.args.iter_mut().find(|arg| *arg == "acceptEdits") {
        *permission_mode = "dontAsk".into();
    }
    spec.args.extend(["--tools".into(), String::new()]);
    spec
}

fn user_input(prompt: &str, message_id: Option<&str>) -> Value {
    let mut frame = json!({
        "type": "user", "session_id": "", "parent_tool_use_id": null,
        "message": { "role": "user", "content": prompt }
    });
    if let Some(id) = message_id {
        frame["uuid"] = id.into();
    }
    frame
}

pub async fn probe(configured_executable: &str) -> HarnessProbe {
    let executable = resolve_executable(configured_executable);
    let Some(executable) = executable else {
        return HarnessProbe {
            harness: HarnessKind::Claude,
            available: false,
            authenticated: false,
            executable: configured_executable.to_owned(),
            version: None,
            message: "未找到 Claude Code。请安装后在设置中填写 claude 可执行文件路径。".into(),
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
                harness: HarnessKind::Claude,
                available: false,
                authenticated: false,
                executable: executable.display().to_string(),
                version: None,
                message: if error.kind() == std::io::ErrorKind::TimedOut {
                    "Claude Code 版本探测超时，请重试。".into()
                } else {
                    "Claude Code 存在，但无法执行。请检查文件权限。".into()
                },
            };
        }
    };
    if !version.status.success() {
        return HarnessProbe {
            harness: HarnessKind::Claude,
            available: false,
            authenticated: false,
            executable: executable.display().to_string(),
            version: None,
            message: "Claude Code 版本探测失败。".into(),
        };
    }
    let version = String::from_utf8_lossy(&version.stdout).trim().to_owned();

    let mut auth_command = nexus_harness_core::probe::command(&executable);
    let auth = nexus_harness_core::probe::output(
        auth_command.args(["auth", "status", "--json"]),
        deadline,
    )
    .await;
    let authenticated = auth
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<Value>(&output.stdout).ok())
        .and_then(|value| value.get("loggedIn").and_then(Value::as_bool))
        .unwrap_or(false);

    HarnessProbe {
        harness: HarnessKind::Claude,
        available: true,
        authenticated,
        executable: executable.display().to_string(),
        version: Some(version),
        message: if authenticated {
            "Claude Code 已就绪".into()
        } else {
            "Claude Code 尚未登录，请在终端运行 `claude auth login`。".into()
        },
    }
}

#[cfg(test)]
mod tests;
