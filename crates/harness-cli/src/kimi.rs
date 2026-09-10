use nexus_domain::{
    PermissionMode, ThinkingEffort, UserAskAnswer, UserAskAnswerMode, UserAskAnswerValue,
    UserAskOption, UserAskQuestion,
};
use nexus_harness_core::{
    ApprovalOption, ApprovalPrompt, DecodedEvent, InputFrame, LaunchSpec, LineDecoder,
    UserAskRequest, tool_content,
};
use nexus_protocol::StartRun;
use serde_json::{Value, json};
use std::{collections::HashMap, path::Path};
use uuid::Uuid;
fn request(id: &str, method: &str, params: Value) -> InputFrame {
    InputFrame(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
}
pub(super) fn prepare(run: &StartRun, cwd: &Path) -> (LaunchSpec, Decoder) {
    let session = run
        .session_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let mut args = vec!["--wire".into(), "--session".into(), session.clone()];
    if let Some(model) = &run.model {
        let (id, thinking) = model
            .strip_suffix(",thinking")
            .map(|m| (m, true))
            .unwrap_or((model, false));
        args.extend([
            "--model".into(),
            id.into(),
            if thinking {
                "--thinking"
            } else {
                "--no-thinking"
            }
            .into(),
        ]);
    } else if matches!(run.effort, ThinkingEffort::Off | ThinkingEffort::None) {
        args.push("--no-thinking".into());
    }
    let initialize = request(
        "initialize",
        "initialize",
        json!({"protocol_version":"1.10","client":{"name":"nexus"},"capabilities":{"supports_question":true}}),
    );
    (
        LaunchSpec {
            executable: run.executable.clone().into(),
            cwd: cwd.into(),
            args,
            stdin: format!("{}\n", initialize.0),
        },
        Decoder {
            session,
            prompt: Some(run.prompt.clone()),
            text: String::new(),
            permission: run.permission_mode,
            questions: HashMap::new(),
        },
    )
}
pub(super) struct Decoder {
    session: String,
    prompt: Option<String>,
    text: String,
    permission: PermissionMode,
    questions: HashMap<String, (String, Vec<UserAskQuestion>)>,
}
impl Decoder {
    fn flush(&mut self, events: &mut Vec<DecodedEvent>) {
        if !self.text.is_empty() {
            events.push(DecodedEvent::MessageCompleted(std::mem::take(
                &mut self.text,
            )));
        }
    }
}
impl LineDecoder for Decoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let frame: Value = serde_json::from_str(line)?;
        let id = frame["id"].as_str().unwrap_or("");
        if frame.get("error").is_some() {
            let message = "Kimi CLI 请求失败，请检查模型、认证与原生日志。".to_owned();
            return Ok(if id != "initialize" && id != "prompt" {
                vec![DecodedEvent::InputRejected {
                    id: id.into(),
                    message,
                }]
            } else {
                vec![DecodedEvent::Error(message), DecodedEvent::TurnCompleted]
            });
        }
        if frame.get("result").is_some() {
            return Ok(match id {
                "initialize" => {
                    if !frame["result"]["protocol_version"]
                        .as_str()
                        .is_some_and(|v| v.starts_with("1."))
                    {
                        return Ok(vec![
                            DecodedEvent::Error("不支持的 Kimi Wire 协议版本。".into()),
                            DecodedEvent::TurnCompleted,
                        ]);
                    }
                    self.prompt
                        .take()
                        .map(|prompt| {
                            vec![
                                DecodedEvent::SessionStarted(self.session.clone()),
                                DecodedEvent::WriteStdin(request(
                                    "prompt",
                                    "prompt",
                                    json!({"user_input":prompt}),
                                )),
                            ]
                        })
                        .unwrap_or_default()
                }
                "prompt" => {
                    let mut events = vec![];
                    self.flush(&mut events);
                    if frame["result"]["status"] != "finished" {
                        events.push(DecodedEvent::Error(format!(
                            "Kimi CLI 提前结束：{}",
                            frame["result"]["status"]
                        )));
                    }
                    events.push(DecodedEvent::TurnCompleted);
                    events
                }
                _ => vec![DecodedEvent::InputAccepted(id.into())],
            });
        }
        let payload = &frame["params"]["payload"];
        let kind = frame["params"]["type"].as_str().unwrap_or("");
        let mut events = vec![];
        if frame["method"] == "event" {
            match kind {
                "ContentPart" if payload["type"] == "text" => {
                    if let Some(text) = payload["text"].as_str() {
                        self.text.push_str(text);
                        events.push(DecodedEvent::TextDelta(text.into()));
                    }
                }
                "ToolCall" => {
                    self.flush(&mut events);
                    events.push(DecodedEvent::ToolStarted {
                        id: payload["id"].as_str().unwrap_or("").into(),
                        name: payload["function"]["name"]
                            .as_str()
                            .unwrap_or("Tool")
                            .into(),
                        summary: payload["function"]["arguments"]
                            .as_str()
                            .unwrap_or("")
                            .into(),
                    });
                }
                "ToolResult" => events.push(DecodedEvent::ToolCompleted {
                    id: payload["tool_call_id"].as_str().unwrap_or("").into(),
                    output: tool_content(&payload["return_value"]["output"]),
                    is_error: payload["return_value"]["is_error"] == true,
                }),
                "ApprovalResponse" => events.push(DecodedEvent::ApprovalResolved(
                    payload["request_id"].as_str().unwrap_or("").into(),
                )),
                _ => {}
            }
        } else if frame["method"] == "request" {
            let response = |result: Value| {
                InputFrame(json!({"jsonrpc":"2.0","id":frame["id"],"result":result}))
            };
            match kind {
                "ApprovalRequest" => {
                    let choice = |value: &str| response(json!({"request_id":payload["id"],"response":value}));
                    let edit = matches!(payload["sender"].as_str(), Some("WriteFile" | "StrReplaceFile"));
                    if self.permission == PermissionMode::Yolo
                        || (self.permission == PermissionMode::AutoEdit && edit)
                    {
                        events.push(DecodedEvent::WriteStdin(choice("approve")));
                    } else {
                        events.push(DecodedEvent::ApprovalRequested(ApprovalPrompt {
                            id: payload["id"].as_str().unwrap_or(id).into(),
                            title: "Kimi Code".into(),
                            details: tool_content(payload),
                            options: [("Approve", "approve"), ("Deny", "reject")]
                                .into_iter()
                                .map(|(label, value)| ApprovalOption { label: label.into(), response: choice(value) })
                                .collect(),
                            cancel: choice("reject"),
                            timeout_ms: None,
                        }));
                    }
                }
                "QuestionRequest" => {
                    let questions: Vec<_> = payload["questions"]
                        .as_array().into_iter().flatten().enumerate()
                        .filter_map(|(index, question)| Some(UserAskQuestion {
                            id: index.to_string(),
                            prompt: question["question"].as_str()?.into(),
                            answer_mode: UserAskAnswerMode::Choice {
                                multiple: question["multi_select"] == true,
                                allow_custom: true,
                            },
                            options: question["options"].as_array()?.iter().enumerate()
                                .filter_map(|(index, option)| Some(UserAskOption {
                                    id: index.to_string(),
                                    label: option["label"].as_str()?.into(),
                                    description: option["description"].as_str().map(Into::into),
                                })).collect(),
                        })).collect();
                    self.questions.insert(
                        id.into(),
                        (payload["id"].as_str().unwrap_or(id).into(), questions.clone()),
                    );
                    events.push(DecodedEvent::UserAskRequested(UserAskRequest {
                        native_request_id: id.into(),
                        questions,
                        timeout_ms: None,
                        resolve_on_send: true,
                    }));
                }
                _=>events.push(DecodedEvent::WriteStdin(InputFrame(json!({"jsonrpc":"2.0","id":frame["id"],"error":{"code":-32601,"message":"Unsupported request"}}))))
            }
        }
        Ok(events)
    }
    fn steer(&mut self, id: &str, prompt: &str) -> Option<InputFrame> {
        Some(request(id, "steer", json!({"user_input":prompt})))
    }
    fn answer_user_ask(&mut self, id: &str, answers: &[UserAskAnswer]) -> Option<InputFrame> {
        let (request_id, questions) = self.questions.get(id)?;
        let mut values = serde_json::Map::new();
        for answer in answers {
            let question = questions.iter().find(|q| q.id == answer.question_id)?;
            let value = match &answer.value {
                UserAskAnswerValue::Text(text) => text.clone(),
                UserAskAnswerValue::Selected(ids) => ids
                    .iter()
                    .map(|id| {
                        question
                            .options
                            .iter()
                            .find(|o| &o.id == id)
                            .map(|o| o.label.clone())
                    })
                    .collect::<Option<Vec<_>>>()?
                    .join(", "),
            };
            values.insert(question.prompt.clone(), json!(value));
        }
        let frame = InputFrame(
            json!({"jsonrpc":"2.0","id":id,"result":{"request_id":request_id,"answers":values}}),
        );
        self.questions.remove(id);
        Some(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn decoder(permission: PermissionMode) -> Decoder {
        Decoder {
            session: "session".into(),
            prompt: Some("hello".into()),
            text: String::new(),
            permission,
            questions: HashMap::new(),
        }
    }
    #[test]
    fn wire_questions_preserve_native_labels_custom_text_and_request_ids() {
        let mut decoder = decoder(PermissionMode::Yolo);
        let frame = json!({"jsonrpc":"2.0","id":"rpc-id","method":"request","params":{"type":"QuestionRequest","payload":{"id":"question-id","questions":[{"question":"Choose","multi_select":true,"options":[{"label":"A"},{"label":"B"}]},{"question":"Details","options":[{"label":"Default"}]}]}}});
        let events = decoder.decode_line(&frame.to_string()).unwrap();
        assert!(matches!(&events[..], [DecodedEvent::UserAskRequested(_)]));
        let answer = decoder
            .answer_user_ask(
                "rpc-id",
                &[
                    UserAskAnswer {
                        question_id: "0".into(),
                        value: UserAskAnswerValue::Selected(vec!["0".into(), "1".into()]),
                    },
                    UserAskAnswer {
                        question_id: "1".into(),
                        value: UserAskAnswerValue::Text("自定义".into()),
                    },
                ],
            )
            .unwrap();
        assert_eq!(
            answer.0,
            json!({"jsonrpc":"2.0","id":"rpc-id","result":{"request_id":"question-id","answers":{"Choose":"A, B","Details":"自定义"}}})
        );
        assert!(decoder.answer_user_ask("rpc-id", &[]).is_none());
    }
    #[test]
    fn wire_approvals_autoedit_only_allows_edit_tools_and_roundtrips_rejection() {
        for (sender, automatic) in [
            ("WriteFile", true),
            ("StrReplaceFile", true),
            ("Shell", false),
        ] {
            let events = decoder(PermissionMode::AutoEdit).decode_line(&json!({"id":"rpc-id","method":"request","params":{"type":"ApprovalRequest","payload":{"id":"approval-id","sender":sender,"description":"run command"}}}).to_string()).unwrap();
            if automatic {
                assert!(
                    matches!(&events[..],[DecodedEvent::WriteStdin(frame)] if frame.0["result"]["response"]=="approve")
                );
            } else {
                let [DecodedEvent::ApprovalRequested(approval)] = &events[..] else {
                    panic!()
                };
                assert_eq!(
                    approval.cancel.0,
                    json!({"jsonrpc":"2.0","id":"rpc-id","result":{"request_id":"approval-id","response":"reject"}})
                );
            }
        }
    }
    #[test]
    fn wire_stream_and_steer_receipts_finish_without_losing_text() {
        let mut decoder = decoder(PermissionMode::Ask);
        let events = decoder
            .decode_line(r#"{"id":"initialize","result":{"protocol_version":"1.10"}}"#)
            .unwrap();
        assert!(
            matches!(&events[..],[DecodedEvent::SessionStarted(id),DecodedEvent::WriteStdin(_)] if id=="session")
        );
        let events=decoder.decode_line(r#"{"method":"event","params":{"type":"ContentPart","payload":{"type":"text","text":"Hello"}}}"#).unwrap();
        assert!(matches!(&events[..],[DecodedEvent::TextDelta(text)] if text=="Hello"));
        assert_eq!(
            decoder.steer("steer-id", "more").unwrap().0["method"],
            "steer"
        );
        assert!(
            matches!(&decoder.decode_line(r#"{"id":"steer-id","result":{"status":"steered"}}"#).unwrap()[..],[DecodedEvent::InputAccepted(id)] if id=="steer-id")
        );
        assert!(
            matches!(&decoder.decode_line(r#"{"id":"prompt","result":{"status":"finished"}}"#).unwrap()[..],[DecodedEvent::MessageCompleted(text),DecodedEvent::TurnCompleted] if text=="Hello")
        );
        assert!(matches!(
            &decoder
                .decode_line(r#"{"id":"prompt","error":{"code":-1}}"#)
                .unwrap()[..],
            [DecodedEvent::Error(_), DecodedEvent::TurnCompleted]
        ));
    }
}
