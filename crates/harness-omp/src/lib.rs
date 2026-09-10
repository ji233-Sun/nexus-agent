use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use nexus_domain::{
    HarnessKind, ModelDescriptor, ModelReasoningEffort, PermissionMode, ThinkingEffort,
    UserAskAnswer, UserAskAnswerMode, UserAskAnswerValue,
};
pub use nexus_harness_core::{DecodedEvent, LaunchSpec, ModelCatalogError};
use nexus_harness_core::{InputFrame, LineDecoder, resolve_executable};
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

const OTHER_OPTION: &str = "Other (type your own)";

pub struct EventDecoder {
    rpc: nexus_harness_core::rpc::RpcEventDecoder,
    custom_options: HashMap<String, String>,
    custom_answers: VecDeque<(String, String)>,
}

impl Default for EventDecoder {
    fn default() -> Self {
        Self {
            rpc: nexus_harness_core::rpc::RpcEventDecoder::new("Oh My Pi"),
            custom_options: HashMap::new(),
            custom_answers: VecDeque::new(),
        }
    }
}

impl LineDecoder for EventDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let mut events = self.rpc.decode_line(line)?;
        if !self.custom_answers.is_empty() {
            let frame: Value = serde_json::from_str(line)?;
            if frame["type"] == "extension_ui_request" {
                if frame["method"] == "cancel" {
                    self.custom_answers
                        .retain(|(id, _)| Some(id.as_str()) != frame["targetId"].as_str());
                } else if frame["method"] == "editor"
                    && frame["promptStyle"] == true
                    && frame["title"].as_str().is_some_and(|title| {
                        title.contains(OTHER_OPTION) && title.ends_with("\nEnter your response:")
                    })
                    && let [DecodedEvent::UserAskRequested(request)] = events.as_slice()
                    && let Some((_, text)) = self.custom_answers.front()
                    && let Some(response) = self.rpc.answer_user_ask(
                        &request.native_request_id,
                        &[UserAskAnswer {
                            question_id: request.native_request_id.clone(),
                            value: UserAskAnswerValue::Text(text.clone()),
                        }],
                    )
                {
                    // OMP opens these editors in selection-response order. Complete its
                    // native Other -> editor exchange without asking the user twice.
                    self.custom_answers.pop_front();
                    return Ok(vec![DecodedEvent::WriteStdin(response)]);
                }
            }
        }
        for event in &mut events {
            match event {
                DecodedEvent::UserAskRequested(request) => {
                    for question in &mut request.questions {
                        if let UserAskAnswerMode::Choice { allow_custom, .. } =
                            &mut question.answer_mode
                            && let Some(index) = question
                                .options
                                .iter()
                                .position(|option| option.label == OTHER_OPTION)
                        {
                            *allow_custom = true;
                            let other = question.options.remove(index);
                            if question.options.is_empty() {
                                question.answer_mode = UserAskAnswerMode::Text;
                            }
                            self.custom_options
                                .insert(request.native_request_id.clone(), other.id);
                        }
                    }
                }
                DecodedEvent::UserAskFinished {
                    native_request_id, ..
                } => {
                    self.custom_options.remove(native_request_id);
                }
                DecodedEvent::TurnCompleted => {
                    self.custom_options.clear();
                    self.custom_answers.clear();
                }
                _ => {}
            }
        }
        Ok(events)
    }
    fn steer(&mut self, id: &str, prompt: &str) -> Option<InputFrame> {
        self.rpc.steer(id, prompt)
    }
    fn answer_user_ask(&mut self, id: &str, answers: &[UserAskAnswer]) -> Option<InputFrame> {
        let [answer] = answers else { return None };
        if answer.question_id != id {
            return None;
        }
        let response = if let Some(option) = self.custom_options.get(id)
            && let UserAskAnswerValue::Text(text) = &answer.value
        {
            if text.trim().is_empty() {
                return None;
            }
            let response = self.rpc.answer_user_ask(
                id,
                &[UserAskAnswer {
                    question_id: id.into(),
                    value: UserAskAnswerValue::Selected(vec![option.clone()]),
                }],
            )?;
            self.custom_answers.push_back((id.into(), text.clone()));
            response
        } else {
            self.rpc.answer_user_ask(id, answers)?
        };
        self.custom_options.remove(id);
        Some(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_domain::{UserAskAnswerMode, UserAskAnswerValue, UserAskQuestion, UserAskStatus};

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
                json!(["Tests", "Clippy"]),
                "value",
                json!("Clippy"),
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
    fn rpc_custom_choice_uses_inline_text_and_answers_the_native_editor_once() {
        let mut decoder = EventDecoder::default();
        let events = decoder.decode_line(&json!({
            "type": "extension_ui_request", "id": "choice", "method": "select",
            "title": "Which checks?", "timeout": 5000,
            "options": ["Tests", "Other (type your own)", "Clippy"],
            "optionDetails": [{"description": "Run tests"}, {}, {"description": "Lint code"}]
        }).to_string()).unwrap();
        let [DecodedEvent::UserAskRequested(request)] = events.as_slice() else {
            panic!("expected User Ask, got {events:?}");
        };
        assert_eq!(request.timeout_ms, Some(5000));
        assert_eq!(
            request.questions[0].answer_mode,
            UserAskAnswerMode::Choice {
                multiple: false,
                allow_custom: true,
            }
        );
        assert_eq!(
            request.questions[0]
                .options
                .iter()
                .map(|option| (
                    option.id.as_str(),
                    option.label.as_str(),
                    option.description.as_deref()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("0", "Tests", Some("Run tests")),
                ("2", "Clippy", Some("Lint code"))
            ]
        );
        assert!(
            decoder
                .decode_line(
                    &json!({
                        "type": "extension_ui_request", "id": "choice", "method": "select",
                        "options": ["Tests", "Other (type your own)"]
                    })
                    .to_string()
                )
                .unwrap()
                .is_empty()
        );

        let mut answer = [UserAskAnswer {
            question_id: "wrong".into(),
            value: UserAskAnswerValue::Text("只运行测试\n\"details\"".into()),
        }];
        assert!(decoder.answer_user_ask("choice", &answer).is_none());
        answer[0].question_id = "choice".into();
        assert_eq!(
            decoder.answer_user_ask("choice", &answer).unwrap().0,
            json!({"type": "extension_ui_response", "id": "choice", "value": "Other (type your own)"})
        );
        assert!(decoder.answer_user_ask("choice", &answer).is_none());

        // Unrelated text dialogs must not consume the queued custom answer.
        for method in ["input", "editor"] {
            let events = decoder
                .decode_line(
                    &json!({
                        "type": "extension_ui_request", "id": format!("unrelated-{method}"),
                        "method": method, "title": "Another question"
                    })
                    .to_string(),
                )
                .unwrap();
            assert!(matches!(
                events.as_slice(),
                [DecodedEvent::UserAskRequested(_)]
            ));
        }
        let events = decoder
            .decode_line(
                &json!({
                    "type": "extension_ui_request", "id": "custom-editor", "method": "editor",
                    "title": "Which checks?\n\n◉ Other (type your own)\n\nEnter your response:",
                    "promptStyle": true
                })
                .to_string(),
            )
            .unwrap();
        assert_eq!(
            events,
            vec![DecodedEvent::WriteStdin(InputFrame(json!({
                "type": "extension_ui_response", "id": "custom-editor", "value": "只运行测试\n\"details\""
            })))]
        );
        assert!(
            decoder
                .answer_user_ask(
                    "custom-editor",
                    &[UserAskAnswer {
                        question_id: "custom-editor".into(),
                        value: UserAskAnswerValue::Text("again".into()),
                    }]
                )
                .is_none()
        );
    }

    #[test]
    fn rpc_custom_choice_preserves_regular_selection_and_clears_abandoned_answers() {
        for value in [
            UserAskAnswerValue::Selected(vec!["1".into()]),
            UserAskAnswerValue::Text("custom".into()),
        ] {
            for end in [
                json!({"type": "extension_ui_request", "method": "cancel", "targetId": "choice"}),
                json!({"type": "agent_end"}),
            ] {
                let mut decoder = EventDecoder::default();
                decoder
                    .decode_line(
                        &json!({
                            "type": "extension_ui_request", "id": "choice", "method": "select",
                            "title": "Which checks?", "options": ["Other (type your own)", "Tests"]
                        })
                        .to_string(),
                    )
                    .unwrap();
                assert!(
                    decoder
                        .answer_user_ask(
                            "choice",
                            &[UserAskAnswer {
                                question_id: "choice".into(),
                                value: UserAskAnswerValue::Text("  \n".into()),
                            }]
                        )
                        .is_none()
                );
                let response = decoder
                    .answer_user_ask(
                        "choice",
                        &[UserAskAnswer {
                            question_id: "choice".into(),
                            value: value.clone(),
                        }],
                    )
                    .unwrap();
                assert_eq!(
                    response.0["value"],
                    if matches!(value, UserAskAnswerValue::Text(_)) {
                        "Other (type your own)"
                    } else {
                        "Tests"
                    }
                );
                decoder.decode_line(&end.to_string()).unwrap();
                let events = decoder.decode_line(&json!({
                    "type": "extension_ui_request", "id": "later-editor", "method": "editor",
                    "title": "Which checks?\n\n◉ Other (type your own)\n\nEnter your response:",
                    "promptStyle": true
                }).to_string()).unwrap();
                assert!(matches!(
                    events.as_slice(),
                    [DecodedEvent::UserAskRequested(_)]
                ));
            }
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
        let plain =
            json!({"type":"tool_execution_end","result":{"content":[{"type":"text","text":code}]}});
        assert!(
            matches!(decoder.decode_line(&plain.to_string()).unwrap().as_slice(),
            [DecodedEvent::ToolCompleted {output,..}] if output == &code)
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
