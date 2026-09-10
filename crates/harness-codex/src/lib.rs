use std::{
    collections::{HashMap, HashSet},
    env,
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
            approval_items: HashMap::new(),
            pending_user_asks: HashMap::new(),
            seen_async_asks: HashSet::new(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_domain::ThinkingEffort;

    fn request() -> StartRun {
        StartRun {
            transport: nexus_domain::HarnessTransport::Acp,
            title_generation: None,
            permission_mode: nexus_domain::PermissionMode::AutoEdit,
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
        for (mode, sandbox, policy) in [
            (PermissionMode::Ask, "read-only", "on-request"),
            (PermissionMode::AutoEdit, "workspace-write", "on-request"),
            (PermissionMode::Yolo, "danger-full-access", "never"),
        ] {
            for session_id in [None, Some("existing-session".into())] {
                let mut request = request();
                request.permission_mode = mode;
                request.session_id = session_id;
                let (_, mut decoder) = prepare_run(&request, Path::new(&request.cwd));
                let events = decoder.decode_line(r#"{"id":0,"result":{}}"#).unwrap();
                let frame = input(&events);
                assert_eq!(frame["params"]["sandbox"], sandbox);
                assert_eq!(frame["params"]["approvalPolicy"], policy);
            }
        }
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
            assert_eq!(frame["params"]["approvalPolicy"], "on-request");
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
    fn approvals_preserve_rpc_ids_scope_and_file_change_details() {
        let mut decoder = decoder();
        decoder.decode_line(r#"{"method":"item/started","params":{"item":{"id":"patch","type":"fileChange","changes":[{"path":"src/lib.rs","diff":"+new content"}]}}}"#).unwrap();
        for id in [json!(17), json!("approval-17")] {
            for method in [
                "item/commandExecution/requestApproval",
                "item/fileChange/requestApproval",
                "item/permissions/requestApproval",
            ] {
                let frame = json!({"id": id, "method": method, "params": {"threadId":"thread-1", "turnId":"turn-1", "itemId":"patch", "command":"echo test", "cwd":"/tmp/project", "reason":"needs access", "permissions":{"network":{"enabled":true}}}});
                let events = decoder.decode_line(&frame.to_string()).unwrap();
                let [DecodedEvent::ApprovalRequested(prompt)] = events.as_slice() else {
                    panic!("expected approval")
                };
                assert_eq!(prompt.options[0].response.0["id"], id);
                assert!(prompt.details.contains("src/lib.rs"));
                assert!(prompt.details.contains("needs access"));
                if method.contains("permissions") {
                    assert_eq!(
                        prompt.options[0].response.0["result"]["permissions"],
                        frame["params"]["permissions"]
                    );
                    assert_eq!(prompt.options[0].response.0["result"]["scope"], "turn");
                    assert_eq!(prompt.cancel.0["result"]["permissions"], json!({}));
                } else {
                    assert_eq!(prompt.options[0].response.0["result"]["decision"], "accept");
                    assert_eq!(
                        prompt.options[1].response.0["result"]["decision"],
                        "decline"
                    );
                }
                let resolved = json!({"method":"serverRequest/resolved","params":{"threadId":"thread-1","requestId":id}});
                assert_eq!(
                    decoder.decode_line(&resolved.to_string()).unwrap(),
                    vec![DecodedEvent::ApprovalResolved(prompt.id.clone())]
                );
            }
        }
        let events = decoder.decode_line(r#"{"id":18,"method":"item/commandExecution/requestApproval","params":{"threadId":"other","turnId":"turn-1"}}"#).unwrap();
        assert!(input(&events).get("error").is_some());
        let events = decoder.decode_line(r#"{"id":19,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-1","turnId":"turn-1","availableDecisions":["decline","cancel"]}}"#).unwrap();
        let [DecodedEvent::ApprovalRequested(prompt)] = events.as_slice() else {
            panic!("expected approval")
        };
        assert_eq!(prompt.options.len(), 1);
        assert_eq!(prompt.options[0].label, "Deny");
    }

    #[test]
    fn user_input_maps_questions_and_preserves_numeric_and_string_rpc_ids() {
        let mut decoder = decoder();
        let questions = json!([
            {
                "id": "path-kind",
                "question": "Choose exactly one:\n路径",
                "isOther": true,
                "options": [
                    {"label": "Fast", "description": "Quick \"path\""},
                    {"label": "Safe", "description": "保守"}
                ]
            },
            {
                "id": "details",
                "question": "Exact details?",
                "options": null
            }
        ]);

        for id in [json!(27), json!("27")] {
            let frame = json!({
                "id": id,
                "method": "item/tool/requestUserInput",
                "params": {"threadId": "thread-1", "turnId": "turn-1", "questions": questions}
            });
            let events = decoder.decode_line(&frame.to_string()).unwrap();
            let [DecodedEvent::UserAskRequested(request)] = events.as_slice() else {
                panic!("expected user ask")
            };
            assert_eq!(request.native_request_id, id.to_string());
            assert_eq!(request.timeout_ms, None);
            assert!(request.resolve_on_send);
            assert_eq!(request.questions[0].id, "path-kind");
            assert_eq!(request.questions[0].prompt, "Choose exactly one:\n路径");
            assert_eq!(request.questions[0].options[0].label, "Fast");
            assert_eq!(
                request.questions[0].options[0].description.as_deref(),
                Some("Quick \"path\"")
            );
            assert_eq!(
                request.questions[0].answer_mode,
                UserAskAnswerMode::Choice {
                    multiple: false,
                    allow_custom: true
                }
            );
            assert_eq!(request.questions[1].answer_mode, UserAskAnswerMode::Text);

            let response = decoder
                .answer_user_ask(
                    &id.to_string(),
                    &[
                        UserAskAnswer {
                            question_id: "path-kind".into(),
                            value: UserAskAnswerValue::Selected(vec!["Safe".into()]),
                        },
                        UserAskAnswer {
                            question_id: "details".into(),
                            value: UserAskAnswerValue::Text("line 1\n\"原样\"".into()),
                        },
                    ],
                )
                .unwrap();
            assert_eq!(
                response.0,
                json!({"id": id, "result": {"answers": {
                    "path-kind": {"answers": ["Safe"]},
                    "details": {"answers": ["line 1\n\"原样\""]}
                }}})
            );
            assert!(decoder.answer_user_ask(&id.to_string(), &[]).is_none());
        }
    }

    #[test]
    fn user_input_rejects_wrong_scope_and_resolved_requests_cannot_be_answered() {
        let mut decoder = decoder();
        let out_of_scope = json!({
            "id": "ask-out",
            "method": "item/tool/requestUserInput",
            "params": {"threadId": "other", "turnId": "turn-1", "questions": [
                {"id": "q", "question": "Question?", "options": []}
            ]}
        });
        let events = decoder.decode_line(&out_of_scope.to_string()).unwrap();
        assert_eq!(input(&events)["id"], "ask-out");
        assert!(input(&events).get("error").is_some());
        assert!(decoder.answer_user_ask("\"ask-out\"", &[]).is_none());

        let ask = json!({
            "id": "ask-live",
            "method": "item/tool/requestUserInput",
            "params": {"threadId": "thread-1", "turnId": "turn-1", "questions": [
                {"id": "q", "question": "Question?"}
            ]}
        });
        decoder.decode_line(&ask.to_string()).unwrap();
        let resolved = json!({
            "method": "serverRequest/resolved",
            "params": {"threadId": "thread-1", "turnId": "turn-1", "requestId": "ask-live"}
        });
        assert_eq!(
            decoder.decode_line(&resolved.to_string()).unwrap(),
            vec![DecodedEvent::UserAskFinished {
                native_request_id: "\"ask-live\"".into(),
                status: UserAskStatus::Cancelled,
                message: None,
            }]
        );
        assert!(
            decoder
                .answer_user_ask(
                    "\"ask-live\"",
                    &[UserAskAnswer {
                        question_id: "q".into(),
                        value: UserAskAnswerValue::Text("late".into()),
                    }],
                )
                .is_none()
        );
    }

    fn async_question(id: &str) -> Value {
        json!({"method": "item/completed", "params": {
            "threadId": "thread-1", "turnId": "turn-1", "item": {
                "id": id, "type": "agentMessage", "delivery": "async",
                "phase": "final_answer", "text": "Choose a skill?\n- 外语\n- 乐器",
                "questions": [
                    {"title": "Choose a skill?", "options": ["外语", "乐器"]},
                    {"title": "Any details?"}
                ]
            }
        }})
    }

    #[test]
    fn async_user_input_survives_turn_completion_and_waits_for_answer_receipt() {
        for completed in [false, true] {
            let mut decoder = decoder();
            let frame = async_question("call-ask");
            let events = decoder.decode_line(&frame.to_string()).unwrap();
            let [DecodedEvent::UserAskRequested(request)] = events.as_slice() else {
                panic!("expected structured async User Ask, got {events:?}")
            };
            assert!(!request.resolve_on_send);
            assert_eq!(request.questions.len(), 2);
            assert_eq!(request.questions[0].prompt, "Choose a skill?");
            assert_eq!(
                request.questions[0].answer_mode,
                UserAskAnswerMode::Choice {
                    multiple: false,
                    allow_custom: true,
                }
            );
            assert_eq!(request.questions[1].answer_mode, UserAskAnswerMode::Text);
            assert!(decoder.decode_line(&frame.to_string()).unwrap().is_empty());
            if completed {
                let events = decoder.decode_line(r#"{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}}"#).unwrap();
                assert!(!events.contains(&DecodedEvent::TurnCompleted));
            }
            let answers = [
                UserAskAnswer {
                    question_id: request.questions[0].id.clone(),
                    value: if completed {
                        UserAskAnswerValue::Text("做菜".into())
                    } else {
                        UserAskAnswerValue::Selected(vec![
                            request.questions[0].options[1].id.clone(),
                        ])
                    },
                },
                UserAskAnswer {
                    question_id: request.questions[1].id.clone(),
                    value: UserAskAnswerValue::Text("每天\n半小时".into()),
                },
            ];
            let response = decoder
                .answer_user_ask(&request.native_request_id, &answers)
                .unwrap()
                .0;
            assert_eq!(response["method"], "turn/start");
            assert_eq!(response["params"]["threadId"], "thread-1");
            assert_eq!(
                response["params"]["input"][0]["text"],
                format!(
                    "User Ask answers:\n\nChoose a skill?\n{}\n\nAny details?\n每天\n半小时",
                    if completed { "做菜" } else { "乐器" }
                )
            );
            assert!(
                decoder
                    .answer_user_ask(&request.native_request_id, &answers)
                    .is_none()
            );
            let receipt = json!({"id": response["id"], "result": {"turn": {"id": "turn-2"}}});
            assert_eq!(
                decoder.decode_line(&receipt.to_string()).unwrap(),
                vec![DecodedEvent::UserAskFinished {
                    native_request_id: request.native_request_id.clone(),
                    status: UserAskStatus::Answered,
                    message: None,
                }]
            );
            assert_eq!(
                decoder.steer("message", "continue").unwrap().0["params"]["expectedTurnId"],
                "turn-2"
            );
            assert!(decoder.decode_line(&frame.to_string()).unwrap().is_empty());
            assert_eq!(decoder.decode_line(r#"{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-2","status":"completed"}}}"#).unwrap(), vec![DecodedEvent::TurnCompleted]);
        }
    }

    #[test]
    fn async_user_input_rejects_foreign_turns_and_reports_failed_delivery() {
        let mut decoder = decoder();
        let mut frame = async_question("call-ask");
        frame["params"]["turnId"] = "old-turn".into();
        assert!(decoder.decode_line(&frame.to_string()).unwrap().is_empty());
        frame["params"]["turnId"] = "turn-1".into();
        let events = decoder.decode_line(&frame.to_string()).unwrap();
        let [DecodedEvent::UserAskRequested(request)] = events.as_slice() else {
            panic!("expected User Ask")
        };
        let answers = request
            .questions
            .iter()
            .map(|question| UserAskAnswer {
                question_id: question.id.clone(),
                value: UserAskAnswerValue::Text("custom".into()),
            })
            .collect::<Vec<_>>();
        let response = decoder
            .answer_user_ask(&request.native_request_id, &answers)
            .unwrap()
            .0;
        let events = decoder.decode_line(&json!({"id": response["id"], "error": {"code": -32600, "message": "Cannot accept input"}}).to_string()).unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            DecodedEvent::UserAskFinished {
                status: UserAskStatus::Failed,
                ..
            }
        )));
        assert!(events.contains(&DecodedEvent::TurnCompleted));
        assert!(
            decoder
                .answer_user_ask(&request.native_request_id, &answers)
                .is_none()
        );
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
