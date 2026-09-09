use std::{
    collections::{HashMap, HashSet},
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
    ApprovalOption, ApprovalPrompt, InputFrame, LineDecoder, UserAskRequest, resolve_executable,
    summarize_text, tool_content,
};
pub use nexus_harness_core::{DecodedEvent, LaunchSpec, ModelCatalogError};
use nexus_protocol::{EnvironmentVariable, HarnessProbe};
use serde_json::{Value, json};
use tokio::{io::AsyncReadExt as _, process::Command, sync::watch, time::sleep};

const MODEL_CATALOG_TIMEOUT: Duration = Duration::from_secs(15);
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

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
        "--mode".into(),
        "rpc-ui".into(),
        "--no-title".into(),
        "--approval-mode".into(),
        match permission_mode {
            PermissionMode::Ask => "always-ask",
            PermissionMode::AutoEdit => "write",
            PermissionMode::Yolo => "yolo",
        }
        .into(),
    ];
    if let Some(effort) = omp_thinking_value(effort) {
        args.push("--thinking".into());
        args.push(effort.into());
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
        stdin: format!(
            "{}\n{}\n",
            json!({"type": "get_state", "id": "nexus-session"}),
            json!({"type": "prompt", "id": "nexus-prompt", "message": prompt})
        ),
    }
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
    spec.args.extend([
        "--no-tools".into(),
        "--no-lsp".into(),
        "--no-extensions".into(),
        "--no-skills".into(),
        "--no-rules".into(),
    ]);
    spec
}

fn omp_thinking_value(effort: ThinkingEffort) -> Option<&'static str> {
    match effort {
        ThinkingEffort::Default => None,
        ThinkingEffort::Max => Some(ThinkingEffort::XHigh.as_str()),
        ThinkingEffort::None => Some(ThinkingEffort::Off.as_str()),
        _ => Some(effort.as_str()),
    }
}

pub async fn discover_models(
    configured_executable: &str,
    cwd: &Path,
    environment: &[EnvironmentVariable],
    mut cancel: watch::Receiver<bool>,
) -> Result<Vec<ModelDescriptor>, ModelCatalogError> {
    if *cancel.borrow() {
        return Err(ModelCatalogError::Cancelled);
    }
    let executable = resolve_executable(configured_executable).ok_or_else(|| {
        ModelCatalogError::Failed(
            "未找到 Oh My Pi，无法加载模型目录。请检查可执行文件路径。".into(),
        )
    })?;
    let mut child = Command::new(&executable)
        .args(["models", "--json"])
        .envs(
            environment
                .iter()
                .map(|variable| (&variable.name, &variable.value)),
        )
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| {
            ModelCatalogError::Failed(
                "无法启动 Oh My Pi 模型目录命令。请检查 CLI 版本和可执行文件权限。".into(),
            )
        })?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ModelCatalogError::Failed("无法读取 Oh My Pi 模型目录输出。".into()))?;

    enum Collection {
        Complete(Result<(std::process::ExitStatus, Vec<u8>), ()>),
        Cancelled,
        TimedOut,
    }
    let collection = {
        let collect = async {
            let mut output = Vec::new();
            stdout.read_to_end(&mut output).await.map_err(|_| ())?;
            let status = child.wait().await.map_err(|_| ())?;
            Ok((status, output))
        };
        tokio::pin!(collect);
        let timeout = sleep(MODEL_CATALOG_TIMEOUT);
        tokio::pin!(timeout);
        tokio::select! {
            result = &mut collect => Collection::Complete(result),
            _ = cancel.changed() => Collection::Cancelled,
            _ = &mut timeout => Collection::TimedOut,
        }
    };

    let (status, output) = match collection {
        Collection::Complete(Ok(output)) => output,
        Collection::Complete(Err(())) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(ModelCatalogError::Failed(
                "执行 Oh My Pi 模型目录命令失败。".into(),
            ));
        }
        Collection::Cancelled => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(ModelCatalogError::Cancelled);
        }
        Collection::TimedOut => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(ModelCatalogError::Failed(
                "Oh My Pi 模型目录命令超时，请重试。".into(),
            ));
        }
    };
    if !status.success() {
        return Err(ModelCatalogError::Failed(
            "Oh My Pi 模型目录命令执行失败。请检查 Provider 配置后重试。".into(),
        ));
    }
    parse_model_catalog(&output)
}

fn parse_model_catalog(output: &[u8]) -> Result<Vec<ModelDescriptor>, ModelCatalogError> {
    let value: Value = serde_json::from_slice(output)
        .map_err(|_| ModelCatalogError::Failed("Oh My Pi 模型目录返回了无效 JSON。".into()))?;
    let items = value
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| ModelCatalogError::Failed("Oh My Pi 模型目录响应缺少 models。".into()))?;
    let mut selectors = HashSet::new();
    let mut models = Vec::with_capacity(items.len());
    for item in items {
        let provider = required_catalog_string(item, "provider")?;
        let selector = required_catalog_string(item, "selector")?;
        if !selectors.insert(selector.clone()) {
            return Err(ModelCatalogError::Failed(format!(
                "Oh My Pi 模型目录包含重复标识：{selector}。"
            )));
        }
        let display_name = item
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or(&selector)
            .to_owned();
        let mut supported_reasoning_efforts = Vec::new();
        for value in item
            .get("thinking")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let value = value.as_str().ok_or_else(|| {
                ModelCatalogError::Failed("Oh My Pi 模型目录包含无效的 thinking 能力项。".into())
            })?;
            let effort = value.parse().map_err(|_| {
                ModelCatalogError::Failed(format!(
                    "Oh My Pi 模型目录包含未知的 thinking 值：{value}。"
                ))
            })?;
            if !supported_reasoning_efforts
                .iter()
                .any(|option: &ModelReasoningEffort| option.effort == effort)
            {
                supported_reasoning_efforts.push(ModelReasoningEffort {
                    effort,
                    description: String::new(),
                });
            }
        }
        models.push(ModelDescriptor {
            id: selector,
            display_name,
            source: nexus_domain::ModelSource::OmpCli,
            availability: nexus_domain::ModelAvailability::Available,
            provider: Some(provider),
            is_default: false,
            supported_reasoning_efforts,
            default_reasoning_effort: None,
        });
    }
    Ok(models)
}

fn required_catalog_string(item: &Value, field: &str) -> Result<String, ModelCatalogError> {
    item.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            ModelCatalogError::Failed(format!("Oh My Pi 模型目录包含缺少 {field} 的模型。"))
        })
}

pub async fn probe(configured_executable: &str) -> HarnessProbe {
    let executable = resolve_executable(configured_executable);
    let Some(executable) = executable else {
        return HarnessProbe {
            harness: HarnessKind::Omp,
            available: false,
            authenticated: false,
            executable: configured_executable.to_owned(),
            version: None,
            message: "未找到 Oh My Pi。请安装后在设置中填写 omp 可执行文件路径。".into(),
        };
    };

    let deadline = tokio::time::Instant::now() + PROBE_TIMEOUT;
    let version = tokio::time::timeout_at(
        deadline,
        Command::new(&executable)
            .arg("--version")
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let Ok(Ok(version)) = version else {
        return HarnessProbe {
            harness: HarnessKind::Omp,
            available: false,
            authenticated: false,
            executable: executable.display().to_string(),
            version: None,
            message: if version.is_err() {
                "Oh My Pi 版本探测超时，请重试。".into()
            } else {
                "Oh My Pi 存在，但无法执行。请检查文件权限。".into()
            },
        };
    };
    if !version.status.success() {
        return HarnessProbe {
            harness: HarnessKind::Omp,
            available: false,
            authenticated: false,
            executable: executable.display().to_string(),
            version: None,
            message: "Oh My Pi 版本探测失败。".into(),
        };
    }
    let version = String::from_utf8_lossy(&version.stdout).trim().to_owned();
    let models = tokio::time::timeout_at(
        deadline,
        Command::new(&executable)
            .args(["models", "--json"])
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let timed_out = models.is_err();
    let authenticated = models
        .ok()
        .and_then(Result::ok)
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<Value>(&output.stdout).ok())
        .and_then(|value| value.get("models").and_then(Value::as_array).map(Vec::len))
        .is_some_and(|count| count > 0);

    HarnessProbe {
        harness: HarnessKind::Omp,
        available: true,
        authenticated,
        executable: executable.display().to_string(),
        version: Some(version),
        message: if authenticated {
            "Oh My Pi 已就绪".into()
        } else if timed_out {
            "Oh My Pi 模型探测超时，请重试。".into()
        } else {
            "Oh My Pi 尚无可用模型，请先完成登录或配置 Provider。".into()
        },
    }
}

#[derive(Default)]
pub struct EventDecoder {
    pending_user_asks: HashMap<String, Vec<ApprovalOption>>,
}

impl LineDecoder for EventDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let frame: Value = serde_json::from_str(line)?;
        let events = if frame["type"] == "extension_ui_request" {
            self.decode_ui_request(&frame)
        } else {
            decode_frame(&frame)
        };
        if events.contains(&DecodedEvent::TurnCompleted) {
            self.pending_user_asks.clear();
        }
        Ok(events)
    }

    fn steer(&mut self, message_id: &str, prompt: &str) -> Option<InputFrame> {
        Some(InputFrame(
            json!({"type": "steer", "id": message_id, "message": prompt}),
        ))
    }

    fn answer_user_ask(
        &mut self,
        native_request_id: &str,
        answers: &[UserAskAnswer],
    ) -> Option<InputFrame> {
        let [answer] = answers else { return None };
        if answer.question_id != native_request_id {
            return None;
        }
        let options = self.pending_user_asks.get(native_request_id)?;
        let response = match &answer.value {
            UserAskAnswerValue::Text(value) if options.is_empty() => InputFrame(json!({
                "type": "extension_ui_response", "id": native_request_id, "value": value
            })),
            UserAskAnswerValue::Selected(values) if values.len() == 1 => options
                .get(values[0].parse::<usize>().ok()?)?
                .response
                .clone(),
            _ => return None,
        };
        self.pending_user_asks.remove(native_request_id);
        Some(response)
    }
}

fn decode_frame(frame: &Value) -> Vec<DecodedEvent> {
    match frame.get("type").and_then(Value::as_str) {
        Some("response") => {
            let id = frame.get("id").and_then(Value::as_str).unwrap_or_default();
            let command = frame
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if frame.get("success").and_then(Value::as_bool) != Some(true) {
                let message = frame
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("Oh My Pi 请求失败。")
                    .to_owned();
                return if command == "steer" {
                    vec![DecodedEvent::InputRejected {
                        id: id.into(),
                        message,
                    }]
                } else {
                    vec![DecodedEvent::Error(message), DecodedEvent::TurnCompleted]
                };
            }
            match (command, id) {
                ("get_state", "nexus-session") => match frame
                    .pointer("/data/sessionId")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                {
                    Some(id) => vec![DecodedEvent::SessionStarted(id.into())],
                    None => vec![
                        DecodedEvent::Error("Oh My Pi 未返回会话 ID。".into()),
                        DecodedEvent::TurnCompleted,
                    ],
                },
                ("steer", _) => vec![DecodedEvent::InputAccepted(id.into())],
                ("prompt", _)
                    if frame.pointer("/data/agentInvoked").and_then(Value::as_bool)
                        == Some(false) =>
                {
                    vec![DecodedEvent::TurnCompleted]
                }
                _ => Vec::new(),
            }
        }
        Some("prompt_result")
            if frame.get("agentInvoked").and_then(Value::as_bool) == Some(false) =>
        {
            vec![DecodedEvent::TurnCompleted]
        }
        Some("agent_end") if frame.get("isTerminal").and_then(Value::as_bool) != Some(false) => {
            vec![DecodedEvent::TurnCompleted]
        }
        Some("agent_start") => vec![DecodedEvent::Status("Oh My Pi 会话已启动".into())],
        Some("turn_start") => vec![DecodedEvent::Status("Oh My Pi 正在处理任务…".into())],
        Some("message_update") => frame
            .get("assistantMessageEvent")
            .filter(|event| event.get("type").and_then(Value::as_str) == Some("text_delta"))
            .and_then(|event| event.get("delta").and_then(Value::as_str))
            .filter(|text| !text.is_empty())
            .map(|text| vec![DecodedEvent::TextDelta(text.to_owned())])
            .unwrap_or_default(),
        Some("message_end") => decode_message(frame),
        Some("tool_execution_start") => vec![DecodedEvent::ToolStarted {
            id: event_string(frame, "toolCallId", "unknown"),
            name: event_string(frame, "toolName", "Tool"),
            summary: frame.get("args").map(tool_content).unwrap_or_default(),
        }],
        Some("tool_execution_end") => vec![DecodedEvent::ToolCompleted {
            id: event_string(frame, "toolCallId", "unknown"),
            output: frame
                .get("result")
                .map(format_tool_result)
                .unwrap_or_default(),
            is_error: frame
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }],
        Some("notice") => frame
            .get("message")
            .and_then(Value::as_str)
            .map(|message| {
                if frame.get("level").and_then(Value::as_str) == Some("error") {
                    DecodedEvent::Error(summarize_text(message))
                } else {
                    DecodedEvent::Status(format!("Oh My Pi: {}", summarize_text(message)))
                }
            })
            .into_iter()
            .collect(),
        Some("auto_retry_start") => vec![DecodedEvent::Status("Oh My Pi 正在重试请求…".into())],
        Some("auto_retry_end") if frame.get("success").and_then(Value::as_bool) == Some(false) => {
            frame
                .get("finalError")
                .and_then(Value::as_str)
                .map(|message| vec![DecodedEvent::Error(summarize_text(message))])
                .unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

impl EventDecoder {
    fn decode_ui_request(&mut self, frame: &Value) -> Vec<DecodedEvent> {
        let method = frame
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if method == "cancel" {
            if let Some(id) = frame["targetId"].as_str()
                && self.pending_user_asks.remove(id).is_some()
            {
                return vec![DecodedEvent::UserAskFinished {
                    native_request_id: id.into(),
                    status: UserAskStatus::Cancelled,
                    message: None,
                }];
            }
            return vec![DecodedEvent::ApprovalResolved(
                frame["targetId"].to_string(),
            )];
        }
        let cancel = InputFrame(
            json!({"type": "extension_ui_response", "id": frame["id"], "cancelled": true}),
        );
        let options = match method {
            "select" => frame
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|label| ApprovalOption {
                    label: label.into(),
                    response: InputFrame(json!({
                        "type": "extension_ui_response", "id": frame["id"], "value": label
                    })),
                })
                .collect::<Vec<_>>(),
            "confirm" => [("Yes", true), ("No", false)]
                .into_iter()
                .map(|(label, confirmed)| ApprovalOption {
                    label: label.into(),
                    response: InputFrame(json!({
                        "type": "extension_ui_response", "id": frame["id"], "confirmed": confirmed
                    })),
                })
                .collect(),
            "input" | "editor" => Vec::new(),
            // Non-dialog notifications do not expect a response.
            "notify" | "setStatus" | "setWidget" | "setTitle" | "set_editor_text" | "open_url" => {
                return Vec::new();
            }
            _ => return vec![DecodedEvent::WriteStdin(cancel)],
        };
        let Some(id) = frame["id"].as_str().filter(|id| !id.trim().is_empty()) else {
            return vec![DecodedEvent::WriteStdin(cancel)];
        };
        if matches!(method, "select" | "confirm") && options.is_empty() {
            return vec![DecodedEvent::WriteStdin(cancel)];
        }
        let prompt = ["title", "message", "placeholder", "prefill"]
            .into_iter()
            .filter_map(|key| frame[key].as_str())
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        // OMP's tool approval wrapper uses this exact select prompt and option pair.
        if method == "select"
            && frame["title"]
                .as_str()
                .is_some_and(|title| title.starts_with("Allow tool: "))
            && options
                .iter()
                .map(|option| option.label.as_str())
                .eq(["Approve", "Deny"])
        {
            return vec![DecodedEvent::ApprovalRequested(ApprovalPrompt {
                id: frame["id"].to_string(),
                title: "Oh My Pi".into(),
                details: prompt,
                options,
                cancel,
                timeout_ms: frame.get("timeout").and_then(Value::as_u64),
            })];
        }
        if self.pending_user_asks.contains_key(id) {
            return Vec::new();
        }
        let question = UserAskQuestion {
            id: id.into(),
            prompt: if prompt.is_empty() {
                "Oh My Pi".into()
            } else {
                prompt
            },
            answer_mode: if options.is_empty() {
                UserAskAnswerMode::Text
            } else {
                UserAskAnswerMode::Choice {
                    multiple: false,
                    allow_custom: false,
                }
            },
            options: options
                .iter()
                .enumerate()
                .map(|(index, option)| UserAskOption {
                    id: index.to_string(),
                    label: option.label.clone(),
                    description: frame["optionDetails"]
                        .get(index)
                        .and_then(|detail| detail["description"].as_str())
                        .map(str::to_owned),
                })
                .collect(),
        };
        self.pending_user_asks.insert(id.into(), options);
        vec![DecodedEvent::UserAskRequested(UserAskRequest {
            native_request_id: id.into(),
            questions: vec![question],
            timeout_ms: frame.get("timeout").and_then(Value::as_u64),
            resolve_on_send: true,
        })]
    }
}

fn decode_message(frame: &Value) -> Vec<DecodedEvent> {
    let Some(message) = frame.get("message") else {
        return Vec::new();
    };
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return Vec::new();
    }
    if matches!(
        message.get("stopReason").and_then(Value::as_str),
        Some("error" | "aborted")
    ) && let Some(error) = message.get("errorMessage").and_then(Value::as_str)
    {
        return vec![DecodedEvent::Error(summarize_text(error))];
    }

    let text = message
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<String>();
    (!text.is_empty())
        .then_some(DecodedEvent::MessageCompleted(text))
        .into_iter()
        .collect()
}

fn format_tool_result(result: &Value) -> String {
    // Edit tools include the actual patch in details (including hashline edits).
    if result.get("details").is_some() {
        return tool_content(result);
    }
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        tool_content(result)
    } else {
        text
    }
}

fn event_string(frame: &Value, key: &str, fallback: &str) -> String {
    frame
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_spec_uses_rpc_ui_without_prompt_in_argv() {
        for (mode, value) in [
            (PermissionMode::Ask, "always-ask"),
            (PermissionMode::AutoEdit, "write"),
            (PermissionMode::Yolo, "yolo"),
        ] {
            let spec = build_launch_spec(
                "omp",
                Path::new("."),
                "test",
                None,
                ThinkingEffort::Default,
                Some("session"),
                mode,
            );
            assert!(
                spec.args
                    .windows(2)
                    .any(|pair| pair == ["--approval-mode", value])
            );
        }
        let spec = build_launch_spec(
            "/usr/local/bin/omp",
            Path::new("/tmp/project"),
            "secret prompt",
            Some("deepseek/deepseek-v4-pro"),
            ThinkingEffort::High,
            None,
            PermissionMode::AutoEdit,
        );
        assert!(
            spec.args
                .windows(2)
                .any(|pair| pair == ["--mode", "rpc-ui"])
        );
        assert!(!spec.args.iter().any(|arg| arg == "--print"));
        assert!(
            spec.args
                .windows(2)
                .any(|pair| pair == ["--approval-mode", "write"])
        );
        assert!(
            spec.args
                .windows(2)
                .any(|pair| pair == ["--thinking", "high"])
        );
        assert!(
            spec.args
                .windows(2)
                .any(|pair| pair == ["--model", "deepseek/deepseek-v4-pro"])
        );
        assert!(!spec.args.iter().any(|arg| arg.contains("secret prompt")));
        let frames: Vec<Value> = spec
            .stdin
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(frames[0]["type"], "get_state");
        assert_eq!(frames[1]["message"], "secret prompt");
        assert!(
            !spec
                .args
                .iter()
                .any(|arg| arg == "--no-session" || arg == "--resume")
        );
        let resumed = build_launch_spec(
            "omp",
            Path::new("/tmp/project"),
            "follow-up",
            None,
            ThinkingEffort::High,
            Some("existing-session"),
            PermissionMode::AutoEdit,
        );
        assert!(
            resumed
                .args
                .windows(2)
                .any(|pair| pair == ["--resume", "existing-session"])
        );
        assert!(
            resumed
                .args
                .windows(2)
                .any(|pair| pair == ["--approval-mode", "write"])
        );
        assert!(
            !resumed
                .args
                .iter()
                .any(|arg| arg == "--no-session" || arg == "--continue")
        );
        assert_eq!(
            serde_json::from_str::<Value>(resumed.stdin.lines().nth(1).unwrap()).unwrap()["message"],
            "follow-up"
        );

        let max_spec = build_launch_spec(
            "omp",
            Path::new("/tmp/project"),
            "prompt",
            None,
            ThinkingEffort::Max,
            None,
            PermissionMode::AutoEdit,
        );
        assert!(
            max_spec
                .args
                .windows(2)
                .any(|pair| pair == ["--thinking", "xhigh"])
        );
        assert!(!max_spec.args.iter().any(|arg| arg == "max"));

        let default_spec = build_launch_spec(
            "omp",
            Path::new("/tmp/project"),
            "prompt",
            None,
            ThinkingEffort::Default,
            None,
            PermissionMode::AutoEdit,
        );
        assert!(!default_spec.args.iter().any(|arg| arg == "--thinking"));

        let legacy_none_spec = build_launch_spec(
            "omp",
            Path::new("/tmp/project"),
            "prompt",
            None,
            ThinkingEffort::None,
            None,
            PermissionMode::AutoEdit,
        );
        assert!(
            legacy_none_spec
                .args
                .windows(2)
                .any(|pair| pair == ["--thinking", "off"])
        );
    }

    #[test]
    fn rpc_choice_dialogs_preserve_options_and_native_answer_types() {
        for (method, options, answer_key, expected) in [
            (
                "select",
                json!(["Tests", "Other (type your own)"]),
                "value",
                json!("Other (type your own)"),
            ),
            ("confirm", Value::Null, "confirmed", json!(false)),
        ] {
            let mut decoder = EventDecoder::default();
            let frame = json!({"type":"extension_ui_request", "id":"choice", "method":method,
                "title":"Which checks?", "options":options, "timeout":5000,
                "optionDetails":[{"description":"Run tests"},{}]});
            let events = decoder.decode_line(&frame.to_string()).unwrap();
            let [DecodedEvent::UserAskRequested(request)] = events.as_slice() else {
                panic!("expected User Ask, got {events:?}")
            };
            assert_eq!(request.timeout_ms, Some(5000));
            assert_eq!(
                request.questions[0].answer_mode,
                UserAskAnswerMode::Choice {
                    multiple: false,
                    allow_custom: false
                }
            );
            if method == "select" {
                assert_eq!(
                    request.questions[0].options[0].description.as_deref(),
                    Some("Run tests")
                );
            }
            let answer = [UserAskAnswer {
                question_id: "choice".into(),
                value: UserAskAnswerValue::Selected(vec![
                    request.questions[0].options[1].id.clone(),
                ]),
            }];
            let response = decoder.answer_user_ask("choice", &answer).unwrap().0;
            assert_eq!(response[answer_key], expected);
            assert!(decoder.answer_user_ask("choice", &answer).is_none());
            let cancel_frame = json!({"type":"extension_ui_request","id":"cancelled-choice","method":method,"title":"Question?","options":options});
            decoder.decode_line(&cancel_frame.to_string()).unwrap();
            assert_eq!(decoder.decode_line(r#"{"type":"extension_ui_request","id":"cancel","method":"cancel","targetId":"cancelled-choice"}"#).unwrap(), vec![DecodedEvent::UserAskFinished {
                native_request_id:"cancelled-choice".into(), status:UserAskStatus::Cancelled, message:None,
            }]);
            assert!(
                decoder
                    .answer_user_ask(
                        "cancelled-choice",
                        &[UserAskAnswer {
                            question_id: "cancelled-choice".into(),
                            value: UserAskAnswerValue::Selected(vec!["0".into()])
                        }]
                    )
                    .is_none()
            );
        }
    }

    #[test]
    fn rpc_tool_approval_preserves_choices_timeout_and_cancel() {
        let mut decoder = EventDecoder::default();
        let frame = json!({"type": "extension_ui_request", "id": "approval-1", "method": "select", "title": "Allow tool: bash", "timeout": 1000, "options": ["Approve", "Deny"]});
        let events = decoder.decode_line(&frame.to_string()).unwrap();
        let [DecodedEvent::ApprovalRequested(prompt)] = events.as_slice() else {
            panic!("expected approval")
        };
        assert_eq!(prompt.options.len(), 2);
        assert_eq!(prompt.timeout_ms, Some(1000));
        assert!(prompt.details.contains("Allow tool: bash"));
        assert_eq!(prompt.options[0].response.0["value"], "Approve");
        assert_eq!(prompt.options[1].response.0["value"], "Deny");
        assert_eq!(prompt.cancel.0["cancelled"], true);
        assert_eq!(decoder.decode_line(r#"{"type":"extension_ui_request","method":"cancel","id":"other","targetId":"approval-1"}"#).unwrap(), vec![
                DecodedEvent::ApprovalResolved(prompt.id.clone()),
            ]);
    }

    #[test]
    fn rpc_text_dialogs_round_trip_without_cancelling() {
        let mut decoder = EventDecoder::default();
        for method in ["input", "editor"] {
            let frame = json!({"type": "extension_ui_request", "id": "ui_1",
                "method": method, "title": "Branch name", "timeout": 1234});
            let events = decoder.decode_line(&frame.to_string()).unwrap();
            let [DecodedEvent::UserAskRequested(request)] = events.as_slice() else {
                panic!("expected User Ask, got {events:?}");
            };
            assert_eq!(request.native_request_id, "ui_1");
            assert_eq!(request.timeout_ms, Some(1234));
            assert!(request.resolve_on_send);
            assert_eq!(
                request.questions,
                vec![UserAskQuestion {
                    id: "ui_1".into(),
                    prompt: "Branch name".into(),
                    answer_mode: UserAskAnswerMode::Text,
                    options: vec![],
                }]
            );
            let mut answers = vec![UserAskAnswer {
                question_id: "ui_1".into(),
                value: UserAskAnswerValue::Text("feature/修复\n\"details\"".into()),
            }];
            assert_eq!(
                decoder.answer_user_ask("ui_1", &answers).unwrap().0,
                json!({"type": "extension_ui_response", "id": "ui_1", "value": "feature/修复\n\"details\""})
            );
            assert!(decoder.answer_user_ask("other", &answers).is_none());
            assert!(decoder.answer_user_ask("ui_1", &[]).is_none());
            answers[0].value = UserAskAnswerValue::Selected(vec!["wrong".into()]);
            assert!(decoder.answer_user_ask("ui_1", &answers).is_none());
        }
        let events = decoder.decode_line(r#"{"type":"extension_ui_request","id":"editor-2","method":"editor","title":"Custom answer","prefill":"existing text"}"#).unwrap();
        assert!(
            matches!(events.as_slice(), [DecodedEvent::UserAskRequested(request)]
            if request.questions[0].prompt == "Custom answer\n\nexisting text"
                && request.timeout_ms.is_none())
        );
    }

    #[test]
    fn model_catalog_parses_real_omp_shape_and_preserves_provider_selectors() {
        let output = br#"{
          "models": [
            {
              "provider": "bigmodel",
              "id": "glm-5.2",
              "selector": "bigmodel/glm-5.2",
              "name": "GLM-5.2",
              "contextWindow": 1048576,
              "maxTokens": 131072,
              "reasoning": true,
              "thinking": ["minimal", "low", "medium", "high", "xhigh"],
              "input": ["text"],
              "cost": {}
            },
            {
              "provider": "second-provider",
              "id": "glm-5.2",
              "selector": "second-provider/glm-5.2",
              "name": "GLM-5.2",
              "reasoning": true,
              "thinking": ["off", "auto"]
            }
          ]
        }"#;

        let models = parse_model_catalog(output).unwrap();

        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "bigmodel/glm-5.2");
        assert_eq!(models[0].provider.as_deref(), Some("bigmodel"));
        assert_eq!(models[1].id, "second-provider/glm-5.2");
        assert_eq!(models[0].display_name, models[1].display_name);
        assert_eq!(
            models[0]
                .supported_reasoning_efforts
                .iter()
                .map(|option| option.effort)
                .collect::<Vec<_>>(),
            [
                ThinkingEffort::Minimal,
                ThinkingEffort::Low,
                ThinkingEffort::Medium,
                ThinkingEffort::High,
                ThinkingEffort::XHigh,
            ]
        );
        assert_eq!(
            models[1]
                .supported_reasoning_efforts
                .iter()
                .map(|option| option.effort)
                .collect::<Vec<_>>(),
            [ThinkingEffort::Off, ThinkingEffort::Auto]
        );
    }

    #[test]
    fn model_catalog_distinguishes_empty_and_malformed_responses() {
        assert!(
            parse_model_catalog(br#"{"models": []}"#)
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            parse_model_catalog(b"not json"),
            Err(ModelCatalogError::Failed(message)) if message.contains("无效 JSON")
        ));
        assert!(matches!(
            parse_model_catalog(br#"{}"#),
            Err(ModelCatalogError::Failed(message)) if message.contains("缺少 models")
        ));
        assert!(matches!(
            parse_model_catalog(br#"{"models":[{"provider":"p"}]}"#),
            Err(ModelCatalogError::Failed(message)) if message.contains("selector")
        ));
        assert!(matches!(
            parse_model_catalog(br#"{"models":[
                {"provider":"a","selector":"same","thinking":[]},
                {"provider":"b","selector":"same","thinking":[]}
            ]}"#),
            Err(ModelCatalogError::Failed(message)) if message.contains("重复标识")
        ));
    }

    #[test]
    fn title_launch_spec_disables_tools_and_project_extensions() {
        let spec = build_title_launch_spec(
            "/usr/local/bin/omp",
            Path::new("/tmp/project"),
            "title prompt",
            Some("openai/gpt-test"),
            ThinkingEffort::Low,
        );

        for flag in [
            "--no-tools",
            "--no-lsp",
            "--no-extensions",
            "--no-skills",
            "--no-rules",
        ] {
            assert!(spec.args.iter().any(|arg| arg == flag));
        }
        assert!(!spec.args.iter().any(|arg| arg.contains("title prompt")));
    }

    #[test]
    fn decoder_maps_stream_messages_and_tool_events() {
        let mut decoder = EventDecoder::default();
        assert_eq!(
            decoder
                .decode_line(r#"{"type":"response","command":"get_state","id":"nexus-session","success":true,"data":{"sessionId":"existing-session"}}"#)
                .unwrap(),
            vec![DecodedEvent::SessionStarted("existing-session".into())]
        );
        let delta = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"你好"}}"#;
        assert_eq!(
            decoder.decode_line(delta).unwrap(),
            vec![DecodedEvent::TextDelta("你好".into())]
        );

        let started = r#"{"type":"tool_execution_start","toolCallId":"t1","toolName":"read","args":{"path":"README.md"}}"#;
        assert!(matches!(
            decoder.decode_line(started).unwrap().as_slice(),
            [DecodedEvent::ToolStarted { id, name, summary }]
                if id == "t1" && name == "read" && summary.contains("README.md")
        ));

        let completed = r#"{"type":"tool_execution_end","toolCallId":"t1","toolName":"read","result":{"content":[{"type":"text","text":"contents"}]},"isError":false}"#;
        assert_eq!(
            decoder.decode_line(completed).unwrap(),
            vec![DecodedEvent::ToolCompleted {
                id: "t1".into(),
                output: "contents".into(),
                is_error: false,
            }]
        );

        let message = r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"stopReason":"stop"}}"#;
        assert_eq!(
            decoder.decode_line(message).unwrap(),
            vec![DecodedEvent::MessageCompleted("done".into())]
        );

        let code = "完整修改内容\n".repeat(100);
        let args = serde_json::json!({"path": "main.rs", "content": code});
        let started = serde_json::json!({"type": "tool_execution_start", "toolCallId": "write",
            "toolName": "write", "args": args});
        assert!(matches!(
            decoder.decode_line(&started.to_string()).unwrap().as_slice(),
            [DecodedEvent::ToolStarted { summary, .. }]
                if serde_json::from_str::<Value>(summary).unwrap() == args
        ));
        let result = serde_json::json!({"content": [{"type": "text", "text": code}],
            "details": {"diff": "-old\n+new"}});
        let completed = serde_json::json!({"type": "tool_execution_end", "toolCallId": "write",
            "result": result, "isError": false});
        assert!(matches!(
            decoder.decode_line(&completed.to_string()).unwrap().as_slice(),
            [DecodedEvent::ToolCompleted { output, .. }]
                if serde_json::from_str::<Value>(output).unwrap() == result
        ));
        assert_eq!(
            format_tool_result(&serde_json::json!({
                "content": [{"type": "text", "text": code}]
            })),
            code
        );
    }

    #[test]
    fn steering_receipts_and_terminal_events_are_distinct() {
        let mut decoder = EventDecoder::default();
        let frame = decoder.steer("message-1", "update\n第二行").unwrap();
        assert_eq!(
            frame.0,
            json!({"type": "steer", "id": "message-1", "message": "update\n第二行"})
        );
        assert_eq!(
            decoder
                .decode_line(
                    r#"{"type":"response","command":"steer","id":"message-1","success":true}"#
                )
                .unwrap(),
            vec![DecodedEvent::InputAccepted("message-1".into())]
        );
        assert_eq!(decoder.decode_line(r#"{"type":"response","command":"steer","id":"message-1","success":false,"error":"ended"}"#).unwrap(),
            vec![DecodedEvent::InputRejected { id: "message-1".into(), message: "ended".into() }]);
        assert!(
            decoder
                .decode_line(r#"{"type":"agent_end","isTerminal":false}"#)
                .unwrap()
                .is_empty()
        );
        for terminal in [
            r#"{"type":"agent_end","isTerminal":true}"#,
            r#"{"type":"prompt_result","agentInvoked":false}"#,
        ] {
            assert_eq!(
                decoder.decode_line(terminal).unwrap(),
                vec![DecodedEvent::TurnCompleted]
            );
        }
        assert!(matches!(
            decoder
                .decode_line(
                    r#"{"type":"response","command":"prompt","success":false,"error":"denied"}"#
                )
                .unwrap()
                .as_slice(),
            [DecodedEvent::Error(_), DecodedEvent::TurnCompleted]
        ));
    }

    #[test]
    fn decoder_surfaces_provider_errors() {
        let mut decoder = EventDecoder::default();
        let message = r#"{"type":"message_end","message":{"role":"assistant","content":[],"stopReason":"error","errorMessage":"denied"}}"#;
        assert_eq!(
            decoder.decode_line(message).unwrap(),
            vec![DecodedEvent::Error("denied".into())]
        );
        assert!(decoder.decode_line("not json").is_err());
    }
}
