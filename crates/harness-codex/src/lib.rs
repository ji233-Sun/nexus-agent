use std::{
    collections::HashSet,
    env,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use nexus_domain::{HarnessKind, ModelDescriptor, ModelReasoningEffort, ThinkingEffort};
pub use nexus_harness_core::{DecodedEvent, LaunchSpec, ModelCatalogError};
use nexus_harness_core::{
    InputFrame, LineDecoder, resolve_executable, summarize_text, tool_content,
};
use nexus_protocol::{EnvironmentVariable, HarnessProbe, StartRun};
use serde_json::{Map, Value, json};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::watch,
    time::timeout,
};

const MODEL_PAGE_LIMIT: u64 = 100;
const MODEL_MAX_PAGES: usize = 100;
const APP_SERVER_TIMEOUT: Duration = Duration::from_secs(10);

pub fn prepare_run(request: &StartRun, cwd: &Path) -> (LaunchSpec, EventDecoder) {
    let api_key = request
        .environment
        .iter()
        .find(|variable| variable.name == "CODEX_API_KEY")
        .cloned()
        .or_else(|| {
            env::var("CODEX_API_KEY")
                .ok()
                .map(|value| EnvironmentVariable {
                    name: "CODEX_API_KEY".into(),
                    value,
                })
        })
        .filter(|variable| !variable.value.trim().is_empty());
    let mut args = vec!["app-server".into()];
    if api_key.is_some() {
        // app-server does not read CODEX_API_KEY. Keep its API-key login in memory.
        args.extend([
            "--config".into(),
            "cli_auth_credentials_store=\"ephemeral\"".into(),
        ]);
    }
    (
        LaunchSpec {
            executable: PathBuf::from(&request.executable),
            args,
            cwd: cwd.to_path_buf(),
            stdin: format!(
                "{}\n",
                json!({"id": 0, "method": "initialize", "params": {
                    "clientInfo": {"name": "nexus_agent", "version": env!("CARGO_PKG_VERSION")}
                }})
            ),
        },
        EventDecoder {
            request: request.clone(),
            cwd: cwd.to_path_buf(),
            api_key,
            thread_id: None,
            turn_id: None,
        },
    )
}

pub fn build_title_launch_spec(
    executable: &str,
    cwd: &Path,
    prompt: &str,
    model: Option<&str>,
    effort: ThinkingEffort,
) -> LaunchSpec {
    let mut args = vec![
        "exec".into(),
        "--ignore-rules".into(),
        "--skip-git-repo-check".into(),
        "--json".into(),
        "--sandbox".into(),
        "read-only".into(),
        "--ephemeral".into(),
        "--color".into(),
        "never".into(),
    ];
    if !effort.is_default() {
        args.extend([
            "--config".into(),
            format!("model_reasoning_effort=\"{}\"", effort.as_str()),
        ]);
    }
    if let Some(model) = model {
        args.extend(["--model".into(), model.into()]);
    }
    args.push("-".into());

    LaunchSpec {
        executable: PathBuf::from(executable),
        args,
        cwd: cwd.to_path_buf(),
        stdin: prompt.to_owned(),
    }
}

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
            "未找到 Codex CLI，无法加载模型目录。请检查可执行文件路径。".into(),
        )
    })?;
    let mut server = AppServer::spawn(&executable, cwd, environment)?;
    let result = {
        let query = async {
            server
                .request(
                    "initialize",
                    json!({
                        "clientInfo": {
                            "name": "nexus-agent",
                            "title": "Nexus Agent",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }),
                )
                .await?;
            server.notify("initialized", json!({})).await?;
            list_all_models(&mut server).await
        };
        tokio::pin!(query);
        tokio::select! {
            result = &mut query => result,
            _ = cancel.changed() => Err(ModelCatalogError::Cancelled),
        }
    };
    server.shutdown().await;
    result
}

struct AppServer {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl AppServer {
    fn spawn(
        executable: &Path,
        cwd: &Path,
        environment: &[EnvironmentVariable],
    ) -> Result<Self, ModelCatalogError> {
        let mut child = Command::new(executable)
            .args(["app-server", "--listen", "stdio://"])
            .envs(
                environment
                    .iter()
                    .map(|variable| (&variable.name, &variable.value)),
            )
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| {
                ModelCatalogError::Failed(
                    "无法启动 Codex App Server。请检查 CLI 版本和可执行文件权限。".into(),
                )
            })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            ModelCatalogError::Failed("无法连接 Codex App Server 输入流。".into())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ModelCatalogError::Failed("无法连接 Codex App Server 输出流。".into())
        })?;
        Ok(Self {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            next_id: 0,
        })
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), ModelCatalogError> {
        self.write_frame(&json!({ "method": method, "params": params }))
            .await
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, ModelCatalogError> {
        let id = self.next_id;
        self.next_id += 1;
        self.write_frame(&json!({ "id": id, "method": method, "params": params }))
            .await?;
        timeout(APP_SERVER_TIMEOUT, self.read_response(id, method))
            .await
            .map_err(|_| {
                ModelCatalogError::Failed(format!(
                    "Codex App Server 的 {method} 请求超时，请重试。"
                ))
            })?
    }

    async fn read_response(&mut self, id: u64, method: &str) -> Result<Value, ModelCatalogError> {
        loop {
            let mut line = String::new();
            let read = self.stdout.read_line(&mut line).await.map_err(|_| {
                ModelCatalogError::Failed(format!("读取 Codex App Server {method} 响应失败。"))
            })?;
            if read == 0 {
                return Err(ModelCatalogError::Failed(format!(
                    "Codex App Server 在返回 {method} 前退出，当前 CLI 可能不支持模型目录。"
                )));
            }
            let frame: Value = serde_json::from_str(line.trim()).map_err(|_| {
                ModelCatalogError::Failed(format!(
                    "Codex App Server 的 {method} 响应不是有效 JSON。"
                ))
            })?;
            if frame.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(message) = frame.pointer("/error/message").and_then(Value::as_str) {
                return Err(ModelCatalogError::Failed(format!(
                    "Codex App Server {method} 失败：{}",
                    summarize_text(message)
                )));
            }
            return frame.get("result").cloned().ok_or_else(|| {
                ModelCatalogError::Failed(format!("Codex App Server 的 {method} 响应缺少 result。"))
            });
        }
    }

    async fn write_frame(&mut self, frame: &Value) -> Result<(), ModelCatalogError> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| ModelCatalogError::Failed("Codex App Server 输入流已关闭。".into()))?;
        let mut encoded = serde_json::to_vec(frame)
            .map_err(|_| ModelCatalogError::Failed("无法编码 Codex App Server 请求。".into()))?;
        encoded.push(b'\n');
        stdin
            .write_all(&encoded)
            .await
            .map_err(|_| ModelCatalogError::Failed("写入 Codex App Server 请求失败。".into()))?;
        stdin
            .flush()
            .await
            .map_err(|_| ModelCatalogError::Failed("刷新 Codex App Server 请求失败。".into()))
    }

    async fn shutdown(mut self) {
        self.stdin.take();
        if matches!(
            timeout(Duration::from_millis(500), self.child.wait()).await,
            Ok(Ok(_))
        ) {
            return;
        }
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

async fn list_all_models(
    server: &mut AppServer,
) -> Result<Vec<ModelDescriptor>, ModelCatalogError> {
    let mut models = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen_cursors = HashSet::new();
    for _ in 0..MODEL_MAX_PAGES {
        let mut params = Map::new();
        params.insert("limit".into(), Value::from(MODEL_PAGE_LIMIT));
        params.insert("includeHidden".into(), Value::Bool(false));
        if let Some(cursor) = &cursor {
            params.insert("cursor".into(), Value::String(cursor.clone()));
        }
        let result = server.request("model/list", Value::Object(params)).await?;
        let (mut page, next_cursor) = parse_model_page(&result)?;
        models.append(&mut page);
        let Some(next_cursor) = next_cursor else {
            return Ok(models);
        };
        if next_cursor.is_empty() || !seen_cursors.insert(next_cursor.clone()) {
            return Err(ModelCatalogError::Failed(
                "Codex App Server 返回了重复的模型目录游标。".into(),
            ));
        }
        cursor = Some(next_cursor);
    }
    Err(ModelCatalogError::Failed(
        "Codex App Server 模型目录分页超过安全上限。".into(),
    ))
}

fn parse_model_page(
    result: &Value,
) -> Result<(Vec<ModelDescriptor>, Option<String>), ModelCatalogError> {
    let data = result
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| ModelCatalogError::Failed("Codex model/list 响应缺少 data。".into()))?;
    let mut models = Vec::with_capacity(data.len());
    for item in data {
        if item.get("hidden").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| item.get("model").and_then(Value::as_str))
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                ModelCatalogError::Failed("Codex model/list 返回了缺少 ID 的模型。".into())
            })?
            .to_owned();
        let display_name = item
            .get("displayName")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or(&id)
            .to_owned();
        let mut supported_reasoning_efforts = Vec::new();
        for option in item
            .get("supportedReasoningEfforts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let value = option
                .get("reasoningEffort")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ModelCatalogError::Failed(
                        "Codex model/list 返回了缺少 reasoningEffort 的能力项。".into(),
                    )
                })?;
            let effort = value.parse().map_err(ModelCatalogError::Failed)?;
            supported_reasoning_efforts.push(ModelReasoningEffort {
                effort,
                description: option
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            });
        }
        let default_reasoning_effort = item
            .get("defaultReasoningEffort")
            .and_then(Value::as_str)
            .map(str::parse)
            .transpose()
            .map_err(ModelCatalogError::Failed)?;
        models.push(ModelDescriptor {
            id,
            display_name,
            source: nexus_domain::ModelSource::CodexAppServer,
            availability: nexus_domain::ModelAvailability::Available,
            provider: None,
            is_default: item
                .get("isDefault")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            supported_reasoning_efforts,
            default_reasoning_effort,
        });
    }
    let next_cursor = result
        .get("nextCursor")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok((models, next_cursor))
}

pub async fn probe(configured_executable: &str) -> HarnessProbe {
    let executable = resolve_executable(configured_executable);
    let Some(executable) = executable else {
        return HarnessProbe {
            harness: HarnessKind::Codex,
            available: false,
            authenticated: false,
            executable: configured_executable.to_owned(),
            version: None,
            message: "未找到 Codex CLI。请安装后在设置中填写 codex 可执行文件路径。".into(),
        };
    };

    let version = Command::new(&executable).arg("--version").output().await;
    let Ok(version) = version else {
        return HarnessProbe {
            harness: HarnessKind::Codex,
            available: false,
            authenticated: false,
            executable: executable.display().to_string(),
            version: None,
            message: "Codex CLI 存在，但无法执行。请检查文件权限。".into(),
        };
    };
    if !version.status.success() {
        return HarnessProbe {
            harness: HarnessKind::Codex,
            available: false,
            authenticated: false,
            executable: executable.display().to_string(),
            version: None,
            message: "Codex CLI 版本探测失败。".into(),
        };
    }
    let version = String::from_utf8_lossy(&version.stdout).trim().to_owned();

    let auth = Command::new(&executable)
        .args(["login", "status"])
        .output()
        .await;
    let authenticated = auth.is_ok_and(|output| output.status.success())
        || env::var_os("CODEX_API_KEY").is_some_and(|value| !value.is_empty());

    HarnessProbe {
        harness: HarnessKind::Codex,
        available: true,
        authenticated,
        executable: executable.display().to_string(),
        version: Some(version),
        message: if authenticated {
            "Codex CLI 已就绪".into()
        } else {
            "Codex CLI 尚未登录，请在终端运行 `codex login`。".into()
        },
    }
}

pub struct EventDecoder {
    request: StartRun,
    cwd: PathBuf,
    api_key: Option<EnvironmentVariable>,
    thread_id: Option<String>,
    turn_id: Option<String>,
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
}

impl EventDecoder {
    fn start_thread(&self) -> DecodedEvent {
        let mut params =
            json!({"cwd": self.cwd, "approvalPolicy": "never", "sandbox": "workspace-write"});
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
            return vec![DecodedEvent::WriteStdin(InputFrame(
                json!({"id": frame["id"],
                    "error": {"code": -32601, "message": "Nexus does not support interactive requests."}
                }),
            ))];
        }
        if let Some(id) = frame.get("id").and_then(Value::as_str) {
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
            Some("item/started") => decode_started_item(&params["item"]),
            Some("item/completed") => decode_completed_item(&params["item"]),
            Some("item/agentMessage/delta") => params
                .get("delta")
                .and_then(Value::as_str)
                .map(|text| vec![DecodedEvent::TextDelta(text.into())])
                .unwrap_or_default(),
            Some("turn/completed") => {
                let mut events = Vec::new();
                if params.pointer("/turn/status").and_then(Value::as_str) != Some("completed") {
                    events.push(DecodedEvent::Error(
                        params
                            .pointer("/turn/error/message")
                            .and_then(Value::as_str)
                            .map(summarize_text)
                            .unwrap_or_else(|| "Codex 轮次已中断。".into()),
                    ));
                }
                events.push(DecodedEvent::TurnCompleted);
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

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_domain::ThinkingEffort;

    fn request() -> StartRun {
        StartRun {
            run_id: Default::default(),
            task_id: Default::default(),
            session_id: None,
            cwd: "/tmp/project".into(),
            prompt: "secret prompt".into(),
            harness: HarnessKind::Codex,
            executable: "codex".into(),
            model: None,
            effort: ThinkingEffort::Default,
            environment: vec![EnvironmentVariable {
                name: "CODEX_API_KEY".into(),
                value: String::new(),
            }],
        }
    }

    fn input(events: &[DecodedEvent]) -> &Value {
        match events.last().unwrap() {
            DecodedEvent::WriteStdin(frame) => &frame.0,
            event => panic!("expected stdin frame, got {event:?}"),
        }
    }

    fn decoder() -> EventDecoder {
        let (_, mut decoder) = prepare_run(&request(), Path::new("/tmp/project"));
        decoder
            .decode_line(r#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#)
            .unwrap();
        decoder
            .decode_line(r#"{"id":3,"result":{"turn":{"id":"turn-1"}}}"#)
            .unwrap();
        decoder
    }

    #[test]
    fn launch_spec_preserves_permissions_model_effort_and_session_resume() {
        for session_id in [None, Some("existing-thread")] {
            let mut request = request();
            request.session_id = session_id.map(str::to_owned);
            request.model = Some("gpt-test".into());
            request.effort = ThinkingEffort::XHigh;
            let (spec, mut decoder) = prepare_run(&request, Path::new(&request.cwd));
            assert_eq!(spec.args, ["app-server"]);
            assert!(!spec.args.iter().any(|arg| arg.contains(&request.prompt)));
            assert_eq!(
                serde_json::from_str::<Value>(&spec.stdin).unwrap()["method"],
                "initialize"
            );
            let events = decoder.decode_line(r#"{"id":0,"result":{}}"#).unwrap();
            let frame = input(&events);
            assert_eq!(
                frame["method"],
                if session_id.is_some() {
                    "thread/resume"
                } else {
                    "thread/start"
                }
            );
            assert_eq!(frame["params"]["cwd"], request.cwd);
            assert_eq!(frame["params"]["approvalPolicy"], "never");
            assert_eq!(frame["params"]["sandbox"], "workspace-write");
            assert_eq!(frame["params"]["model"], "gpt-test");
            if let Some(id) = session_id {
                assert_eq!(frame["params"]["threadId"], id);
            }
            let events = decoder
                .decode_line(r#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#)
                .unwrap();
            assert_eq!(events[0], DecodedEvent::SessionStarted("thread-1".into()));
            let frame = input(&events);
            assert_eq!(frame["method"], "turn/start");
            assert_eq!(frame["params"]["input"][0]["text"], request.prompt);
            assert_eq!(frame["params"]["effort"], "xhigh");
        }
    }

    #[test]
    fn title_launch_spec_uses_read_only_sandbox() {
        let spec = build_title_launch_spec(
            "/usr/local/bin/codex",
            Path::new("/tmp/project"),
            "title prompt",
            Some("gpt-test"),
            ThinkingEffort::Low,
        );

        assert!(
            spec.args
                .windows(2)
                .any(|pair| pair == ["--sandbox", "read-only"])
        );
        assert!(spec.args.iter().any(|arg| arg == "--ignore-rules"));
        assert!(!spec.args.iter().any(|arg| arg == "workspace-write"));
        assert!(!spec.args.iter().any(|arg| arg.contains("title prompt")));
    }

    #[test]
    fn launch_spec_omits_cli_overrides_when_following_defaults() {
        let (spec, mut decoder) = prepare_run(&request(), Path::new("/tmp/project"));
        assert_eq!(spec.args, ["app-server"]);
        let events = decoder.decode_line(r#"{"id":0,"result":{}}"#).unwrap();
        assert!(input(&events)["params"].get("model").is_none());
        let events = decoder
            .decode_line(r#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#)
            .unwrap();
        assert!(input(&events)["params"].get("effort").is_none());
    }

    #[test]
    fn api_key_login_is_ephemeral_and_redacted() {
        let mut request = request();
        request.environment[0].value = "test-secret".into();
        let (spec, mut decoder) = prepare_run(&request, Path::new(&request.cwd));
        assert!(
            spec.args
                .windows(2)
                .any(|pair| pair == ["--config", "cli_auth_credentials_store=\"ephemeral\""])
        );
        let events = decoder.decode_line(r#"{"id":0,"result":{}}"#).unwrap();
        assert_eq!(input(&events)["method"], "account/login/start");
        assert_eq!(input(&events)["params"]["apiKey"], "test-secret");
        assert!(!format!("{events:?}").contains("test-secret"));
        let events = decoder
            .decode_line(r#"{"id":1,"error":{"message":"test-secret"}}"#)
            .unwrap();
        assert!(!format!("{events:?}").contains("test-secret"));
        assert!(matches!(
            events.as_slice(),
            [DecodedEvent::Error(_), DecodedEvent::TurnCompleted]
        ));
    }

    #[test]
    fn model_page_preserves_ids_defaults_and_all_effort_values() {
        let result = serde_json::json!({
            "data": [
                {
                    "id": "gpt-visible",
                    "displayName": "GPT Visible",
                    "hidden": false,
                    "isDefault": true,
                    "defaultReasoningEffort": "low",
                    "supportedReasoningEfforts": [
                        {"reasoningEffort": "low", "description": "Fast"},
                        {"reasoningEffort": "ultra", "description": "Ultra"}
                    ]
                },
                {
                    "id": "gpt-hidden",
                    "displayName": "GPT Hidden",
                    "hidden": true,
                    "supportedReasoningEfforts": []
                }
            ],
            "nextCursor": "next-page"
        });

        let (models, cursor) = parse_model_page(&result).unwrap();

        assert_eq!(cursor.as_deref(), Some("next-page"));
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-visible");
        assert_eq!(models[0].display_name, "GPT Visible");
        assert_eq!(models[0].provider, None);
        assert!(models[0].is_default);
        assert_eq!(
            models[0].default_reasoning_effort,
            Some(ThinkingEffort::Low)
        );
        assert_eq!(
            models[0].supported_reasoning_efforts[1].effort,
            ThinkingEffort::Ultra
        );
    }

    #[test]
    fn decoder_maps_messages_and_command_events() {
        let mut decoder = decoder();
        let started = r#"{"method":"item/started","params":{"item":{"id":"item_1","type":"commandExecution","command":"cargo test","status":"inProgress"}}}"#;
        assert!(matches!(decoder.decode_line(started).unwrap().as_slice(),
            [DecodedEvent::ToolStarted { id, name, summary }]
                if id == "item_1" && name == "Command" && summary == "cargo test"));
        let completed = r#"{"method":"item/completed","params":{"item":{"id":"item_1","type":"commandExecution","aggregatedOutput":"ok","exitCode":0,"status":"completed"}}}"#;
        assert_eq!(
            decoder.decode_line(completed).unwrap(),
            vec![DecodedEvent::ToolCompleted {
                id: "item_1".into(),
                output: "ok".into(),
                is_error: false,
            }]
        );
        let message = r#"{"method":"item/completed","params":{"item":{"id":"item_2","type":"agentMessage","text":"done"}}}"#;
        assert_eq!(
            decoder.decode_line(message).unwrap(),
            vec![DecodedEvent::MessageCompleted("done".into())]
        );

        let long_text = "完整命令和输出\n".repeat(100);
        let started = json!({"method": "item/started", "params": {"item": {
            "id": "long", "type": "commandExecution", "command": long_text
        }}});
        assert!(
            matches!(decoder.decode_line(&started.to_string()).unwrap().as_slice(),
            [DecodedEvent::ToolStarted { summary, .. }] if summary == &long_text)
        );
        let completed = json!({"method": "item/completed", "params": {"item": {
            "id": "long", "type": "commandExecution", "aggregatedOutput": long_text,
            "status": "completed", "exitCode": 1
        }}});
        assert!(
            matches!(decoder.decode_line(&completed.to_string()).unwrap().as_slice(),
            [DecodedEvent::ToolCompleted { output, is_error: true, .. }] if output == &long_text)
        );
        let item = json!({"id": "edit", "type": "fileChange", "status": "completed",
            "changes": [{"kind": "update", "path": "main.rs", "diff": long_text}]
        });
        let started = json!({"method": "item/started", "params": {"item": item}});
        assert!(
            matches!(decoder.decode_line(&started.to_string()).unwrap().as_slice(),
            [DecodedEvent::ToolStarted { summary, .. }]
                if serde_json::from_str::<Value>(summary).unwrap() == item)
        );
        let completed = json!({"method": "item/completed", "params": {"item": item}});
        assert!(matches!(
            decoder
                .decode_line(&completed.to_string())
                .unwrap()
                .as_slice(),
            [DecodedEvent::ToolCompleted {
                is_error: false,
                ..
            }]
        ));
    }

    #[test]
    fn steer_targets_the_current_turn_and_matches_native_receipts() {
        let (_, mut unstarted) = prepare_run(&request(), Path::new("/tmp/project"));
        assert!(unstarted.steer("message-1", "update").is_none());
        assert!(
            unstarted
                .decode_line(r#"{"id":"message-1","result":{}}"#)
                .unwrap()
                .is_empty()
        );
        let mut decoder = decoder();
        let frame = decoder.steer("message-1", "update\n第二行").unwrap();
        assert_eq!(frame.0["method"], "turn/steer");
        assert_eq!(frame.0["params"]["threadId"], "thread-1");
        assert_eq!(frame.0["params"]["expectedTurnId"], "turn-1");
        assert_eq!(frame.0["params"]["clientUserMessageId"], "message-1");
        assert_eq!(frame.0["params"]["input"][0]["text"], "update\n第二行");
        assert_eq!(
            decoder
                .decode_line(r#"{"id":"message-1","result":{"turnId":"turn-1"}}"#)
                .unwrap(),
            vec![DecodedEvent::InputAccepted("message-1".into())]
        );
        assert!(
            decoder
                .decode_line(r#"{"id":"message-1","result":{"turnId":"old-turn"}}"#)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            decoder
                .decode_line(r#"{"id":"message-1","error":{"message":"turn ended"}}"#)
                .unwrap(),
            vec![DecodedEvent::InputRejected {
                id: "message-1".into(),
                message: "turn ended".into()
            }]
        );
    }

    #[test]
    fn malformed_frames_are_recoverable_and_only_current_turn_is_terminal() {
        let mut decoder = decoder();
        assert!(decoder.decode_line("not json").is_err());
        assert!(decoder.decode_line(r#"{"method":"turn/completed","params":{"threadId":"other-thread","turn":{"id":"turn-1","status":"completed"}}}"#).unwrap().is_empty());
        assert!(decoder.decode_line(r#"{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"old-turn","status":"completed"}}}"#).unwrap().is_empty());
        assert_eq!(decoder.decode_line(r#"{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"failed","error":{"message":"denied"}}}}"#).unwrap(),
            vec![DecodedEvent::Error("denied".into()), DecodedEvent::TurnCompleted]);
        assert_eq!(decoder.decode_line(r#"{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}}"#).unwrap(),
            vec![DecodedEvent::TurnCompleted]);
    }
}
