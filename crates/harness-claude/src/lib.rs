use std::{
    collections::HashMap,
    path::{Path, PathBuf},
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
use nexus_protocol::{EnvironmentVariable, HarnessProbe};
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::sync::watch;

pub async fn discover_models(
    executable: &str,
    _cwd: &Path,
    _environment: &[EnvironmentVariable],
    cancel: watch::Receiver<bool>,
) -> Result<Vec<ModelDescriptor>, ModelCatalogError> {
    if *cancel.borrow() {
        return Err(ModelCatalogError::Cancelled);
    }
    if resolve_executable(executable).is_none() {
        return Err(ModelCatalogError::Failed(
            "未找到 Claude Code，无法加载模型别名。".into(),
        ));
    }
    // These are CLI aliases, not a discovered account catalog. Version-specific
    // model capabilities are deliberately left unknown until the adapter reports them.
    Ok(ClaudeModel::ALL
        .into_iter()
        .filter_map(|model| {
            Some(ModelDescriptor {
                id: model.cli_value()?.into(),
                display_name: model.to_string(),
                source: ModelSource::ClaudeAliases,
                availability: ModelAvailability::Unknown,
                provider: None,
                is_default: false,
                supported_reasoning_efforts: Vec::new(),
                default_reasoning_effort: None,
            })
        })
        .collect())
}

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

    let version = Command::new(&executable).arg("--version").output().await;
    let Ok(version) = version else {
        return HarnessProbe {
            harness: HarnessKind::Claude,
            available: false,
            authenticated: false,
            executable: executable.display().to_string(),
            version: None,
            message: "Claude Code 存在，但无法执行。请检查文件权限。".into(),
        };
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

    let auth = Command::new(&executable)
        .args(["auth", "status", "--json"])
        .output()
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

#[derive(Default)]
pub struct EventDecoder {
    pending_user_asks: HashMap<String, Value>,
}

impl LineDecoder for EventDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let frame: Value = serde_json::from_str(line)?;
        if frame.get("type").and_then(Value::as_str) == Some("control_request")
            && frame.pointer("/request/subtype").and_then(Value::as_str) == Some("can_use_tool")
            && frame.pointer("/request/tool_name").and_then(Value::as_str)
                == Some("AskUserQuestion")
        {
            return Ok(self.decode_user_ask(&frame));
        }
        if frame.get("type").and_then(Value::as_str) == Some("control_cancel_request")
            && let Some(id) = frame.get("request_id").and_then(Value::as_str)
            && self.pending_user_asks.remove(id).is_some()
        {
            return Ok(vec![DecodedEvent::UserAskFinished {
                native_request_id: id.into(),
                status: UserAskStatus::Cancelled,
                message: None,
            }]);
        }
        if frame.get("type").and_then(Value::as_str) == Some("result") {
            self.pending_user_asks.clear();
        }
        Ok(decode_frame(&frame))
    }

    fn steer(&mut self, message_id: &str, prompt: &str) -> Option<InputFrame> {
        Some(InputFrame(user_input(prompt, Some(message_id))))
    }

    fn answer_user_ask(
        &mut self,
        native_request_id: &str,
        answers: &[UserAskAnswer],
    ) -> Option<InputFrame> {
        let input = self.pending_user_asks.get(native_request_id)?;
        let questions = input.get("questions")?.as_array()?;
        if answers.len() != questions.len() {
            return None;
        }
        let mut response_answers = serde_json::Map::new();
        for question in questions {
            let text = question.get("question")?.as_str()?;
            let answer = answers.iter().find(|answer| answer.question_id == text)?;
            let value = match &answer.value {
                UserAskAnswerValue::Text(value) => value.clone(),
                UserAskAnswerValue::Selected(values) => values.join(", "),
            };
            response_answers.insert(text.into(), Value::String(value));
        }
        let mut input = self.pending_user_asks.remove(native_request_id)?;
        input["answers"] = Value::Object(response_answers);
        Some(InputFrame(json!({"type": "control_response", "response": {
            "subtype": "success", "request_id": native_request_id, "response": {
                "behavior": "allow", "updatedInput": input
            }
        }})))
    }
}

impl EventDecoder {
    fn decode_user_ask(&mut self, frame: &Value) -> Vec<DecodedEvent> {
        let unsupported = || {
            vec![DecodedEvent::WriteStdin(InputFrame(json!({
                "type": "control_response", "response": {
                    "subtype": "error", "request_id": frame["request_id"],
                    "error": "Nexus 无法解析此用户问题请求。"
                }
            })))]
        };
        let Some(id) = frame
            .get("request_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            return unsupported();
        };
        let input = frame
            .pointer("/request/input")
            .cloned()
            .unwrap_or(Value::Null);
        let Some(raw_questions) = input.get("questions").and_then(Value::as_array) else {
            return unsupported();
        };
        let questions = raw_questions
            .iter()
            .filter_map(|question| {
                let prompt = question.get("question")?.as_str()?.to_owned();
                let options = question
                    .get("options")
                    .and_then(Value::as_array)
                    .map(|options| {
                        options
                            .iter()
                            .filter_map(|option| {
                                let label = option.get("label")?.as_str()?.to_owned();
                                Some(UserAskOption {
                                    id: label.clone(),
                                    label,
                                    description: option
                                        .get("description")
                                        .and_then(Value::as_str)
                                        .map(str::to_owned),
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                Some(UserAskQuestion {
                    id: prompt.clone(),
                    prompt,
                    answer_mode: if options.is_empty() {
                        UserAskAnswerMode::Text
                    } else {
                        UserAskAnswerMode::Choice {
                            multiple: question
                                .get("multiSelect")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                            allow_custom: true,
                        }
                    },
                    options,
                })
            })
            .collect::<Vec<_>>();
        if questions.len() != raw_questions.len() || questions.is_empty() {
            return unsupported();
        }
        self.pending_user_asks.insert(id.into(), input);
        vec![DecodedEvent::UserAskRequested(UserAskRequest {
            native_request_id: id.into(),
            questions,
            timeout_ms: None,
            resolve_on_send: true,
        })]
    }
}

fn decode_frame(frame: &Value) -> Vec<DecodedEvent> {
    match frame.get("type").and_then(Value::as_str) {
        Some("stream_event") => decode_stream_event(frame),
        Some("assistant") => decode_assistant(frame),
        Some("user") => {
            let mut events = decode_tool_results(frame);
            if events.is_empty()
                && let Some(id) = frame.get("uuid").and_then(Value::as_str)
            {
                events.push(DecodedEvent::InputAccepted(id.into()));
            }
            events
        }
        Some("system") => {
            let mut events = Vec::new();
            if let Some(subtype) = frame.get("subtype").and_then(Value::as_str) {
                if subtype == "init"
                    && let Some(session_id) = frame.get("session_id").and_then(Value::as_str)
                    && !session_id.is_empty()
                {
                    events.push(DecodedEvent::SessionStarted(session_id.to_owned()));
                }
                events.push(DecodedEvent::Status(format!("Claude: {subtype}")));
            }
            events
        }
        Some("result") => {
            let mut events = Vec::new();
            if frame.get("is_error").and_then(Value::as_bool) == Some(true) {
                events.push(DecodedEvent::Error(
                    frame
                        .get("errors")
                        .map(tool_content)
                        .unwrap_or_else(|| "Claude 轮次执行失败。".into()),
                ));
            }
            events.push(DecodedEvent::TurnCompleted);
            events
        }
        Some("control_request") => {
            let request_id = &frame["request_id"];
            if request_id.as_str().is_some_and(|id| !id.is_empty())
                && frame.pointer("/request/subtype").and_then(Value::as_str) == Some("can_use_tool")
            {
                let response = |value| {
                    InputFrame(json!({"type": "control_response", "response": {
                        "subtype": "success", "request_id": request_id, "response": value
                    }}))
                };
                let deny =
                    response(json!({"behavior": "deny", "message": "User denied this action."}));
                return vec![DecodedEvent::ApprovalRequested(ApprovalPrompt {
                    id: request_id.to_string(),
                    title: frame
                        .pointer("/request/tool_name")
                        .and_then(Value::as_str)
                        .unwrap_or("Claude Code")
                        .into(),
                    details: serde_json::to_string_pretty(&frame["request"]["input"])
                        .unwrap_or_default(),
                    options: vec![
                        ApprovalOption {
                            label: "Approve".into(),
                            response: response(json!({
                                "behavior": "allow", "updatedInput": frame["request"]["input"]
                            })),
                        },
                        ApprovalOption {
                            label: "Deny".into(),
                            response: deny.clone(),
                        },
                    ],
                    cancel: deny,
                    timeout_ms: None,
                })];
            }
            let response = json!({"subtype": "error", "request_id": request_id, "error": "Nexus 不支持此控制请求。"});
            vec![DecodedEvent::WriteStdin(InputFrame(
                json!({"type": "control_response", "response": response}),
            ))]
        }
        Some("control_cancel_request") => vec![DecodedEvent::ApprovalResolved(
            frame["request_id"].to_string(),
        )],
        _ => Vec::new(),
    }
}

fn decode_stream_event(frame: &Value) -> Vec<DecodedEvent> {
    let Some(delta) = frame.get("event").and_then(|event| event.get("delta")) else {
        return Vec::new();
    };
    if delta.get("type").and_then(Value::as_str) == Some("text_delta") {
        return delta
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(|text| vec![DecodedEvent::TextDelta(text.to_owned())])
            .unwrap_or_default();
    }
    Vec::new()
}

fn decode_assistant(frame: &Value) -> Vec<DecodedEvent> {
    let Some(content) = frame
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    let mut events = Vec::new();
    let mut completed_text = String::new();
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    completed_text.push_str(text);
                }
            }
            Some("tool_use") => {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned();
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Tool")
                    .to_owned();
                let summary = block.get("input").map(tool_content).unwrap_or_default();
                events.push(DecodedEvent::ToolStarted { id, name, summary });
            }
            _ => {}
        }
    }
    if !completed_text.is_empty() {
        events.push(DecodedEvent::MessageCompleted(completed_text));
    }
    events
}

fn decode_tool_results(frame: &Value) -> Vec<DecodedEvent> {
    frame
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        .map(|block| DecodedEvent::ToolCompleted {
            id: block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
            output: block.get("content").map(tool_content).unwrap_or_default(),
            is_error: block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn catalog_exposes_aliases_without_claiming_unknown_capabilities() {
        let executable = std::env::current_exe().unwrap();
        let (cancel, receiver) = watch::channel(false);
        let models = discover_models(
            executable.to_str().unwrap(),
            Path::new("."),
            &[],
            receiver.clone(),
        )
        .await
        .unwrap();
        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["sonnet", "opus", "haiku"]
        );
        for model in models {
            assert_eq!(model.source.harness(), HarnessKind::Claude);
            assert_eq!(model.availability, ModelAvailability::Unknown);
            assert!(model.supported_reasoning_efforts.is_empty());
            assert!(model.default_reasoning_effort.is_none());
        }
        cancel.send_replace(true);
        assert_eq!(
            discover_models("unused", Path::new("."), &[], receiver).await,
            Err(ModelCatalogError::Cancelled)
        );
        assert!(matches!(
            discover_models(
                "nexus-missing-claude",
                Path::new("."),
                &[],
                watch::channel(false).1
            )
            .await,
            Err(ModelCatalogError::Failed(_))
        ));
    }

    #[test]
    fn launch_spec_includes_model_and_effort_without_prompt_in_argv() {
        for (mode, value) in [
            (PermissionMode::Ask, "default"),
            (PermissionMode::AutoEdit, "acceptEdits"),
            (PermissionMode::Yolo, "bypassPermissions"),
        ] {
            let spec = build_launch_spec(
                "claude",
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
                    .any(|pair| pair == ["--permission-mode", value])
            );
            assert!(
                spec.args
                    .windows(2)
                    .any(|pair| pair == ["--permission-prompt-tool", "stdio"])
            );
            assert_eq!(
                spec.args
                    .iter()
                    .any(|arg| arg == "--allow-dangerously-skip-permissions"),
                mode == PermissionMode::Yolo
            );
        }
        let defaults = build_launch_spec(
            "claude",
            Path::new("."),
            "test",
            None,
            ThinkingEffort::Default,
            None,
            PermissionMode::AutoEdit,
        );
        assert!(
            !defaults
                .args
                .iter()
                .any(|arg| arg == "--model" || arg == "--effort")
        );
        let spec = build_launch_spec(
            "/usr/local/bin/claude",
            Path::new("/tmp/project"),
            "secret prompt",
            Some("opus"),
            ThinkingEffort::XHigh,
            None,
            PermissionMode::AutoEdit,
        );
        assert!(spec.args.windows(2).any(|pair| pair == ["--model", "opus"]));
        assert!(
            spec.args
                .windows(2)
                .any(|pair| pair == ["--effort", "xhigh"])
        );
        assert!(!spec.args.iter().any(|arg| arg.contains("secret prompt")));
        assert!(
            spec.args
                .windows(2)
                .any(|pair| pair == ["--input-format", "stream-json"])
        );
        assert!(spec.args.iter().any(|arg| arg == "--replay-user-messages"));
        assert_eq!(
            serde_json::from_str::<Value>(&spec.stdin).unwrap()["message"]["content"],
            "secret prompt"
        );
        assert!(
            !spec
                .args
                .iter()
                .any(|arg| arg == "--no-session-persistence" || arg == "--resume")
        );
        let resumed = build_launch_spec(
            "claude",
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
                .any(|pair| pair == ["--permission-mode", "acceptEdits"])
        );
        assert!(
            !resumed
                .args
                .iter()
                .any(|arg| arg == "--no-session-persistence" || arg == "--fork-session")
        );
        assert_eq!(
            serde_json::from_str::<Value>(&resumed.stdin).unwrap()["message"]["content"],
            "follow-up"
        );
    }

    #[test]
    fn tool_approval_waits_for_user_and_preserves_input_and_cancellation() {
        let mut decoder = EventDecoder::default();
        let frame = json!({"type": "control_request", "request_id": "approval-1", "request": {
            "subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "echo \"审批\"", "timeout": 1000}
        }});
        let events = decoder.decode_line(&frame.to_string()).unwrap();
        let [DecodedEvent::ApprovalRequested(prompt)] = events.as_slice() else {
            panic!("expected approval")
        };
        assert!(prompt.details.contains("审批"));
        assert_eq!(
            prompt.options[0].response.0["response"]["response"]["updatedInput"],
            frame["request"]["input"]
        );
        assert_eq!(
            prompt.options[0].response.0["response"]["request_id"],
            "approval-1"
        );
        assert_eq!(
            prompt.options[1].response.0["response"]["response"]["behavior"],
            "deny"
        );
        assert_eq!(prompt.cancel, prompt.options[1].response);
        assert_eq!(
            decoder
                .decode_line(r#"{"type":"control_cancel_request","request_id":"approval-1"}"#)
                .unwrap(),
            vec![DecodedEvent::ApprovalResolved(prompt.id.clone())]
        );
    }

    #[test]
    fn ask_user_question_maps_questions_and_builds_sdk_response() {
        let mut decoder = EventDecoder::default();
        let input = json!({
            "questions": [
                {"question": "Which features?", "header": "Features", "multiSelect": true,
                 "options": [
                    {"label": "Fast", "description": "Optimize latency"},
                    {"label": "Safe", "description": "Prefer checks"}
                 ]},
                {"question": "Anything else?", "header": "Notes"}
            ],
            "metadata": {"preserve": true}
        });
        let frame = json!({"type": "control_request", "request_id": "ask-1", "request": {
            "subtype": "can_use_tool", "tool_name": "AskUserQuestion", "input": input
        }});
        let events = decoder.decode_line(&frame.to_string()).unwrap();
        let [DecodedEvent::UserAskRequested(request)] = events.as_slice() else {
            panic!("expected user ask")
        };
        assert_eq!(request.native_request_id, "ask-1");
        assert_eq!(request.timeout_ms, None);
        assert!(request.resolve_on_send);
        assert_eq!(request.questions[0].id, "Which features?");
        assert_eq!(request.questions[0].prompt, "Which features?");
        assert_eq!(
            request.questions[0].answer_mode,
            UserAskAnswerMode::Choice {
                multiple: true,
                allow_custom: true
            }
        );
        assert_eq!(request.questions[0].options[0].id, "Fast");
        assert_eq!(request.questions[0].options[0].label, "Fast");
        assert_eq!(
            request.questions[0].options[0].description.as_deref(),
            Some("Optimize latency")
        );
        assert_eq!(request.questions[1].id, "Anything else?");
        assert_eq!(request.questions[1].answer_mode, UserAskAnswerMode::Text);

        let response = decoder
            .answer_user_ask(
                "ask-1",
                &[
                    UserAskAnswer {
                        question_id: "Which features?".into(),
                        value: UserAskAnswerValue::Selected(vec!["Fast".into(), "Safe".into()]),
                    },
                    UserAskAnswer {
                        question_id: "Anything else?".into(),
                        value: UserAskAnswerValue::Text("custom text".into()),
                    },
                ],
            )
            .unwrap();
        assert_eq!(response.0["response"]["response"]["behavior"], "allow");
        let mut expected_input = input;
        expected_input["answers"] = json!({
            "Which features?": "Fast, Safe", "Anything else?": "custom text"
        });
        assert_eq!(
            response.0["response"]["response"]["updatedInput"],
            expected_input
        );
        assert_eq!(
            response.0["response"]["response"]["updatedInput"]["answers"],
            json!({
                "Which features?": "Fast, Safe", "Anything else?": "custom text"
            })
        );
        assert!(decoder.answer_user_ask("ask-1", &[]).is_none());
    }

    #[test]
    fn ask_user_question_cancel_clears_pending_and_rejects_late_answer() {
        let mut decoder = EventDecoder::default();
        let frame = json!({"type": "control_request", "request_id": "ask-cancel", "request": {
            "subtype": "can_use_tool", "tool_name": "AskUserQuestion",
            "input": {"questions": [{"question": "Continue?", "options": [{"label": "Yes"}]}]}
        }});
        assert!(matches!(
            decoder.decode_line(&frame.to_string()).unwrap().as_slice(),
            [DecodedEvent::UserAskRequested(_)]
        ));
        assert_eq!(
            decoder
                .decode_line(r#"{"type":"control_cancel_request","request_id":"ask-cancel"}"#)
                .unwrap(),
            vec![DecodedEvent::UserAskFinished {
                native_request_id: "ask-cancel".into(),
                status: UserAskStatus::Cancelled,
                message: None,
            }]
        );
        assert!(
            decoder
                .answer_user_ask(
                    "ask-cancel",
                    &[UserAskAnswer {
                        question_id: "Continue?".into(),
                        value: UserAskAnswerValue::Selected(vec!["Yes".into()]),
                    }]
                )
                .is_none()
        );
    }

    #[test]
    fn title_launch_spec_disables_tools_and_edit_approval() {
        let spec = build_title_launch_spec(
            "/usr/local/bin/claude",
            Path::new("/tmp/project"),
            "title prompt",
            Some("sonnet"),
            ThinkingEffort::Low,
        );

        assert!(
            spec.args
                .windows(2)
                .any(|pair| pair == ["--permission-mode", "dontAsk"])
        );
        assert!(spec.args.windows(2).any(|pair| pair == ["--tools", ""]));
        assert!(!spec.args.iter().any(|arg| arg == "acceptEdits"));
        assert!(!spec.args.iter().any(|arg| arg.contains("title prompt")));
    }

    #[test]
    fn decoder_maps_text_and_tool_events() {
        let mut decoder = EventDecoder::default();
        assert_eq!(
            decoder
                .decode_line(
                    r#"{"type":"system","subtype":"init","session_id":"existing-session"}"#
                )
                .unwrap(),
            vec![
                DecodedEvent::SessionStarted("existing-session".into()),
                DecodedEvent::Status("Claude: init".into())
            ]
        );
        let delta = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"你好"}}}"#;
        assert_eq!(
            decoder.decode_line(delta).unwrap(),
            vec![DecodedEvent::TextDelta("你好".into())]
        );

        let tool = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"README.md"}}]}}"#;
        assert!(matches!(
            decoder.decode_line(tool).unwrap().as_slice(),
            [DecodedEvent::ToolStarted { id, name, .. }] if id == "t1" && name == "Read"
        ));

        let code = "let value = \"完整内容\";\n".repeat(100);
        let input = serde_json::json!({"file_path": "src/main.rs", "content": code});
        let tool = serde_json::json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": "write-1", "name": "Write", "input": input}
        ]}});
        assert!(matches!(
            decoder.decode_line(&tool.to_string()).unwrap().as_slice(),
            [DecodedEvent::ToolStarted { summary, .. }]
                if serde_json::from_str::<Value>(summary).unwrap() == input
        ));
        let result = serde_json::json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "write-1", "content": code, "is_error": true}
        ]}});
        assert!(matches!(
            decoder.decode_line(&result.to_string()).unwrap().as_slice(),
            [DecodedEvent::ToolCompleted { id, output, is_error: true }]
                if id == "write-1" && output == &code
        ));
    }

    #[test]
    fn malformed_frames_are_recoverable() {
        let mut decoder = EventDecoder::default();
        assert!(decoder.decode_line("not json").is_err());
        assert_eq!(
            decoder
                .decode_line(r#"{"type":"result","result":"done"}"#)
                .unwrap(),
            vec![DecodedEvent::TurnCompleted]
        );
    }

    #[test]
    fn steering_replays_the_message_id_and_reports_failed_turns() {
        let mut decoder = EventDecoder::default();
        let frame = decoder
            .steer("message-id", "new instruction\n第二行")
            .unwrap();
        assert_eq!(frame.0["uuid"], "message-id");
        assert_eq!(frame.0["message"]["content"], "new instruction\n第二行");
        assert_eq!(
            decoder.decode_line(&frame.0.to_string()).unwrap(),
            vec![DecodedEvent::InputAccepted("message-id".into())]
        );
        assert!(
            matches!(decoder.decode_line(r#"{"type":"result","is_error":true,"errors":["denied"]}"#).unwrap().as_slice(),
            [DecodedEvent::Error(message), DecodedEvent::TurnCompleted] if message.contains("denied"))
        );
    }
}
