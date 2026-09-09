use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use nexus_domain::{
    HarnessKind, ModelAvailability, ModelDescriptor, ModelReasoningEffort, ModelSource,
    PermissionMode, ThinkingEffort, UserAskAnswer, UserAskAnswerMode, UserAskAnswerValue,
    UserAskOption, UserAskQuestion, UserAskStatus,
};
use nexus_harness_core::{
    ApprovalOption, ApprovalPrompt, DecodedEvent, InputFrame, LaunchSpec, LineDecoder,
    ModelCatalogError, UserAskRequest, resolve_executable, summarize_text, tool_content,
};
use nexus_protocol::{EnvironmentVariable, HarnessProbe, StartRun};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    process::Command,
    sync::watch,
    time::timeout,
};

const RPC_TIMEOUT: Duration = Duration::from_secs(30);

mod resume;

fn workspace(cwd: &Path) -> Value {
    json!({"workspaceKey": cwd, "workspacePath": cwd})
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"id": id, "method": method, "params": params})
}

fn mode(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Ask => "build",
        PermissionMode::AutoEdit => "edit",
        PermissionMode::Yolo => "yolo",
    }
}

// Model references are kept losslessly, including variants and slashes in model IDs.
fn model_ref(selector: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(selector).ok()?;
    (value["providerId"].as_str().is_some_and(|s| !s.is_empty())
        && value["modelId"].as_str().is_some_and(|s| !s.is_empty()))
    .then_some(value)
}

fn launch(executable: &str, cwd: &Path, frame: Value) -> LaunchSpec {
    LaunchSpec {
        executable: PathBuf::from(executable),
        args: vec!["app-server".into()],
        cwd: cwd.into(),
        stdin: format!("{frame}\n"),
    }
}

pub fn prepare_run(run: &StartRun, cwd: &Path) -> (LaunchSpec, EventDecoder) {
    let mut params = json!({"workspace": workspace(cwd)});
    let method = if let Some(session_id) = &run.session_id {
        params["sessionId"] = json!(session_id);
        "session/resume"
    } else {
        params["mode"] = json!(mode(run.permission_mode));
        params["titleGenerationEnabled"] = json!(false);
        "session/create"
    };
    (
        launch(&run.executable, cwd, request(1, method, params)),
        EventDecoder {
            prompt: run.prompt.clone(),
            model: run.model.clone(),
            effort: run.effort,
            permission: run.permission_mode,
            session: None,
            pending: VecDeque::new(),
            expected: 1,
            asks: HashMap::new(),
            tools: HashSet::new(),
            last_message: None,
            cwd: cwd.into(),
            environment: run.environment.clone(),
        },
    )
}

pub struct EventDecoder {
    prompt: String,
    model: Option<String>,
    effort: ThinkingEffort,
    permission: PermissionMode,
    session: Option<String>,
    pending: VecDeque<Value>,
    expected: u64,
    asks: HashMap<String, Value>,
    tools: HashSet<String>,
    last_message: Option<String>,
    cwd: PathBuf,
    environment: Vec<EnvironmentVariable>,
}

fn error(message: impl Into<String>) -> Vec<DecodedEvent> {
    vec![
        DecodedEvent::Error(message.into()),
        DecodedEvent::TurnCompleted,
    ]
}

// Runtime preferences are requested before both session creation and execution.
// Keep questions interactive instead of allowing the runtime's timed auto-answer.
fn client_response(frame: &Value) -> Option<Value> {
    let id = frame.get("id")?;
    let method = frame.get("method")?.as_str()?;
    Some(if method == "session/requestRuntimePreferences" {
        json!({"id": id, "result": {"nativeSearchEnhancementsEnabled": true,
            "memoryEnabled": false, "askUserQuestionAutoResolutionEnabled": false}})
    } else {
        json!({"id": id, "error": {"code": -32601,
            "message": format!("Nexus does not support {method}")}})
    })
}

impl LineDecoder for EventDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let frame: Value = serde_json::from_str(line)?;
        if frame.get("method").is_some() && frame.get("id").is_some() {
            return Ok(self.interaction(&frame));
        }
        if frame["method"] == "session/event" {
            return Ok(self.event(&frame["params"]));
        }
        if frame["id"].as_u64() != Some(self.expected) {
            return Ok(vec![]);
        }
        if let Some(failure) = frame.get("error") {
            return Ok(error(format!(
                "ZCode: {}",
                failure["message"].as_str().unwrap_or("请求失败")
            )));
        }
        let result = &frame["result"];
        let mut events = vec![];
        if self.expected == 1 {
            if result["protocol"]["name"] != "ZCode Protocol" || result["protocol"]["version"] != 1
            {
                return Ok(error("不支持的 ZCode Protocol 版本。"));
            }
            let Some(id) = result["session"]["sessionId"]
                .as_str()
                .filter(|id| !id.is_empty())
            else {
                return Ok(error("ZCode 会话响应缺少 sessionId。"));
            };
            self.session = Some(id.into());
            events.push(DecodedEvent::SessionStarted(id.into()));
            self.pending.push_back(request(
                2,
                "session/setMode",
                json!({"sessionId": id, "mode": mode(self.permission)}),
            ));
            let restore =
                result["projection"]["lastError"]["type"] == "ZCODE_RUNTIME_MODEL_UNAVAILABLE";
            let model = match &self.model {
                Some(selector) => match model_ref(selector) {
                    Some(model) => Some(model),
                    None => return Ok(error("无效的 ZCode 模型标识，请重新加载模型目录。")),
                },
                None if restore => result["settings"]["model"].get("current").cloned(),
                None => None,
            };
            if let Some(model) = model {
                let mut params =
                    json!({"sessionId": id, "model": model, "persistAsWorkspaceLastUsed": false});
                if restore {
                    match resume::runtime_model(&self.cwd, &self.environment, &model, result) {
                        Ok(runtime) => params["runtimeModel"] = runtime,
                        Err(message) => return Ok(error(message)),
                    }
                }
                self.pending
                    .push_back(request(3, "session/setModel", params));
            } else if restore {
                return Ok(error("ZCode 恢复会话缺少模型。"));
            }
            if !self.effort.is_default() {
                self.pending.push_back(request(4, "session/setThoughtLevel", json!({"sessionId": id, "thoughtLevel": self.effort.as_str(), "persistAsWorkspaceLastUsed": false})));
            }
            self.pending.push_back(request(
                5,
                "session/subscribe",
                json!({"sessionId": id, "deliveryKind": "desktop-continuous"}),
            ));
            self.pending.push_back(request(
                6,
                "session/send",
                json!({"sessionId": id, "content": self.prompt}),
            ));
        } else if self.expected == 6 && result["accepted"] != true {
            return Ok(error("ZCode 未接受本轮消息。"));
        }
        if let Some(next) = self.pending.pop_front() {
            self.expected = next["id"].as_u64().unwrap();
            events.push(DecodedEvent::WriteStdin(InputFrame(next)));
        } else {
            self.expected = 0;
        }
        Ok(events)
    }

    fn steer(&mut self, _message_id: &str, _prompt: &str) -> Option<InputFrame> {
        None
    }

    fn answer_user_ask(
        &mut self,
        native_request_id: &str,
        answers: &[UserAskAnswer],
    ) -> Option<InputFrame> {
        let id = self.asks.remove(native_request_id)?;
        let mut content = serde_json::Map::new();
        for answer in answers {
            let value = match &answer.value {
                UserAskAnswerValue::Text(text) => json!(text),
                UserAskAnswerValue::Selected(values) => json!(values),
            };
            content.insert(answer.question_id.clone(), value);
        }
        Some(InputFrame(
            json!({"id": id, "result": {"action": "accept", "content": content}}),
        ))
    }
}

impl EventDecoder {
    fn interaction(&mut self, frame: &Value) -> Vec<DecodedEvent> {
        let params = &frame["params"];
        let id = &frame["id"];
        match frame["method"].as_str().unwrap_or_default() {
            "interaction/requestPermission" => {
                let options = params["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|option| {
                        Some(ApprovalOption {
                            label: option["name"].as_str()?.into(),
                            response: InputFrame(
                                json!({"id": id, "result": option.get("response")?}),
                            ),
                        })
                    })
                    .collect::<Vec<_>>();
                let cancel = InputFrame(
                    json!({"id": id, "result": {"decision": "deny", "reason": "Cancelled in Nexus"}}),
                );
                if options.is_empty() {
                    return vec![DecodedEvent::WriteStdin(cancel)];
                }
                vec![DecodedEvent::ApprovalRequested(ApprovalPrompt {
                    id: params["requestId"].as_str().unwrap_or_default().into(),
                    title: params["toolName"].as_str().unwrap_or("ZCode").into(),
                    details: format!(
                        "{}\n{}",
                        params["reason"].as_str().unwrap_or_default(),
                        tool_content(&params["input"])
                    ),
                    options,
                    cancel,
                    timeout_ms: None,
                })]
            }
            "interaction/requestUserInput" => {
                let native_id = params["requestId"].as_str().unwrap_or_default().to_owned();
                let questions = params["questions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(index, question)| {
                        let options = question["options"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|option| {
                                Some(UserAskOption {
                                    id: option["value"].as_str()?.into(),
                                    label: option["label"].as_str()?.into(),
                                    description: option["description"].as_str().map(Into::into),
                                })
                            })
                            .collect::<Vec<_>>();
                        UserAskQuestion {
                            id: if params["schema"]["interaction"] == "plan_approval" {
                                "answer".into()
                            } else {
                                format!("answer_{index}")
                            },
                            prompt: question["question"].as_str().unwrap_or_default().into(),
                            answer_mode: if options.is_empty() {
                                UserAskAnswerMode::Text
                            } else {
                                UserAskAnswerMode::Choice {
                                    multiple: question["multiSelect"].as_bool().unwrap_or(false),
                                    allow_custom: true,
                                }
                            },
                            options,
                        }
                    })
                    .collect::<Vec<_>>();
                if native_id.is_empty() || questions.is_empty() {
                    return vec![DecodedEvent::WriteStdin(InputFrame(
                        json!({"id": id, "result": {"action": "decline"}}),
                    ))];
                }
                // Reannouncements reuse the native request ID but carry a fresh transport ID.
                let repeated = self.asks.insert(native_id.clone(), id.clone()).is_some();
                if repeated {
                    vec![]
                } else {
                    vec![DecodedEvent::UserAskRequested(UserAskRequest {
                        native_request_id: native_id,
                        questions,
                        timeout_ms: None,
                        resolve_on_send: false,
                    })]
                }
            }
            _ => client_response(frame)
                .map(|response| vec![DecodedEvent::WriteStdin(InputFrame(response))])
                .unwrap_or_default(),
        }
    }

    fn event(&mut self, event: &Value) -> Vec<DecodedEvent> {
        if event["sessionId"].as_str() != self.session.as_deref() {
            return vec![];
        }
        let payload = &event["payload"];
        match event["type"].as_str().unwrap_or_default() {
            "session.updated"
                if payload["querySource"] == "main_turn"
                    && payload["content"]
                        .as_str()
                        .is_some_and(|text| !text.is_empty()) =>
            {
                let text = payload["content"].as_str().unwrap().to_owned();
                self.last_message = Some(text.clone());
                vec![DecodedEvent::MessageCompleted(text)]
            }
            // part.delta mirrors model.streaming; consuming both duplicates the answer.
            "model.streaming" if payload["kind"] == "text_delta" => vec![DecodedEvent::TextDelta(
                payload["delta"].as_str().unwrap_or_default().into(),
            )],
            "turn.completed" => {
                if payload["resultType"] != "success" {
                    return error(format!("ZCode 本轮未成功结束：{}", payload["resultType"]));
                }
                let text = payload["response"].as_str().unwrap_or_default();
                let mut events = Vec::new();
                if self.last_message.as_deref() != Some(text) {
                    events.push(DecodedEvent::MessageCompleted(text.into()));
                }
                events.push(DecodedEvent::TurnCompleted);
                events
            }
            "turn.failed" => error(
                payload["error"]["message"]
                    .as_str()
                    .unwrap_or("ZCode 执行失败。"),
            ),
            "tool.updated" | "model.streaming" => {
                let Some(id) = payload["toolCallId"].as_str() else {
                    return vec![];
                };
                match payload["kind"].as_str().unwrap_or_default() {
                    "scheduled" | "started" | "tool_call" if self.tools.insert(id.into()) => {
                        vec![DecodedEvent::ToolStarted {
                            id: id.into(),
                            name: payload["toolName"].as_str().unwrap_or("tool").into(),
                            summary: summarize_text(&tool_content(&payload["input"])),
                        }]
                    }
                    "result" => vec![DecodedEvent::ToolCompleted {
                        id: id.into(),
                        output: tool_content(
                            payload["result"]
                                .get("content")
                                .unwrap_or(&payload["result"]),
                        ),
                        is_error: payload["result"]["success"] == false,
                    }],
                    "error" => vec![DecodedEvent::ToolCompleted {
                        id: id.into(),
                        output: payload["error"]["message"]
                            .as_str()
                            .unwrap_or("Tool failed")
                            .into(),
                        is_error: true,
                    }],
                    _ => vec![],
                }
            }
            "permission.resolved" => vec![DecodedEvent::ApprovalResolved(
                payload["requestId"].as_str().unwrap_or_default().into(),
            )],
            "userInput.resolved" => {
                let id = payload["requestId"].as_str().unwrap_or_default();
                self.asks.remove(id);
                vec![DecodedEvent::UserAskFinished {
                    native_request_id: id.into(),
                    status: if payload["cancelled"] == true {
                        UserAskStatus::Cancelled
                    } else {
                        UserAskStatus::Answered
                    },
                    message: None,
                }]
            }
            _ => vec![],
        }
    }
}

pub fn prepare_title(run: &StartRun, cwd: &Path, prompt: &str) -> (LaunchSpec, TitleDecoder) {
    // generateText needs an initialized app to resolve CLI-owned provider config.
    // A deferred session is not persisted unless session/send is called.
    let mut params = json!({"workspace": workspace(cwd), "persistence": "deferred",
        "mode": "build", "titleGenerationEnabled": false});
    if let Some(model) = run.model.as_deref().and_then(model_ref) {
        params["model"] = model;
    }
    if !run.effort.is_default() {
        params["thoughtLevel"] = json!(run.effort.as_str());
    }
    (
        launch(&run.executable, cwd, request(1, "session/create", params)),
        TitleDecoder {
            cwd: cwd.into(),
            prompt: prompt.into(),
            model: run.model.clone(),
        },
    )
}

pub struct TitleDecoder {
    cwd: PathBuf,
    prompt: String,
    model: Option<String>,
}

impl LineDecoder for TitleDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let frame: Value = serde_json::from_str(line)?;
        if let Some(response) = client_response(&frame) {
            return Ok(vec![DecodedEvent::WriteStdin(InputFrame(response))]);
        }
        if frame.get("error").is_some() {
            return Ok(error("ZCode 标题生成失败。"));
        }
        Ok(match frame["id"].as_u64() {
            Some(1) => {
                let model = match &self.model {
                    Some(selector) => model_ref(selector),
                    None => frame["result"]["settings"]["model"].get("current").cloned(),
                };
                let Some(model) = model else {
                    return Ok(error("ZCode 标题生成缺少模型。"));
                };
                vec![DecodedEvent::WriteStdin(InputFrame(request(
                    2,
                    "workspace/generateText",
                    json!({
                        "workspace": workspace(&self.cwd), "modelRef": model, "prompt": self.prompt,
                        "querySource": "nexus_title", "maxOutputTokens": 128
                    }),
                )))]
            }
            Some(2) => vec![
                DecodedEvent::MessageCompleted(
                    frame["result"]["text"].as_str().unwrap_or_default().into(),
                ),
                DecodedEvent::TurnCompleted,
            ],
            _ => vec![],
        })
    }
    fn steer(&mut self, _: &str, _: &str) -> Option<InputFrame> {
        None
    }
}

pub async fn discover_models(
    executable: &str,
    cwd: &Path,
    environment: &[EnvironmentVariable],
    mut cancel: watch::Receiver<bool>,
) -> Result<Vec<ModelDescriptor>, ModelCatalogError> {
    if *cancel.borrow() {
        return Err(ModelCatalogError::Cancelled);
    }
    let executable = resolve_executable(executable).ok_or_else(|| {
        ModelCatalogError::Failed(
            "未找到 ZCode CLI，请安装 zcode-app-cli 或配置兼容的可执行文件。".into(),
        )
    })?;
    let mut child = Command::new(executable)
        .arg("app-server")
        .current_dir(cwd)
        .envs(environment.iter().map(|v| (&v.name, &v.value)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| ModelCatalogError::Failed("无法启动 ZCode 模型目录命令。".into()))?;
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let collect = async {
        stdin
            .write_all(
                format!(
                    "{}\n",
                    request(
                        1,
                        "workspace/readState",
                        json!({"workspace": workspace(cwd)})
                    )
                )
                .as_bytes(),
            )
            .await
            .map_err(|_| "无法向 ZCode 发送模型目录请求。".to_owned())?;
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|_| "无法读取 ZCode 模型目录。".to_owned())?
        {
            let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(response) = client_response(&frame) {
                stdin
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .map_err(|_| "无法响应 ZCode 请求。".to_owned())?;
            } else if frame["id"] == 1 {
                if frame.get("error").is_some() {
                    return Err("ZCode 模型目录加载失败，请检查 CLI 配置。".into());
                }
                return parse_catalog(&frame["result"]);
            }
        }
        Err("ZCode 未返回模型目录。".into())
    };
    let result = tokio::select! {
        result = timeout(RPC_TIMEOUT, collect) => result.unwrap_or_else(|_| Err("ZCode 模型目录请求超时。".into())).map_err(ModelCatalogError::Failed),
        _ = cancel.changed() => Err(ModelCatalogError::Cancelled),
    };
    drop(stdin);
    if timeout(Duration::from_secs(2), child.wait()).await.is_err() {
        let _ = child.kill().await;
    }
    result
}

fn parse_catalog(state: &Value) -> Result<Vec<ModelDescriptor>, String> {
    let model = &state["settings"]["model"];
    let available = model["available"]
        .as_array()
        .ok_or("ZCode 模型目录格式无效。")?;
    let mut ids = HashSet::new();
    available
        .iter()
        .map(|item| {
            let reference = item
                .get("ref")
                .filter(|value| model_ref(&value.to_string()).is_some())
                .ok_or("ZCode 模型缺少有效 ref。")?;
            let id = reference.to_string();
            if !ids.insert(id.clone()) {
                return Err("ZCode 模型目录包含重复标识。".into());
            }
            let reasoning = &item["reasoning"];
            let supported_reasoning_efforts = reasoning["levels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|level| {
                    Some(ModelReasoningEffort {
                        effort: level["value"].as_str()?.parse().ok()?,
                        description: level["description"].as_str().unwrap_or_default().into(),
                    })
                })
                .collect();
            Ok(ModelDescriptor {
                id,
                display_name: item["label"]
                    .as_str()
                    .unwrap_or(reference["modelId"].as_str().unwrap())
                    .into(),
                source: ModelSource::ZcodeAppServer,
                availability: item["disabledReason"]
                    .as_str()
                    .filter(|reason| !reason.is_empty())
                    .map(|reason| ModelAvailability::Unavailable {
                        reason: reason.into(),
                    })
                    .unwrap_or(ModelAvailability::Available),
                provider: reference["providerId"].as_str().map(Into::into),
                is_default: reference == &model["current"],
                supported_reasoning_efforts,
                default_reasoning_effort: reasoning["defaultLevel"]
                    .as_str()
                    .and_then(|value| value.parse().ok()),
            })
        })
        .collect()
}

pub async fn probe(configured: &str) -> HarnessProbe {
    let mut probe = HarnessProbe {
        harness: HarnessKind::Zcode,
        executable: configured.into(),
        available: false,
        authenticated: false,
        version: None,
        message: "未找到 ZCode CLI，请安装 zcode-app-cli 并完成配置。".into(),
    };
    let Some(executable) = resolve_executable(configured) else {
        return probe;
    };
    probe.executable = executable.to_string_lossy().into();
    let output = timeout(
        RPC_TIMEOUT,
        Command::new(&executable)
            .arg("--version")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let Ok(Ok(output)) = output else {
        probe.message = "ZCode 版本探测失败或超时。".into();
        return probe;
    };
    if !output.status.success() {
        probe.message = "ZCode 版本探测失败。".into();
        return probe;
    }
    probe.available = true;
    probe.version = Some(String::from_utf8_lossy(&output.stdout).trim().into());
    let (_sender, cancel) = watch::channel(false);
    if let Ok(cwd) = std::env::current_dir() {
        probe.authenticated = discover_models(&probe.executable, &cwd, &[], cancel)
            .await
            .is_ok_and(|models| {
                models
                    .iter()
                    .any(|model| model.availability.is_selectable())
            });
    }
    probe.message = if probe.authenticated {
        "ZCode 已发现可用模型；实际调用取决于 CLI 登录和账号额度。"
    } else {
        "ZCode CLI 可用，请先运行 zcode 完成登录与模型配置。"
    }
    .into();
    probe
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> StartRun {
        serde_json::from_value(json!({
            "run_id": "00000000-0000-0000-0000-000000000001",
            "task_id": "00000000-0000-0000-0000-000000000002",
            "cwd": "/project", "prompt": "你好\nprivate prompt", "harness": "zcode",
            "executable": "zcode", "effort": "default", "permission_mode": "ask"
        }))
        .unwrap()
    }

    fn decode(decoder: &mut impl LineDecoder, value: Value) -> Vec<DecodedEvent> {
        decoder.decode_line(&value.to_string()).unwrap()
    }

    fn opened(decoder: &mut EventDecoder) -> Vec<DecodedEvent> {
        decode(
            decoder,
            json!({"id": 1, "result": {
                "protocol": {"name": "ZCode Protocol", "version": 1},
                "session": {"sessionId": "sess_test"}
            }}),
        )
    }

    fn event(decoder: &mut EventDecoder, kind: &str, payload: Value) -> Vec<DecodedEvent> {
        decode(
            decoder,
            json!({"method": "session/event", "params": {
                "sessionId": "sess_test", "type": kind, "payload": payload
            }}),
        )
    }

    #[test]
    fn resume_reapplies_permissions_model_and_effort_before_subscribing_and_sending() {
        let mut run = run();
        run.session_id = Some("sess_test".into());
        run.model =
            Some(json!({"providerId":"test", "modelId":"org/model", "variant":"fast"}).to_string());
        run.effort = ThinkingEffort::High;
        let (spec, mut decoder) = prepare_run(&run, Path::new("/project"));
        assert_eq!(spec.args, ["app-server"]);
        assert!(!spec.stdin.contains("private prompt"));
        let initial: Value = serde_json::from_str(&spec.stdin).unwrap();
        assert_eq!(initial["method"], "session/resume");
        assert_eq!(initial["params"]["sessionId"], "sess_test");
        let events = opened(&mut decoder);
        assert!(matches!(&events[0], DecodedEvent::SessionStarted(id) if id == "sess_test"));
        assert!(
            matches!(&events[1], DecodedEvent::WriteStdin(frame) if frame.0["params"]["mode"] == "build")
        );
        for (id, method) in [
            (2, "session/setModel"),
            (3, "session/setThoughtLevel"),
            (4, "session/subscribe"),
            (5, "session/send"),
        ] {
            let events = decode(&mut decoder, json!({"id":id, "result":{}}));
            let DecodedEvent::WriteStdin(frame) = &events[0] else {
                panic!("missing next request");
            };
            assert_eq!(frame.0["method"], method);
            if id == 2 {
                assert_eq!(frame.0["params"]["model"]["modelId"], "org/model");
                assert_eq!(frame.0["params"]["persistAsWorkspaceLastUsed"], false);
            }
            if id == 5 {
                assert_eq!(frame.0["params"]["content"], run.prompt);
            }
        }
        assert!(decode(&mut decoder, json!({"id":6,"result":{"accepted":true}})).is_empty());
        assert!(decoder.steer("later", "next turn").is_none());
    }

    #[test]
    fn cold_resume_restores_private_provider_configuration_and_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        std::fs::create_dir_all(home.join(".zcode/cli")).unwrap();
        std::fs::write(home.join(".zcode/cli/config.json"), r#"{"provider":{"test":{"kind":"anthropic","options":{"baseURL":"https://old.invalid"},"models":{"title-model":{}}}},"model":{"main":"test/title-model"}}"#).unwrap();
        std::fs::write(directory.path().join("zcode.json"), r#"{"provider":{"test":{"kind":"openai-compatible","options":{"baseURL":"http://localhost:8080/v1","apiKey":"private-key","headers":{"x-test":"provider"}},"models":{"alias":{"id":"title-model","options":{"temperature":0.2},"headers":{"x-test":"model"}}}}},"model":{"main":"test/title-model"}}"#).unwrap();
        let mut run = run();
        run.session_id = Some("sess_test".into());
        run.environment = ["HOME", "USERPROFILE"]
            .into_iter()
            .map(|name| EnvironmentVariable {
                name: name.into(),
                value: home.to_string_lossy().into(),
            })
            .collect();
        run.environment.push(EnvironmentVariable {
            name: "ZCODE_MODEL".into(),
            value: String::new(),
        });
        let (_, mut decoder) = prepare_run(&run, directory.path());
        let snapshot = json!({"protocol":{"name":"ZCode Protocol","version":1},"session":{"sessionId":"sess_test"},"projection":{"lastError":{"type":"ZCODE_RUNTIME_MODEL_UNAVAILABLE"}},"settings":{"model":{"current":{"providerId":"test","modelId":"title-model"},"available":[]}}});
        decode(&mut decoder, json!({"id":1,"result":snapshot}));
        let events = decode(&mut decoder, json!({"id":2,"result":{}}));
        assert!(!format!("{events:?}").contains("private-key"));
        let DecodedEvent::WriteStdin(frame) = &events[0] else {
            panic!("missing model restore")
        };
        let provider = &frame.0["params"]["runtimeModel"]["provider"];
        assert_eq!(provider["baseURL"], "http://localhost:8080/v1");
        assert_eq!(provider["apiKey"]["value"], "private-key");
        assert_eq!(provider["headers"]["x-test"], "model");
        assert_eq!(
            provider["models"][0]["providerOptions"]["openaiCompatible"]["temperature"],
            0.2
        );
        std::fs::write(directory.path().join("zcode.json"), "invalid").unwrap();
        let (_, mut decoder) = prepare_run(&run, directory.path());
        assert!(matches!(
            decode(&mut decoder, json!({"id":1,"result":snapshot})).as_slice(),
            [DecodedEvent::Error(_), DecodedEvent::TurnCompleted]
        ));
    }

    #[test]
    fn new_sessions_use_each_native_permission_mode_and_fail_closed_on_bad_protocol() {
        for (permission, native) in [
            (PermissionMode::Ask, "build"),
            (PermissionMode::AutoEdit, "edit"),
            (PermissionMode::Yolo, "yolo"),
        ] {
            let mut run = run();
            run.permission_mode = permission;
            let (spec, mut decoder) = prepare_run(&run, Path::new("/project"));
            let frame: Value = serde_json::from_str(&spec.stdin).unwrap();
            assert_eq!(frame["params"]["mode"], native);
            assert_eq!(frame["params"]["titleGenerationEnabled"], false);
            assert!(matches!(
                &decode(
                    &mut decoder,
                    json!({"id":1,"result":{"protocol":{"version":2}}})
                )[0],
                DecodedEvent::Error(_)
            ));
            assert!(
                matches!(&decode(&mut decoder,json!({"id":1,"error":{"message":"resume failed"}}))[0], DecodedEvent::Error(message) if message.contains("resume failed"))
            );
        }
    }

    #[test]
    fn streams_once_and_distinguishes_failed_turns_and_tool_results() {
        let (_, mut decoder) = prepare_run(&run(), Path::new("/project"));
        opened(&mut decoder);
        assert_eq!(
            event(
                &mut decoder,
                "model.streaming",
                json!({"kind":"text_delta","delta":"hello"})
            ),
            [DecodedEvent::TextDelta("hello".into())]
        );
        assert!(
            event(
                &mut decoder,
                "part.delta",
                json!({"field":"text","delta":"hello"})
            )
            .is_empty()
        );
        assert_eq!(
            event(
                &mut decoder,
                "session.updated",
                json!({"querySource":"main_turn","content":"hello"})
            ),
            [DecodedEvent::MessageCompleted("hello".into())]
        );
        assert_eq!(
            event(
                &mut decoder,
                "turn.completed",
                json!({"resultType":"success","response":"hello"})
            ),
            [DecodedEvent::TurnCompleted]
        );
        let tool = json!({"kind":"scheduled","toolCallId":"t1","toolName":"Bash","input":{"command":"pwd"}});
        assert!(matches!(
            &event(&mut decoder, "tool.updated", tool.clone())[0],
            DecodedEvent::ToolStarted { .. }
        ));
        assert!(event(&mut decoder, "tool.updated", tool).is_empty());
        assert!(matches!(
            &event(
                &mut decoder,
                "tool.updated",
                json!({"kind":"result","toolCallId":"t1","result":{"content":"failed","success":false}})
            )[0],
            DecodedEvent::ToolCompleted { is_error: true, .. }
        ));
        for result_type in ["cancelled", "error_during_execution", "error_max_turns"] {
            assert!(matches!(
                &event(
                    &mut decoder,
                    "turn.completed",
                    json!({"resultType":result_type})
                )[0],
                DecodedEvent::Error(_)
            ));
        }
        assert!(decode(&mut decoder,json!({"method":"session/event","params":{"sessionId":"other","type":"turn.completed","payload":{"resultType":"success"}}})).is_empty());
    }

    #[test]
    fn approvals_preserve_native_choices_and_deny_on_cancellation() {
        let (_, mut decoder) = prepare_run(&run(), Path::new("/project"));
        let response = json!({"decision":"modify","modifiedInput":{"command":"safe"}});
        let events = decode(
            &mut decoder,
            json!({"id":"server-1","method":"interaction/requestPermission","params":{
                "requestId":"permission-1","toolName":"Bash","input":{"command":"original"},"options":[{"name":"Approve modified command","response":response}]
            }}),
        );
        let DecodedEvent::ApprovalRequested(prompt) = &events[0] else {
            panic!("approval missing");
        };
        assert_eq!(prompt.options[0].response.0["result"], response);
        assert_eq!(prompt.options[0].response.0["id"], "server-1");
        assert_eq!(prompt.cancel.0["result"]["decision"], "deny");
        let events = decode(
            &mut decoder,
            json!({"id":2,"method":"session/requestRuntimePreferences"}),
        );
        assert!(
            matches!(&events[0],DecodedEvent::WriteStdin(frame) if frame.0["result"]["askUserQuestionAutoResolutionEnabled"] == false)
        );
        let events = decode(&mut decoder, json!({"id":3,"method":"unsupported/request"}));
        assert!(
            matches!(&events[0],DecodedEvent::WriteStdin(frame) if frame.0["error"]["code"] == -32601)
        );
    }

    #[test]
    fn questions_preserve_multiple_choices_and_refresh_transport_ids() {
        let (_, mut decoder) = prepare_run(&run(), Path::new("/project"));
        opened(&mut decoder);
        let mut frame = json!({"id":"server-1","method":"interaction/requestUserInput","params":{
            "requestId":"ask-1","questions":[{"question":"Which checks?","multiSelect":true,"options":[{"label":"Tests","value":"tests"},{"label":"Build","value":"build"}]}]
        }});
        let events = decode(&mut decoder, frame.clone());
        let [DecodedEvent::UserAskRequested(ask)] = events.as_slice() else {
            panic!("question missing");
        };
        assert_eq!(ask.timeout_ms, None);
        assert!(!ask.resolve_on_send);
        assert!(matches!(
            ask.questions[0].answer_mode,
            UserAskAnswerMode::Choice {
                multiple: true,
                allow_custom: true
            }
        ));
        frame["id"] = json!("server-2");
        assert!(decode(&mut decoder, frame).is_empty());
        let response = decoder
            .answer_user_ask(
                "ask-1",
                &[UserAskAnswer {
                    question_id: "answer_0".into(),
                    value: UserAskAnswerValue::Selected(vec!["tests".into(), "build".into()]),
                }],
            )
            .unwrap();
        assert_eq!(response.0["id"], "server-2");
        assert_eq!(
            response.0["result"]["content"]["answer_0"],
            json!(["tests", "build"])
        );
        assert!(decoder.answer_user_ask("ask-1", &[]).is_none());
        assert_eq!(
            event(
                &mut decoder,
                "userInput.resolved",
                json!({"requestId":"ask-1"})
            ),
            [DecodedEvent::UserAskFinished {
                native_request_id: "ask-1".into(),
                status: UserAskStatus::Answered,
                message: None,
            }]
        );
    }

    #[test]
    fn catalog_keeps_model_identity_availability_and_supported_efforts() {
        let reference = json!({"providerId":"local","modelId":"org/model","variant":"fast"});
        let state = json!({"settings":{"model":{"current":reference,"available":[{
            "ref":reference,"label":"Model","disabledReason":"Login required",
            "reasoning":{"enabled":true,"defaultLevel":"high","levels":[{"value":"low"},{"value":"high"},{"value":"future-effort"}]}
        }]}}});
        let models = parse_catalog(&state).unwrap();
        assert_eq!(model_ref(&models[0].id), Some(reference));
        assert_eq!(models[0].source.harness(), HarnessKind::Zcode);
        assert!(models[0].is_default);
        assert!(!models[0].availability.is_selectable());
        assert_eq!(models[0].supported_reasoning_efforts.len(), 2);
        assert_eq!(
            models[0].default_reasoning_effort,
            Some(ThinkingEffort::High)
        );
        assert!(parse_catalog(&json!({})).is_err());
        assert!(
            parse_catalog(&json!({"settings":{"model":{"available":[]}}}))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn title_initializes_a_deferred_session_and_generates_without_tools() {
        let (spec, mut decoder) =
            prepare_title(&run(), Path::new("/project"), "private title prompt");
        assert!(!spec.stdin.contains("private title prompt"));
        let initial: Value = serde_json::from_str(&spec.stdin).unwrap();
        assert_eq!(initial["method"], "session/create");
        assert_eq!(initial["params"]["persistence"], "deferred");
        assert_eq!(initial["params"]["titleGenerationEnabled"], false);
        let events = decode(
            &mut decoder,
            json!({"id":1,"result":{"settings":{"model":{"current":{"providerId":"p","modelId":"m"}}}}}),
        );
        let DecodedEvent::WriteStdin(frame) = &events[0] else {
            panic!("missing generation request");
        };
        assert_eq!(frame.0["method"], "workspace/generateText");
        assert_eq!(frame.0["params"]["prompt"], "private title prompt");
        assert!(frame.0["params"].get("tools").is_none());
        assert_eq!(
            decode(&mut decoder, json!({"id":2,"result":{"text":"Title"}})),
            [
                DecodedEvent::MessageCompleted("Title".into()),
                DecodedEvent::TurnCompleted
            ]
        );
    }
}
