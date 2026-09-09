use nexus_domain::{
    HarnessKind, ModelAvailability, ModelDescriptor, ModelReasoningEffort, ModelSource,
    PermissionMode, ThinkingEffort,
};
use nexus_harness_core::{
    ApprovalOption, ApprovalPrompt, DecodedEvent, InputFrame, LaunchSpec, LineDecoder,
    ModelCatalogError, resolve_executable, tool_content,
};
use nexus_protocol::{EnvironmentVariable, HarnessProbe, StartRun, TextGenerationConfig};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    process::Command,
    sync::watch,
    time::timeout,
};

fn request(id: &str, method: &str, params: Value) -> InputFrame {
    InputFrame(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
}
fn initialize() -> InputFrame {
    request(
        "initialize",
        "initialize",
        json!({"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"nexus","version":env!("CARGO_PKG_VERSION")}}),
    )
}
fn args(harness: HarnessKind) -> Vec<String> {
    if harness == HarnessKind::Kimi {
        vec!["acp".into()]
    } else {
        vec!["--acp".into(), "--permission-mode".into(), "default".into()]
    }
}
fn source(harness: HarnessKind) -> ModelSource {
    match harness {
        HarnessKind::Kimi => ModelSource::KimiAcp,
        HarnessKind::Qoder => ModelSource::QoderAcp,
        HarnessKind::Codebuddy => ModelSource::CodebuddyAcp,
        _ => unreachable!(),
    }
}
pub fn prepare_run(run: &StartRun, cwd: &Path) -> (LaunchSpec, EventDecoder) {
    (
        LaunchSpec {
            executable: run.executable.clone().into(),
            cwd: cwd.into(),
            args: args(run.harness),
            stdin: format!("{}\n", initialize().0),
        },
        EventDecoder {
            harness: run.harness,
            cwd: cwd.to_string_lossy().into(),
            session: run.session_id.clone(),
            prompt: run.prompt.clone(),
            model: run.model.clone(),
            effort: run.effort,
            permission: run.permission_mode,
            pending: VecDeque::new(),
            active: false,
            text: String::new(),
            tools: HashMap::new(),
            config: Value::Null,
        },
    )
}
#[derive(Default)]
struct Tool {
    kind: String,
    output: String,
    finished: bool,
}
pub struct EventDecoder {
    harness: HarnessKind,
    cwd: String,
    session: Option<String>,
    prompt: String,
    model: Option<String>,
    effort: ThinkingEffort,
    permission: PermissionMode,
    pending: VecDeque<InputFrame>,
    active: bool,
    text: String,
    tools: HashMap<String, Tool>,
    config: Value,
}
impl EventDecoder {
    fn fail(&mut self, message: impl Into<String>) -> Vec<DecodedEvent> {
        self.active = false;
        vec![
            DecodedEvent::Error(message.into()),
            DecodedEvent::TurnCompleted,
        ]
    }
    fn flush(&mut self, events: &mut Vec<DecodedEvent>) {
        if !self.text.is_empty() {
            events.push(DecodedEvent::MessageCompleted(std::mem::take(
                &mut self.text,
            )));
        }
    }
    fn next(&mut self) -> Vec<DecodedEvent> {
        if let Some(frame) = self.pending.pop_front() {
            return vec![DecodedEvent::WriteStdin(frame)];
        }
        self.active = true;
        vec![DecodedEvent::WriteStdin(request(
            "prompt",
            "session/prompt",
            json!({"sessionId":self.session,"prompt":[{"type":"text","text":self.prompt}]}),
        ))]
    }
    fn setup(&mut self, result: &Value) -> Vec<DecodedEvent> {
        if let Some(id) = result["sessionId"].as_str().filter(|s| !s.is_empty()) {
            self.session = Some(id.into());
        }
        if self.session.is_none() {
            return self.fail("ACP 未返回会话 ID。");
        }
        if result["configOptions"].is_array() {
            self.config = result["configOptions"].clone();
        }
        // Restored sessions must return to native default permissions before receiving a prompt.
        self.pending.push_back(request(
            "mode",
            "session/set_mode",
            json!({"sessionId":self.session,"modeId":"default"}),
        ));
        if let Some(model) = &self.model {
            let frame = if let Some(config) = config_option(&self.config, "model") {
                request(
                    "model",
                    "session/set_config_option",
                    json!({"sessionId":self.session,"configId":config["id"],"value":model}),
                )
            } else {
                request(
                    "model",
                    "session/set_model",
                    json!({"sessionId":self.session,"modelId":model}),
                )
            };
            self.pending.push_back(frame);
        }
        let mut events = vec![DecodedEvent::SessionStarted(self.session.clone().unwrap())];
        events.extend(self.next());
        events
    }
    fn permission(&self, frame: &Value) -> Vec<DecodedEvent> {
        let params = &frame["params"];
        let response = |outcome: Value| {
            InputFrame(json!({"jsonrpc":"2.0","id":frame["id"],"result":{"outcome":outcome}}))
        };
        let cancel = response(json!({"outcome":"cancelled"}));
        if !self.active || params["sessionId"].as_str() != self.session.as_deref() {
            return vec![DecodedEvent::WriteStdin(cancel)];
        }
        let tool = &params["toolCall"];
        let kind = tool["kind"]
            .as_str()
            .or_else(|| {
                self.tools
                    .get(tool["toolCallId"].as_str().unwrap_or(""))
                    .map(|t| t.kind.as_str())
            })
            .unwrap_or("");
        let options = params["options"].as_array().cloned().unwrap_or_default();
        if (self.permission == PermissionMode::Yolo
            || (self.permission == PermissionMode::AutoEdit && kind == "edit"))
            && let Some(option) = options
                .iter()
                .find(|o| o["kind"] == "allow_once" && o["optionId"].is_string())
        {
            return vec![DecodedEvent::WriteStdin(response(
                json!({"outcome":"selected","optionId":option["optionId"]}),
            ))];
        }
        let options: Vec<_> = options
            .iter()
            .filter_map(|o| {
                Some(ApprovalOption {
                    label: o["name"].as_str()?.into(),
                    response: response(
                        json!({"outcome":"selected","optionId":o["optionId"].as_str()?}),
                    ),
                })
            })
            .collect();
        if options.is_empty() {
            return vec![DecodedEvent::WriteStdin(cancel)];
        }
        vec![DecodedEvent::ApprovalRequested(ApprovalPrompt {
            id: frame["id"].to_string(),
            title: self.harness.to_string(),
            details: tool_content(tool),
            options,
            cancel,
            timeout_ms: None,
        })]
    }
    fn update(&mut self, params: &Value) -> Vec<DecodedEvent> {
        let update = &params["update"];
        if update["sessionUpdate"] == "config_option_update"
            && (self.session.is_none() || params["sessionId"].as_str() == self.session.as_deref())
        {
            self.config = update["configOptions"].clone();
        }
        // session/load may replay the entire native transcript before acknowledging the load.
        if !self.active
            || params["sessionId"].as_str() != self.session.as_deref()
            || params["_meta"].get("codebuddy.ai/memberEvent").is_some()
            || update["_meta"].get("codebuddy.ai/memberEvent").is_some()
        {
            return vec![];
        }
        let mut events = vec![];
        match update["sessionUpdate"].as_str().unwrap_or("") {
            "agent_message_chunk" => {
                if let Some(text) = update["content"]["text"].as_str() {
                    self.text.push_str(text);
                    events.push(DecodedEvent::TextDelta(text.into()));
                }
            }
            "tool_call" | "tool_call_update" => {
                let Some(id) = update["toolCallId"].as_str() else {
                    return events;
                };
                self.flush(&mut events);
                if !self.tools.contains_key(id) {
                    events.push(DecodedEvent::ToolStarted {
                        id: id.into(),
                        name: update["title"].as_str().unwrap_or("Tool").into(),
                        summary: update.get("rawInput").map(tool_content).unwrap_or_default(),
                    });
                }
                let tool = self.tools.entry(id.into()).or_default();
                if let Some(kind) = update["kind"].as_str() {
                    tool.kind = kind.into();
                }
                if let Some(output) = update.get("rawOutput").filter(|v| !v.is_null()) {
                    tool.output = tool_content(output);
                } else if let Some(content) = update.get("content").and_then(Value::as_array) {
                    tool.output = content
                        .iter()
                        .map(content_text)
                        .collect::<Vec<_>>()
                        .join("\n");
                }
                if matches!(update["status"].as_str(), Some("completed" | "failed"))
                    && !tool.finished
                {
                    tool.finished = true;
                    events.push(DecodedEvent::ToolCompleted {
                        id: id.into(),
                        output: tool.output.clone(),
                        is_error: update["status"] == "failed",
                    });
                }
            }
            _ => {}
        }
        events
    }
    fn effort_frame(&mut self) -> Result<(), String> {
        if self.effort.is_default() {
            return Ok(());
        }
        let config = config_option(&self.config, "thought_level")
            .ok_or("该 ACP 引擎未提供思考层级配置。")?;
        let selected = select_options(config)
            .into_iter()
            .find(|option| effort(option["value"].as_str().unwrap_or("")) == Some(self.effort))
            .ok_or("该 ACP 引擎不支持所选思考层级。")?;
        self.pending.push_back(request(
            "effort",
            "session/set_config_option",
            json!({"sessionId":self.session,"configId":config["id"],"value":selected["value"]}),
        ));
        Ok(())
    }
}
impl LineDecoder for EventDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let frame: Value = serde_json::from_str(line)?;
        if let Some(method) = frame["method"].as_str() {
            return Ok(match method {
                "session/request_permission" => self.permission(&frame),
                "session/update" => self.update(&frame["params"]),
                _ if frame.get("id").is_some() => vec![DecodedEvent::WriteStdin(InputFrame(
                    json!({"jsonrpc":"2.0","id":frame["id"],"error":{"code":-32601,"message":"Method not supported by Nexus"}}),
                ))],
                _ => vec![],
            });
        }
        let Some(id) = frame["id"].as_str() else {
            return Ok(vec![]);
        };
        if !matches!(
            id,
            "initialize" | "session" | "mode" | "model" | "effort" | "prompt"
        ) {
            return Ok(vec![]);
        }
        if let Some(error) = frame.get("error") {
            return Ok(self.fail(if error["code"] == -32000 {
                format!("{} 需要认证，请先在终端完成 CLI 登录。", self.harness)
            } else {
                format!(
                    "{} ACP 请求 {id} 失败（代码 {}），请检查 CLI 配置。",
                    self.harness, error["code"]
                )
            }));
        }
        let result = &frame["result"];
        Ok(match id {
            "initialize" => {
                if result["protocolVersion"] != 1 {
                    self.fail("不支持的 ACP 协议版本。")
                } else {
                    let caps = &result["agentCapabilities"];
                    let method = if self.session.is_none() {
                        "session/new"
                    } else if caps["sessionCapabilities"].get("resume").is_some() {
                        "session/resume"
                    } else if caps["loadSession"] == true {
                        "session/load"
                    } else {
                        return Ok(self.fail("此 CLI 不支持恢复 ACP 会话，请升级 CLI。"));
                    };
                    let mut params = json!({"cwd":self.cwd,"mcpServers":[]});
                    if let Some(session) = &self.session {
                        params["sessionId"] = json!(session);
                    }
                    vec![DecodedEvent::WriteStdin(request("session", method, params))]
                }
            }
            "session" => self.setup(result),
            "mode" | "model" => {
                if result["configOptions"].is_array() {
                    self.config = result["configOptions"].clone();
                }
                if self.pending.is_empty()
                    && let Err(message) = self.effort_frame()
                {
                    return Ok(self.fail(message));
                }
                self.next()
            }
            "effort" => self.next(),
            "prompt" => {
                self.active = false;
                let mut events = vec![];
                self.flush(&mut events);
                if result["stopReason"] != "end_turn" {
                    events.push(DecodedEvent::Error(format!(
                        "{} 提前结束：{}",
                        self.harness, result["stopReason"]
                    )));
                }
                events.push(DecodedEvent::TurnCompleted);
                events
            }
            _ => vec![],
        })
    }
    // ACP v1 does not define steer; the runner retains the queued message for the next turn.
    fn steer(&mut self, _: &str, _: &str) -> Option<InputFrame> {
        None
    }
}
fn content_text(content: &Value) -> String {
    if content["type"] == "content" {
        content["content"]["text"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| tool_content(content))
    } else {
        tool_content(content)
    }
}
fn config_option<'a>(config: &'a Value, category: &str) -> Option<&'a Value> {
    config
        .as_array()?
        .iter()
        .find(|c| c["category"] == category && c["type"] == "select")
}
fn select_options(config: &Value) -> Vec<&Value> {
    config["options"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|o| {
            if let Some(group) = o["options"].as_array() {
                group.iter().collect()
            } else {
                vec![o]
            }
        })
        .collect()
}
fn effort(value: &str) -> Option<ThinkingEffort> {
    match value {
        "disabled" | "off" => Some(ThinkingEffort::Off),
        "enabled" => Some(ThinkingEffort::Default),
        _ => value.parse().ok(),
    }
}
fn catalog(harness: HarnessKind, result: &Value) -> Result<Vec<ModelDescriptor>, String> {
    let reasoning = config_option(&result["configOptions"], "thought_level");
    let efforts: Vec<_> = reasoning
        .map(select_options)
        .unwrap_or_default()
        .iter()
        .filter_map(|o| {
            Some(ModelReasoningEffort {
                effort: effort(o["value"].as_str()?)?,
                description: o["description"].as_str().unwrap_or("").into(),
            })
        })
        .collect();
    let model_config = config_option(&result["configOptions"], "model");
    let models = if let Some(config) = model_config {
        select_options(config)
    } else {
        result["models"]["availableModels"]
            .as_array()
            .into_iter()
            .flatten()
            .collect()
    };
    let current = model_config
        .map(|c| &c["currentValue"])
        .unwrap_or(&result["models"]["currentModelId"]);
    let models: Vec<_> = models
        .into_iter()
        .filter_map(|m| {
            let id = m.get("value").or_else(|| m.get("modelId"))?.as_str()?;
            let supports_reasoning = !result["models"]["availableModels"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|native| {
                    native["modelId"] == id && native["_meta"]["supportsReasoning"] == false
                });
            Some(ModelDescriptor {
                id: id.into(),
                display_name: m["name"].as_str().unwrap_or(id).into(),
                source: source(harness),
                availability: ModelAvailability::Available,
                provider: None,
                is_default: current == id,
                supported_reasoning_efforts: if supports_reasoning {
                    efforts.clone()
                } else {
                    vec![]
                },
                default_reasoning_effort: if supports_reasoning {
                    reasoning.and_then(|c| effort(c["currentValue"].as_str()?))
                } else {
                    None
                },
            })
        })
        .collect();
    if models.is_empty() {
        Err(format!("{harness} 未提供可用模型，请先配置 CLI。"))
    } else {
        Ok(models)
    }
}

pub async fn discover_models(
    harness: HarnessKind,
    executable: &str,
    cwd: &Path,
    environment: &[EnvironmentVariable],
    mut cancel: watch::Receiver<bool>,
) -> Result<Vec<ModelDescriptor>, ModelCatalogError> {
    if *cancel.borrow() {
        return Err(ModelCatalogError::Cancelled);
    }
    let executable = resolve_executable(executable)
        .ok_or_else(|| ModelCatalogError::Failed(format!("未找到 {harness} CLI。")))?;
    let mut child = Command::new(executable)
        .args(args(harness))
        .envs(environment.iter().map(|v| (&v.name, &v.value)))
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| ModelCatalogError::Failed(format!("无法启动 {harness} ACP。")))?;
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let collect = async {
        stdin
            .write_all(format!("{}\n", initialize().0).as_bytes())
            .await
            .map_err(|_| "无法初始化 ACP。")?;
        let mut config = Value::Null;
        while let Some(line) = lines.next_line().await.map_err(|_| "无法读取 ACP 输出。")? {
            let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let response = if frame["method"] == "session/update" {
                if frame["params"]["update"]["sessionUpdate"] == "config_option_update" {
                    config = frame["params"]["update"]["configOptions"].clone();
                }
                None
            } else if frame["method"].is_string() && frame.get("id").is_some() {
                Some(
                    json!({"jsonrpc":"2.0","id":frame["id"],"error":{"code":-32601,"message":"Method not supported"}}),
                )
            } else if matches!(frame["id"].as_str(), Some("initialize" | "catalog"))
                && frame.get("error").is_some()
            {
                return Err(format!(
                    "{harness} 模型目录不可用，请先完成 CLI 登录和配置。"
                ));
            } else if frame["id"] == "initialize" {
                if frame["result"]["protocolVersion"] != 1 {
                    return Err("不支持的 ACP 协议版本。".into());
                }
                Some(request("catalog", "session/new", json!({"cwd":cwd,"mcpServers":[]})).0)
            } else if frame["id"] == "catalog" {
                let mut result = frame["result"].clone();
                if !result["configOptions"].is_array() && config.is_array() {
                    result["configOptions"] = config;
                }
                return catalog(harness, &result);
            } else {
                None
            };
            if let Some(response) = response {
                stdin
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .map_err(|_| "无法发送 ACP 请求。")?;
            }
        }
        Err(format!("{harness} 未返回模型目录。"))
    };
    let result = tokio::select! {result=timeout(Duration::from_secs(30),collect)=>result.unwrap_or_else(|_|Err("ACP 模型目录超时。".into())).map_err(ModelCatalogError::Failed),_=cancel.changed()=>Err(ModelCatalogError::Cancelled)};
    drop(stdin);
    if timeout(Duration::from_secs(2), child.wait()).await.is_err() {
        let _ = child.kill().await;
    }
    result
}
pub async fn probe(harness: HarnessKind, executable: &str) -> HarnessProbe {
    let mut result = HarnessProbe {
        harness,
        executable: executable.into(),
        available: false,
        authenticated: false,
        version: None,
        message: format!("未找到 {harness} CLI。"),
    };
    let Some(path) = resolve_executable(executable) else {
        return result;
    };
    result.executable = path.to_string_lossy().into();
    if let Ok(Ok(output)) = timeout(
        Duration::from_secs(8),
        Command::new(path)
            .arg("--version")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
        && output.status.success()
    {
        result.available = true;
        result.version = Some(String::from_utf8_lossy(&output.stdout).trim().into());
        let (_sender, cancel) = watch::channel(false);
        result.authenticated = discover_models(
            harness,
            &result.executable,
            &std::env::current_dir().unwrap_or_else(|_| ".".into()),
            &[],
            cancel,
        )
        .await
        .is_ok();
        result.message = if result.authenticated {
            format!("{harness} ACP 已就绪，模型调用仍取决于账号权限。")
        } else {
            format!("{harness} 已安装，请先在 CLI 登录并配置模型。")
        };
    }
    result
}

pub fn prepare_kimi_text_generation(
    run: &TextGenerationConfig,
    prompt: &str,
    cwd: &Path,
) -> Result<(LaunchSpec, KimiTextDecoder), String> {
    let directory = tempfile::tempdir().map_err(|_| "无法创建 Kimi 文本生成配置。")?;
    std::fs::write(
        directory.path().join("system.md"),
        "Follow the requested output format. Return only the requested text.",
    )
    .map_err(|_| "无法写入 Kimi 文本生成提示。")?;
    std::fs::write(directory.path().join("agent.yaml"), "version: 1\nagent:\n  name: nexus-text\n  system_prompt_path: system.md\n  tools: []\n  subagents: {}\n").map_err(|_|"无法写入 Kimi 文本生成配置。")?;
    // Explicit empty MCP config also suppresses the user's global MCP configuration.
    std::fs::write(directory.path().join("mcp.json"), "{\"mcpServers\":{}}")
        .map_err(|_| "无法写入 Kimi 文本生成 MCP 配置。")?;
    let mut args: Vec<String> = [
        "--print",
        "--final-message-only",
        "--output-format",
        "text",
        "--agent-file",
    ]
    .map(Into::into)
    .to_vec();
    args.push(directory.path().join("agent.yaml").to_string_lossy().into());
    args.extend([
        "--work-dir".into(),
        directory.path().to_string_lossy().into(),
    ]);
    args.extend([
        "--mcp-config-file".into(),
        directory.path().join("mcp.json").to_string_lossy().into(),
    ]);
    if let Some(model) = &run.model {
        let (model, thinking) = model
            .strip_suffix(",thinking")
            .map(|m| (m, true))
            .unwrap_or((model, false));
        args.extend([
            "--model".into(),
            model.into(),
            if thinking {
                "--thinking"
            } else {
                "--no-thinking"
            }
            .into(),
        ]);
    }
    Ok((
        LaunchSpec {
            executable: run.executable.clone().into(),
            args,
            cwd: cwd.into(),
            stdin: prompt.into(),
        },
        KimiTextDecoder {
            _directory: directory,
            text: String::new(),
        },
    ))
}
pub struct KimiTextDecoder {
    _directory: tempfile::TempDir,
    text: String,
}
impl LineDecoder for KimiTextDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        self.text.push_str(line);
        self.text.push('\n');
        Ok(vec![DecodedEvent::MessageCompleted(self.text.clone())])
    }
    fn steer(&mut self, _: &str, _: &str) -> Option<InputFrame> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(harness: HarnessKind) -> StartRun {
        serde_json::from_value(json!({"run_id":"00000000-0000-0000-0000-000000000001","task_id":"00000000-0000-0000-0000-000000000002","harness":harness,"executable":harness.default_executable(),"cwd":"/project","prompt":"private prompt","effort":"default","permission_mode":"ask"})).unwrap()
    }
    fn decode(decoder: &mut EventDecoder, frame: Value) -> Vec<DecodedEvent> {
        decoder.decode_line(&frame.to_string()).unwrap()
    }
    fn response(id: &str, result: Value) -> Value {
        json!({"jsonrpc":"2.0","id":id,"result":result})
    }
    fn update(value: Value) -> Value {
        json!({"method":"session/update","params":{"sessionId":"saved","update":value}})
    }
    fn active(harness: HarnessKind, permission: PermissionMode) -> EventDecoder {
        let mut run = run(harness);
        run.permission_mode = permission;
        let (_, mut decoder) = prepare_run(&run, Path::new("/project"));
        decode(
            &mut decoder,
            response("session", json!({"sessionId":"saved"})),
        );
        decode(&mut decoder, response("mode", json!({})));
        decoder
    }
    #[test]
    fn initialization_restore_and_permission_setup_precede_prompt_and_ignore_replay() {
        for (caps, method) in [
            (
                json!({"sessionCapabilities":{"resume":{}}}),
                "session/resume",
            ),
            (json!({"loadSession":true}), "session/load"),
        ] {
            let mut run = run(HarnessKind::Kimi);
            run.session_id = Some("saved".into());
            let (spec, mut decoder) = prepare_run(&run, Path::new("/project"));
            assert!(!spec.stdin.contains("private prompt"));
            assert_eq!(
                serde_json::from_str::<Value>(spec.stdin.trim()).unwrap()["params"]["clientCapabilities"],
                json!({})
            );
            let events = decode(
                &mut decoder,
                response(
                    "initialize",
                    json!({"protocolVersion":1,"agentCapabilities":caps}),
                ),
            );
            assert!(
                matches!(&events[0],DecodedEvent::WriteStdin(f) if f.0["method"]==method && f.0["params"]["sessionId"]=="saved")
            );
            assert!(decode(&mut decoder,update(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"old message"}}))).is_empty());
            let events = decode(&mut decoder, response("session", Value::Null));
            assert!(
                matches!(&events[1],DecodedEvent::WriteStdin(f) if f.0["method"]=="session/set_mode" && f.0["params"]["modeId"]=="default")
            );
            let events = decode(&mut decoder, response("mode", json!({})));
            assert!(
                matches!(&events[0],DecodedEvent::WriteStdin(f) if f.0["method"]=="session/prompt")
            );
        }
    }
    #[test]
    fn permissions_preserve_native_ids_and_auto_edit_only_approves_edits() {
        let frame = json!({"jsonrpc":"2.0","id":17,"method":"session/request_permission","params":{"sessionId":"saved","toolCall":{"toolCallId":"call","title":"Edit file"},"options":[{"optionId":"once-id","name":"Once","kind":"allow_once"},{"optionId":"no-id","name":"Deny","kind":"reject_once"}]}});
        for (mode, kind, automatic) in [
            (PermissionMode::Ask, "edit", false),
            (PermissionMode::AutoEdit, "edit", true),
            (PermissionMode::AutoEdit, "execute", false),
            (PermissionMode::Yolo, "execute", true),
        ] {
            let mut decoder = active(HarnessKind::Kimi, mode);
            decode(
                &mut decoder,
                update(
                    json!({"sessionUpdate":"tool_call","toolCallId":"call","title":"Edit file","kind":kind}),
                ),
            );
            let events = decode(&mut decoder, frame.clone());
            if automatic {
                assert!(
                    matches!(&events[0],DecodedEvent::WriteStdin(f) if f.0["id"]==17 && f.0["result"]["outcome"]["optionId"]=="once-id")
                );
            } else {
                let DecodedEvent::ApprovalRequested(p) = &events[0] else {
                    panic!()
                };
                assert_eq!(
                    p.options[1].response.0["result"]["outcome"]["optionId"],
                    "no-id"
                );
                assert_eq!(p.cancel.0["result"]["outcome"]["outcome"], "cancelled");
            }
        }
    }
    #[test]
    fn streams_text_tools_and_terminal_failure_without_repeating_completed_tools() {
        let mut decoder = active(HarnessKind::Codebuddy, PermissionMode::Ask);
        decode(
            &mut decoder,
            update(
                json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Hello"}}),
            ),
        );
        let events = decode(
            &mut decoder,
            update(
                json!({"sessionUpdate":"tool_call","toolCallId":"1","title":"Read","rawInput":{"path":"file"}}),
            ),
        );
        assert!(matches!(&events[0],DecodedEvent::MessageCompleted(t) if t=="Hello"));
        let finished = update(
            json!({"sessionUpdate":"tool_call_update","toolCallId":"1","status":"failed","content":[{"type":"content","content":{"type":"text","text":"missing file"}}]}),
        );
        assert!(
            matches!(&decode(&mut decoder,finished.clone())[0],DecodedEvent::ToolCompleted{output,is_error:true,..} if output=="missing file")
        );
        assert!(decode(&mut decoder, finished).is_empty());
        let events = decode(
            &mut decoder,
            response("prompt", json!({"stopReason":"refusal"})),
        );
        assert!(matches!(
            events.as_slice(),
            [DecodedEvent::Error(_), DecodedEvent::TurnCompleted]
        ));
        let mut decoder = active(HarnessKind::Qoder, PermissionMode::Ask);
        let events = decode(
            &mut decoder,
            json!({"id":"prompt","error":{"code":-32000,"message":"token=private"}}),
        );
        assert!(matches!(
            events.as_slice(),
            [DecodedEvent::Error(_), DecodedEvent::TurnCompleted]
        ));
        assert!(!format!("{events:?}").contains("private"));
    }
    #[test]
    fn model_ids_and_grouped_config_options_round_trip_without_inventing_efforts() {
        let data = json!({"models":{"currentModelId":"kimi,thinking","availableModels":[{"modelId":"kimi,thinking","name":"Thinking"}]}});
        let models = catalog(HarnessKind::Kimi, &data).unwrap();
        assert_eq!(models[0].id, "kimi,thinking");
        assert!(models[0].supported_reasoning_efforts.is_empty());
        let config = json!([{"id":"model-choice","category":"model","type":"select","currentValue":"custom","options":[{"group":"provider","options":[{"value":"custom","name":"Custom"}]}]},{"id":"reason","category":"thought_level","type":"select","currentValue":"disabled","options":[{"value":"disabled","name":"Off"},{"value":"high","name":"High"}]}]);
        assert_eq!(
            catalog(HarnessKind::Codebuddy, &json!({"configOptions":config})).unwrap()[0]
                .supported_reasoning_efforts
                .len(),
            2
        );
        let mut run = run(HarnessKind::Codebuddy);
        run.model = Some("custom".into());
        run.effort = ThinkingEffort::Off;
        let (_, mut decoder) = prepare_run(&run, Path::new("/project"));
        decode(
            &mut decoder,
            response(
                "session",
                json!({"sessionId":"saved","configOptions":config}),
            ),
        );
        let events = decode(&mut decoder, response("mode", json!({})));
        assert!(
            matches!(&events[0],DecodedEvent::WriteStdin(f) if f.0["params"]["configId"]=="model-choice" && f.0["params"]["value"]=="custom")
        );
        let events = decode(&mut decoder, response("model", json!({})));
        assert!(
            matches!(&events[0],DecodedEvent::WriteStdin(f) if f.0["params"]["configId"]=="reason" && f.0["params"]["value"]=="disabled")
        );
    }
    #[test]
    fn kimi_title_agent_has_no_tools_or_mcp_and_never_resumes_conversation() {
        let mut run = run(HarnessKind::Kimi);
        run.session_id = Some("saved".into());
        run.model = Some("model,thinking".into());
        let (spec, mut decoder) = prepare_kimi_text_generation(
            &TextGenerationConfig {
                harness: run.harness,
                executable: run.executable.clone(),
                model: run.model.clone(),
                effort: run.effort,
                environment: vec![],
            },
            "title prompt",
            Path::new("/project"),
        )
        .unwrap();
        assert!(spec.args.windows(2).any(|p| p == ["--model", "model"]));
        assert!(spec.args.contains(&"--thinking".into()));
        assert!(!spec.args.contains(&"--session".into()));
        let path = &spec.args[spec.args.iter().position(|a| a == "--agent-file").unwrap() + 1];
        assert!(std::fs::read_to_string(path).unwrap().contains("tools: []"));
        assert!(
            matches!(&decoder.decode_line("Title").unwrap()[0],DecodedEvent::MessageCompleted(t) if t.trim()=="Title")
        );
    }
}
