use super::*;

pub struct EventDecoder {
    harness: HarnessKind,
    pending_user_asks: HashMap<String, Value>,
    user_message_ids: HashSet<String>,
}

impl Default for EventDecoder {
    fn default() -> Self {
        Self::for_harness(HarnessKind::Claude)
    }
}

impl EventDecoder {
    pub fn for_harness(harness: HarnessKind) -> Self {
        Self {
            harness,
            pending_user_asks: HashMap::new(),
            user_message_ids: HashSet::new(),
        }
    }

    pub fn for_run(request: &StartRun) -> Self {
        let mut decoder = Self::for_harness(request.harness);
        decoder.user_message_ids.insert(request.run_id.to_string());
        decoder
    }
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
            let mut message_ids = frame
                .get("user_message_uuid")
                .and_then(Value::as_str)
                .into_iter()
                .chain(
                    frame
                        .get("user_message_uuids")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str),
                );
            let has_message_id = message_ids.clone().next().is_some();
            let answers_user = message_ids.any(|id| self.user_message_ids.contains(id));
            // Resume can finish a background notification before consuming the
            // prompt. A synthetic turn may also fold in our prompt or Steer.
            if !self.user_message_ids.is_empty()
                && !answers_user
                && (frame.pointer("/origin/kind").and_then(Value::as_str)
                    == Some("task-notification")
                    || has_message_id)
            {
                return Ok(Vec::new());
            }
            self.pending_user_asks.clear();
        }
        Ok(decode_frame(&frame, self.harness))
    }

    fn steer(&mut self, message_id: &str, prompt: &str) -> Option<InputFrame> {
        self.user_message_ids.insert(message_id.to_owned());
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

fn decode_frame(frame: &Value, harness: HarnessKind) -> Vec<DecodedEvent> {
    let name = if harness == HarnessKind::Claude {
        "Claude".to_owned()
    } else {
        harness.to_string()
    };
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
                events.push(DecodedEvent::Status(format!("{name}: {subtype}")));
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
                        .unwrap_or_else(|| format!("{name} 轮次执行失败。")),
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
                        .map(str::to_owned)
                        .unwrap_or_else(|| harness.to_string()),
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
