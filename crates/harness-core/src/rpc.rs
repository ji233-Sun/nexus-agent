use crate::{
    ApprovalOption, ApprovalPrompt, DecodedEvent, InputFrame, LineDecoder, UserAskRequest,
    summarize_text, tool_content,
};
use nexus_domain::{
    UserAskAnswer, UserAskAnswerMode, UserAskAnswerValue, UserAskOption, UserAskQuestion,
    UserAskStatus,
};
use serde_json::{Value, json};
use std::collections::HashMap;

pub struct RpcEventDecoder {
    name: &'static str,
    pending_user_asks: HashMap<String, Vec<ApprovalOption>>,
}

impl RpcEventDecoder {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            pending_user_asks: HashMap::new(),
        }
    }
}

impl LineDecoder for RpcEventDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let frame: Value = serde_json::from_str(line)?;
        let events = if frame["type"] == "extension_ui_request" {
            self.decode_ui_request(&frame)
        } else {
            decode_frame(&frame, self.name)
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

fn decode_frame(frame: &Value, name: &str) -> Vec<DecodedEvent> {
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
                    .unwrap_or("RPC 请求失败。")
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
                        DecodedEvent::Error(format!("{name} 未返回会话 ID。")),
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
        Some("agent_start") => vec![DecodedEvent::Status(format!("{name} 会话已启动"))],
        Some("turn_start") => vec![DecodedEvent::Status(format!("{name} 正在处理任务…"))],
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
                    DecodedEvent::Status(format!("{name}: {}", summarize_text(message)))
                }
            })
            .into_iter()
            .collect(),
        Some("auto_retry_start") => vec![DecodedEvent::Status(format!("{name} 正在重试请求…"))],
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

impl RpcEventDecoder {
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
                title: self.name.into(),
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
                self.name.into()
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
