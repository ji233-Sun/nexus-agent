use super::*;

#[derive(Default)]
pub struct TitleEventDecoder;

impl LineDecoder for TitleEventDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let frame: Value = serde_json::from_str(line)?;
        let events = match frame.get("type").and_then(Value::as_str) {
            Some("item.completed")
                if frame.pointer("/item/type").and_then(Value::as_str) == Some("agent_message") =>
            {
                frame
                    .pointer("/item/text")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .map(|text| vec![DecodedEvent::MessageCompleted(text.into())])
                    .unwrap_or_default()
            }
            Some("turn.failed") => frame
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(|message| vec![DecodedEvent::Error(summarize_text(message))])
                .unwrap_or_default(),
            Some("error") => frame
                .get("message")
                .and_then(Value::as_str)
                .map(|message| vec![DecodedEvent::Error(summarize_text(message))])
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        Ok(events)
    }

    fn steer(&mut self, _message_id: &str, _prompt: &str) -> Option<InputFrame> {
        None
    }
}

pub struct EventDecoder {
    request: StartRun,
    cwd: PathBuf,
    api_key: Option<EnvironmentVariable>,
    thread_id: Option<String>,
    turn_id: Option<String>,
    approval_items: HashMap<String, Value>,
    pending_user_asks: HashMap<String, PendingUserAsk>,
    seen_async_asks: HashSet<String>,
}

struct PendingUserAsk {
    reply: UserAskReply,
    questions: Vec<UserAskQuestion>,
}

enum UserAskReply {
    Rpc(Value),
    Async { submitted: bool },
}

impl LineDecoder for EventDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let frame: Value = serde_json::from_str(line)?;
        Ok(self.decode_frame(&frame))
    }

    fn steer(&mut self, message_id: &str, prompt: &str) -> Option<InputFrame> {
        Some(InputFrame(
            json!({"id": message_id, "method": "turn/steer", "params": {
                "threadId": self.thread_id.as_ref()?, "expectedTurnId": self.turn_id.as_ref()?,
                "clientUserMessageId": message_id,
                "input": [{"type": "text", "text": prompt}]
            }}),
        ))
    }

    fn answer_user_ask(
        &mut self,
        native_request_id: &str,
        answers: &[UserAskAnswer],
    ) -> Option<InputFrame> {
        let pending = self.pending_user_asks.get(native_request_id)?;
        if answers.len() != pending.questions.len() {
            return None;
        }
        let mut encoded = Map::new();
        for question in &pending.questions {
            let answer = answers
                .iter()
                .find(|answer| answer.question_id == question.id)?;
            let values = match (&question.answer_mode, &answer.value) {
                (UserAskAnswerMode::Text, UserAskAnswerValue::Text(value)) => {
                    vec![Value::String(value.clone())]
                }
                (
                    UserAskAnswerMode::Choice { allow_custom, .. },
                    UserAskAnswerValue::Text(value),
                ) if *allow_custom => vec![Value::String(value.clone())],
                (UserAskAnswerMode::Choice { .. }, UserAskAnswerValue::Selected(values))
                    if values.len() == 1
                        && values.iter().all(|value| {
                            question.options.iter().any(|option| option.id == *value)
                        }) =>
                {
                    values
                        .iter()
                        .map(|value| {
                            Value::String(
                                question
                                    .options
                                    .iter()
                                    .find(|option| option.id == *value)
                                    .unwrap()
                                    .label
                                    .clone(),
                            )
                        })
                        .collect()
                }
                _ => return None,
            };
            encoded.insert(question.id.clone(), json!({"answers": values}));
        }
        match &pending.reply {
            UserAskReply::Rpc(id) => {
                let frame = InputFrame(json!({"id": id, "result": {"answers": encoded}}));
                self.pending_user_asks.remove(native_request_id);
                Some(frame)
            }
            UserAskReply::Async { submitted: true } => None,
            UserAskReply::Async { submitted: false } => {
                let text = pending
                    .questions
                    .iter()
                    .map(|question| {
                        format!(
                            "{}\n{}",
                            question.prompt,
                            encoded[&question.id]["answers"][0].as_str().unwrap()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                // turn/start atomically steers an active turn or starts the next one.
                // Async questions receive a new user message, not a JSON-RPC tool result.
                let frame = InputFrame(
                    json!({"id": native_request_id, "method": "turn/start", "params": {
                        "threadId": self.thread_id.as_ref()?,
                        "input": [{"type": "text", "text": format!("User Ask answers:\n\n{text}")}]
                    }}),
                );
                self.pending_user_asks.get_mut(native_request_id)?.reply =
                    UserAskReply::Async { submitted: true };
                Some(frame)
            }
        }
    }
}

impl EventDecoder {
    pub(super) fn new(
        request: &StartRun,
        cwd: &Path,
        api_key: Option<EnvironmentVariable>,
    ) -> Self {
        Self {
            request: request.clone(),
            cwd: cwd.to_path_buf(),
            api_key,
            thread_id: None,
            turn_id: None,
            approval_items: HashMap::new(),
            pending_user_asks: HashMap::new(),
            seen_async_asks: HashSet::new(),
        }
    }

    fn start_thread(&self) -> DecodedEvent {
        let (sandbox, approval) = match self.request.permission_mode {
            PermissionMode::Ask => ("read-only", "on-request"),
            PermissionMode::AutoEdit => ("workspace-write", "on-request"),
            PermissionMode::Yolo => ("danger-full-access", "never"),
        };
        let mut params = json!({"cwd": self.cwd, "approvalPolicy": approval, "sandbox": sandbox});
        if let Some(model) = &self.request.model {
            params["model"] = model.clone().into();
        }
        let method = if let Some(id) = &self.request.session_id {
            params["threadId"] = id.clone().into();
            "thread/resume"
        } else {
            "thread/start"
        };
        write_request(2, method, params)
    }

    fn decode_frame(&mut self, frame: &Value) -> Vec<DecodedEvent> {
        if frame.get("id").is_some() && frame.get("method").is_some() {
            if frame.get("method").and_then(Value::as_str) == Some("item/tool/requestUserInput") {
                return self.decode_user_ask(frame);
            }
            return self.decode_approval(frame);
        }
        if let Some(id) = frame.get("id").and_then(Value::as_str) {
            if self.pending_user_asks.get(id).is_some_and(|pending| {
                matches!(pending.reply, UserAskReply::Async { submitted: true })
            }) {
                self.pending_user_asks.remove(id);
                if frame.get("error").is_none()
                    && let Some(turn_id) = frame
                        .pointer("/result/turn/id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                {
                    self.turn_id = Some(turn_id.into());
                    return vec![DecodedEvent::UserAskFinished {
                        native_request_id: id.into(),
                        status: UserAskStatus::Answered,
                        message: None,
                    }];
                }
                let message = "Codex 未接受 User Ask 回答，请检查会话状态。".to_owned();
                return vec![
                    DecodedEvent::UserAskFinished {
                        native_request_id: id.into(),
                        status: UserAskStatus::Failed,
                        message: Some(message.clone()),
                    },
                    DecodedEvent::Error(message),
                    DecodedEvent::TurnCompleted,
                ];
            }
            return if let Some(message) = frame.pointer("/error/message").and_then(Value::as_str) {
                vec![DecodedEvent::InputRejected {
                    id: id.into(),
                    message: summarize_text(message),
                }]
            } else if self.turn_id.as_deref().is_some_and(|turn_id| {
                frame.pointer("/result/turnId").and_then(Value::as_str) == Some(turn_id)
            }) {
                vec![DecodedEvent::InputAccepted(id.into())]
            } else {
                Vec::new()
            };
        }
        if let Some(id) = frame.get("id").and_then(Value::as_u64) {
            if frame.get("error").is_some() || frame.get("result").is_none() {
                // Authentication failures can echo credentials; never forward the raw response.
                return vec![
                    DecodedEvent::Error(format!(
                        "Codex App Server 请求 {id} 失败，请检查登录与会话配置。"
                    )),
                    DecodedEvent::TurnCompleted,
                ];
            }
            return match id {
                0 => {
                    let mut events = vec![DecodedEvent::WriteStdin(InputFrame(
                        json!({"method": "initialized", "params": {}}),
                    ))];
                    events.push(if let Some(key) = self.api_key.take() {
                        write_request(
                            1,
                            "account/login/start",
                            json!({"type": "apiKey", "apiKey": key.value}),
                        )
                    } else {
                        self.start_thread()
                    });
                    events
                }
                1 => vec![self.start_thread()],
                2 => {
                    let Some(thread_id) = frame
                        .pointer("/result/thread/id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                    else {
                        return vec![
                            DecodedEvent::Error("Codex 未返回会话 ID。".into()),
                            DecodedEvent::TurnCompleted,
                        ];
                    };
                    self.thread_id = Some(thread_id.into());
                    let mut params = json!({"threadId": thread_id,
                        "input": [{"type": "text", "text": self.request.prompt}]});
                    let input = params["input"].as_array_mut().expect("input array");
                    for image in &self.request.attachments {
                        input.push(json!({"type": "text", "text": image.label()}));
                        input.push(json!({"type": "localImage", "path": image.path}));
                    }
                    if !self.request.effort.is_default() {
                        params["effort"] = self.request.effort.as_str().into();
                    }
                    vec![
                        DecodedEvent::SessionStarted(thread_id.into()),
                        write_request(3, "turn/start", params),
                    ]
                }
                3 => {
                    self.turn_id = frame
                        .pointer("/result/turn/id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        .map(str::to_owned);
                    if self.turn_id.is_some() {
                        Vec::new()
                    } else {
                        vec![
                            DecodedEvent::Error("Codex 未返回轮次 ID。".into()),
                            DecodedEvent::TurnCompleted,
                        ]
                    }
                }
                _ => Vec::new(),
            };
        }
        let params = &frame["params"];
        if params
            .get("threadId")
            .and_then(Value::as_str)
            .is_some_and(|id| Some(id) != self.thread_id.as_deref())
            || params
                .get("turnId")
                .or_else(|| params.pointer("/turn/id"))
                .and_then(Value::as_str)
                .is_some_and(|id| self.turn_id.as_deref().is_some_and(|active| active != id))
        {
            return Vec::new();
        }
        match frame.get("method").and_then(Value::as_str) {
            Some("turn/started") => {
                self.turn_id = params
                    .pointer("/turn/id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                vec![DecodedEvent::Status("Codex 正在处理任务…".into())]
            }
            Some("item/started") => {
                let item = &params["item"];
                if matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("commandExecution" | "fileChange")
                ) {
                    self.approval_items.insert(item_id(item), item.clone());
                }
                decode_started_item(item)
            }
            Some("item/completed") => {
                self.approval_items.remove(&item_id(&params["item"]));
                let item = &params["item"];
                if item["type"] == "agentMessage"
                    && item["delivery"] == "async"
                    && item["questions"].is_array()
                {
                    return self.decode_async_user_ask(item);
                }
                decode_completed_item(&params["item"])
            }
            Some("serverRequest/resolved") => {
                let id = params["requestId"].to_string();
                if self.pending_user_asks.remove(&id).is_some() {
                    vec![DecodedEvent::UserAskFinished {
                        native_request_id: id,
                        status: UserAskStatus::Cancelled,
                        message: None,
                    }]
                } else {
                    vec![DecodedEvent::ApprovalResolved(id)]
                }
            }
            Some("item/agentMessage/delta") => params
                .get("delta")
                .and_then(Value::as_str)
                .map(|text| vec![DecodedEvent::TextDelta(text.into())])
                .unwrap_or_default(),
            Some("turn/completed") => {
                self.turn_id = None;
                let mut events = Vec::new();
                if params.pointer("/turn/status").and_then(Value::as_str) != Some("completed") {
                    self.pending_user_asks.clear();
                    events.push(DecodedEvent::Error(
                        params
                            .pointer("/turn/error/message")
                            .and_then(Value::as_str)
                            .map(summarize_text)
                            .unwrap_or_else(|| "Codex 轮次已中断。".into()),
                    ));
                } else {
                    self.pending_user_asks.retain(|id, pending| {
                        if matches!(pending.reply, UserAskReply::Rpc(_)) {
                            events.push(DecodedEvent::UserAskFinished {
                                native_request_id: id.clone(),
                                status: UserAskStatus::Expired,
                                message: None,
                            });
                            false
                        } else {
                            true
                        }
                    });
                }
                if self.pending_user_asks.is_empty() {
                    events.push(DecodedEvent::TurnCompleted);
                } else {
                    // The model can finish while an async question is still awaiting the user.
                    events.push(DecodedEvent::Status("Codex 等待 User Ask 回答…".into()));
                }
                events
            }
            Some("turn/plan/updated") => vec![DecodedEvent::Status(format!(
                "Codex 计划：{}",
                summarize_text(&tool_content(&params["plan"]))
            ))],
            Some("error") if params.get("willRetry").and_then(Value::as_bool) != Some(true) => {
                params
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .map(|message| vec![DecodedEvent::Error(summarize_text(message))])
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    fn decode_async_user_ask(&mut self, item: &Value) -> Vec<DecodedEvent> {
        let Some(id) = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            return decode_completed_item(item);
        };
        let native_request_id = format!("async:{id}");
        if self.seen_async_asks.contains(&native_request_id) {
            return Vec::new();
        }
        let questions = item["questions"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, question)| {
                let prompt = question.get("title")?.as_str()?.to_owned();
                let options = match question.get("options").filter(|options| !options.is_null()) {
                    Some(options) => options
                        .as_array()?
                        .iter()
                        .enumerate()
                        .map(|(index, option)| {
                            Some(UserAskOption {
                                id: index.to_string(),
                                label: option.as_str()?.to_owned(),
                                description: None,
                            })
                        })
                        .collect::<Option<Vec<_>>>()?,
                    None => Vec::new(),
                };
                Some(UserAskQuestion {
                    id: index.to_string(),
                    prompt,
                    answer_mode: if options.is_empty() {
                        UserAskAnswerMode::Text
                    } else {
                        UserAskAnswerMode::Choice {
                            multiple: false,
                            allow_custom: true,
                        }
                    },
                    options,
                })
            })
            .collect::<Option<Vec<_>>>();
        let Some(questions) = questions.filter(|questions| !questions.is_empty()) else {
            return vec![
                DecodedEvent::Error("Codex 返回了无法解析的异步 User Ask 问题。".into()),
                DecodedEvent::TurnCompleted,
            ];
        };
        self.seen_async_asks.insert(native_request_id.clone());
        self.pending_user_asks.insert(
            native_request_id.clone(),
            PendingUserAsk {
                reply: UserAskReply::Async { submitted: false },
                questions: questions.clone(),
            },
        );
        vec![DecodedEvent::UserAskRequested(UserAskRequest {
            native_request_id,
            questions,
            timeout_ms: None,
            resolve_on_send: false,
        })]
    }

    fn decode_user_ask(&mut self, frame: &Value) -> Vec<DecodedEvent> {
        let params = &frame["params"];
        if params.get("threadId").and_then(Value::as_str) != self.thread_id.as_deref()
            || self.thread_id.is_none()
            || params.get("turnId").and_then(Value::as_str) != self.turn_id.as_deref()
            || self.turn_id.is_none()
        {
            return vec![DecodedEvent::WriteStdin(InputFrame(json!({
                "id": frame["id"],
                "error": {"code": -32602, "message": "User input request does not belong to the active turn."}
            })))];
        }
        let Some(raw_questions) = params.get("questions").and_then(Value::as_array) else {
            return vec![DecodedEvent::WriteStdin(InputFrame(json!({
                "id": frame["id"],
                "error": {"code": -32602, "message": "Nexus cannot parse this user input request."}
            })))];
        };
        let questions = raw_questions
            .iter()
            .filter_map(|question| {
                let id = question.get("id")?.as_str()?.to_owned();
                let prompt = question.get("question")?.as_str()?.to_owned();
                let options = question
                    .get("options")
                    .filter(|options| !options.is_null())
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
                    id,
                    prompt,
                    answer_mode: if options.is_empty() {
                        UserAskAnswerMode::Text
                    } else {
                        UserAskAnswerMode::Choice {
                            multiple: false,
                            allow_custom: question
                                .get("isOther")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        }
                    },
                    options,
                })
            })
            .collect::<Vec<_>>();
        if questions.is_empty() || questions.len() != raw_questions.len() {
            return vec![DecodedEvent::WriteStdin(InputFrame(json!({
                "id": frame["id"],
                "error": {"code": -32602, "message": "Nexus cannot parse this user input request."}
            })))];
        }
        let native_request_id = frame["id"].to_string();
        self.pending_user_asks.insert(
            native_request_id.clone(),
            PendingUserAsk {
                reply: UserAskReply::Rpc(frame["id"].clone()),
                questions: questions.clone(),
            },
        );
        vec![DecodedEvent::UserAskRequested(UserAskRequest {
            native_request_id,
            questions,
            timeout_ms: None,
            resolve_on_send: true,
        })]
    }

    fn decode_approval(&self, frame: &Value) -> Vec<DecodedEvent> {
        let params = &frame["params"];
        let method = frame["method"].as_str().unwrap_or_default();
        let response = |result| InputFrame(json!({"id": frame["id"], "result": result}));
        if params.get("threadId").and_then(Value::as_str) != self.thread_id.as_deref()
            || self.thread_id.is_none()
            || params.get("turnId").and_then(Value::as_str) != self.turn_id.as_deref()
            || self.turn_id.is_none()
        {
            return vec![DecodedEvent::WriteStdin(InputFrame(
                json!({"id": frame["id"],
                    "error": {"code": -32602, "message": "Approval does not belong to the active turn."}
                }),
            ))];
        }
        let (title, options, cancel) = match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                let options = [("Approve", "accept"), ("Deny", "decline")]
                    .into_iter()
                    .filter(|(_, decision)| {
                        params
                            .get("availableDecisions")
                            .and_then(Value::as_array)
                            .is_none_or(|available| {
                                available
                                    .iter()
                                    .any(|value| value.as_str() == Some(decision))
                            })
                    })
                    .map(|(label, decision)| ApprovalOption {
                        label: label.into(),
                        response: response(json!({"decision": decision})),
                    })
                    .collect();
                (
                    if method.contains("commandExecution") {
                        "Codex · Command"
                    } else {
                        "Codex · File Change"
                    },
                    options,
                    response(json!({"decision": "cancel"})),
                )
            }
            "item/permissions/requestApproval" => (
                "Codex · Permissions",
                vec![
                    ApprovalOption {
                        label: "Approve".into(),
                        response: response(
                            json!({"permissions": params["permissions"], "scope": "turn"}),
                        ),
                    },
                    ApprovalOption {
                        label: "Deny".into(),
                        response: response(json!({"permissions": {}, "scope": "turn"})),
                    },
                ],
                response(json!({"permissions": {}, "scope": "turn"})),
            ),
            _ => {
                return vec![DecodedEvent::WriteStdin(InputFrame(
                    json!({"id": frame["id"],
                        "error": {"code": -32601, "message": "Nexus does not support this interactive request."}
                    }),
                ))];
            }
        };
        let mut details = Map::new();
        if let Some(item) = params
            .get("itemId")
            .and_then(Value::as_str)
            .and_then(|id| self.approval_items.get(id))
        {
            for key in ["command", "cwd", "changes"] {
                if let Some(value) = item.get(key) {
                    details.insert(key.into(), value.clone());
                }
            }
        }
        for key in [
            "command",
            "cwd",
            "reason",
            "grantRoot",
            "permissions",
            "additionalPermissions",
            "networkApprovalContext",
        ] {
            if let Some(value) = params.get(key).filter(|value| !value.is_null()) {
                details.insert(key.into(), value.clone());
            }
        }
        vec![DecodedEvent::ApprovalRequested(ApprovalPrompt {
            id: frame["id"].to_string(),
            title: title.into(),
            details: serde_json::to_string_pretty(&details).unwrap_or_default(),
            options,
            cancel,
            timeout_ms: None,
        })]
    }
}

fn write_request(id: u64, method: &str, params: Value) -> DecodedEvent {
    DecodedEvent::WriteStdin(InputFrame(
        json!({"id": id, "method": method, "params": params}),
    ))
}

fn decode_started_item(item: &Value) -> Vec<DecodedEvent> {
    let id = item_id(item);
    match item.get("type").and_then(Value::as_str) {
        Some("commandExecution") => vec![DecodedEvent::ToolStarted {
            id,
            name: "Command".into(),
            summary: item
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }],
        Some("mcpToolCall") => vec![DecodedEvent::ToolStarted {
            id,
            name: mcp_tool_name(item),
            summary: item.get("arguments").map(tool_content).unwrap_or_default(),
        }],
        Some("webSearch") => vec![DecodedEvent::ToolStarted {
            id,
            name: "Web Search".into(),
            summary: summarize_text(
                item.get("query")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
        }],
        Some("fileChange") => vec![DecodedEvent::ToolStarted {
            id,
            name: "File Change".into(),
            summary: tool_content(item),
        }],
        Some("userMessage" | "agentMessage" | "reasoning" | "plan") => Vec::new(),
        Some(name) => vec![DecodedEvent::ToolStarted {
            id,
            name: name.into(),
            summary: tool_content(item),
        }],
        _ => Vec::new(),
    }
}

fn decode_completed_item(item: &Value) -> Vec<DecodedEvent> {
    let id = item_id(item);
    match item.get("type").and_then(Value::as_str) {
        Some("agentMessage") => item
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(|text| vec![DecodedEvent::MessageCompleted(text.to_owned())])
            .unwrap_or_default(),
        Some("commandExecution") => {
            let status = item
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("failed");
            let exit_code = item.get("exitCode").and_then(Value::as_i64);
            let output = item
                .get("aggregatedOutput")
                .and_then(Value::as_str)
                .filter(|output| !output.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| match exit_code {
                    Some(code) => format!("退出代码：{code}"),
                    None => format!("命令状态：{status}"),
                });
            vec![DecodedEvent::ToolCompleted {
                id,
                output,
                is_error: status != "completed" || exit_code.is_some_and(|code| code != 0),
            }]
        }
        Some("fileChange") => {
            let status = item
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("failed");
            vec![DecodedEvent::ToolCompleted {
                id,
                output: format!("文件修改状态：{status}"),
                is_error: status != "completed",
            }]
        }
        Some("mcpToolCall") => {
            let error = item.pointer("/error/message").and_then(Value::as_str);
            let output = error
                .map(str::to_owned)
                .or_else(|| item.get("result").map(tool_content))
                .unwrap_or_else(|| "MCP 工具调用已完成".into());
            vec![DecodedEvent::ToolCompleted {
                id,
                output,
                is_error: error.is_some()
                    || item.get("status").and_then(Value::as_str) == Some("failed"),
            }]
        }
        Some("webSearch") => vec![DecodedEvent::ToolCompleted {
            id,
            output: item
                .get("query")
                .and_then(Value::as_str)
                .map(|query| format!("搜索完成：{query}"))
                .unwrap_or_else(|| "搜索已完成".into()),
            is_error: false,
        }],
        Some("userMessage" | "reasoning" | "plan") => Vec::new(),
        Some(_) => vec![DecodedEvent::ToolCompleted {
            id,
            output: tool_content(item),
            is_error: item.get("status").and_then(Value::as_str) == Some("failed"),
        }],
        _ => Vec::new(),
    }
}

fn item_id(item: &Value) -> String {
    item.get("id")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned()
}

fn mcp_tool_name(item: &Value) -> String {
    let server = item.get("server").and_then(Value::as_str).unwrap_or("MCP");
    let tool = item.get("tool").and_then(Value::as_str).unwrap_or("Tool");
    format!("MCP · {server}/{tool}")
}
