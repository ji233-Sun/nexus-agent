use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
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
use nexus_protocol::{EnvironmentVariable, HarnessProbe, StartRun};
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::sync::watch;
/// Anthropic 兼容 API 接入点。第三方网关（Kimi、GLM 等）与官方 API 共用同一套
/// 模型目录端点 `{base_url}/v1/models`。
#[derive(Debug, Default, PartialEq, Eq)]
struct AnthropicEndpoint {
    base_url: Option<String>,
    api_key: Option<String>,
    auth_token: Option<String>,
}

const ANTHROPIC_DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const MODEL_PAGE_LIMIT: u32 = 1000;
const MODEL_MAX_PAGES: usize = 5;
const MODEL_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

pub async fn discover_models(
    executable: &str,
    cwd: &Path,
    environment: &[EnvironmentVariable],
    cancel: watch::Receiver<bool>,
) -> Result<Vec<ModelDescriptor>, ModelCatalogError> {
    discover_models_in(
        executable,
        cwd,
        environment,
        cancel,
        user_claude_config_dir().as_deref(),
    )
    .await
}

async fn discover_models_in(
    executable: &str,
    cwd: &Path,
    environment: &[EnvironmentVariable],
    cancel: watch::Receiver<bool>,
    user_config_dir: Option<&Path>,
) -> Result<Vec<ModelDescriptor>, ModelCatalogError> {
    if *cancel.borrow() {
        return Err(ModelCatalogError::Cancelled);
    }
    if resolve_executable(executable).is_none() {
        return Err(ModelCatalogError::Failed(
            "未找到 Claude Code，无法加载模型目录。".into(),
        ));
    }
    // Claude Code 自身无法枚举第三方 API 的模型，目录改为直接询问该 API；
    // 任何失败（未配置凭证、网关不支持、网络异常）都回退到 CLI 别名。
    if let Some(endpoint) = resolve_anthropic_endpoint(environment, cwd, user_config_dir) {
        match fetch_claude_models(&endpoint, &cancel).await {
            Ok(models) if !models.is_empty() => return Ok(models),
            Ok(_) | Err(ModelCatalogError::Failed(_)) => {}
            Err(error @ ModelCatalogError::Cancelled) => return Err(error),
        }
    }
    Ok(claude_alias_models())
}

/// CLI 别名目录：官方登录等无法枚举 API 目录的场景仍然可用。
fn claude_alias_models() -> Vec<ModelDescriptor> {
    // These are CLI aliases, not a discovered account catalog. Version-specific
    // model capabilities are deliberately left unknown until the adapter reports them.
    ClaudeModel::ALL
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
        .collect()
}

/// Claude Code 读取的用户级配置目录：`CLAUDE_CONFIG_DIR` 优先，默认 `~/.claude`。
fn user_claude_config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".claude"))
}

fn claude_settings_paths(cwd: &Path, user_config_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = vec![
        cwd.join(".claude/settings.local.json"),
        cwd.join(".claude/settings.json"),
    ];
    if let Some(dir) = user_config_dir {
        paths.push(dir.join("settings.json"));
    }
    paths
}

/// 解析模型目录端点。优先级与 Claude Code 启动时的生效顺序一致：
/// Provider Profile 注入的环境变量 → Claude Code settings 文件 → 进程环境。
fn resolve_anthropic_endpoint(
    environment: &[EnvironmentVariable],
    cwd: &Path,
    user_config_dir: Option<&Path>,
) -> Option<AnthropicEndpoint> {
    resolve_endpoint_with(environment, cwd, user_config_dir, |name| {
        std::env::var(name).ok()
    })
}

fn resolve_endpoint_with(
    environment: &[EnvironmentVariable],
    cwd: &Path,
    user_config_dir: Option<&Path>,
    process_env: impl Fn(&str) -> Option<String>,
) -> Option<AnthropicEndpoint> {
    let mut endpoint = AnthropicEndpoint::default();
    for variable in environment {
        apply_endpoint_variable(&mut endpoint, &variable.name, &variable.value);
    }
    for path in claude_settings_paths(cwd, user_config_dir) {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(settings) = serde_json::from_str::<Value>(&content) else {
            continue;
        };
        if let Some(env) = settings.get("env").and_then(Value::as_object) {
            for (name, value) in env {
                if let Some(value) = value.as_str() {
                    apply_endpoint_variable(&mut endpoint, name, value);
                }
            }
        }
    }
    for name in [
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
    ] {
        if let Some(value) = process_env(name) {
            apply_endpoint_variable(&mut endpoint, name, &value);
        }
    }
    if endpoint.api_key.is_none() && endpoint.auth_token.is_none() {
        return None;
    }
    Some(AnthropicEndpoint {
        base_url: Some(
            endpoint
                .base_url
                .unwrap_or_else(|| ANTHROPIC_DEFAULT_BASE_URL.into()),
        ),
        api_key: endpoint.api_key,
        auth_token: endpoint.auth_token,
    })
}

/// 每个变量只接受首个非空来源，保证高优先级配置不被低优先级覆盖。
fn apply_endpoint_variable(endpoint: &mut AnthropicEndpoint, name: &str, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    match name {
        "ANTHROPIC_BASE_URL" => {
            let base = value.trim_end_matches('/');
            if !base.is_empty() {
                endpoint.base_url.get_or_insert(base.to_owned());
            }
        }
        "ANTHROPIC_API_KEY" => {
            endpoint.api_key.get_or_insert(value.to_owned());
        }
        "ANTHROPIC_AUTH_TOKEN" => {
            endpoint.auth_token.get_or_insert(value.to_owned());
        }
        _ => {}
    }
}

async fn fetch_claude_models(
    endpoint: &AnthropicEndpoint,
    cancel: &watch::Receiver<bool>,
) -> Result<Vec<ModelDescriptor>, ModelCatalogError> {
    let mut cancel = cancel.clone();
    let base_url = endpoint
        .base_url
        .as_deref()
        .unwrap_or(ANTHROPIC_DEFAULT_BASE_URL)
        .trim_end_matches('/');
    let client = reqwest::Client::builder()
        .timeout(MODEL_REQUEST_TIMEOUT)
        .build()
        .map_err(|_| ModelCatalogError::Failed("无法初始化模型目录 HTTP 客户端。".into()))?;
    let provider = reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "anthropic".into());
    let mut after_id: Option<String> = None;
    let mut models = Vec::new();
    for _ in 0..MODEL_MAX_PAGES {
        if *cancel.borrow() {
            return Err(ModelCatalogError::Cancelled);
        }
        let mut url = format!("{base_url}/v1/models?limit={MODEL_PAGE_LIMIT}");
        if let Some(after) = &after_id {
            url.push_str("&after_id=");
            url.push_str(after);
        }
        let mut request = client.get(url).header("anthropic-version", "2023-06-01");
        if let Some(token) = &endpoint.auth_token {
            request = request.bearer_auth(token);
        }
        if let Some(key) = &endpoint.api_key {
            request = request.header("x-api-key", key);
        }
        let value: Value = tokio::select! {
            biased;
            // 同时就绪时优先取消；未发出取消信号的关闭通道不影响请求。
            Ok(_) = cancel.wait_for(|cancelled| *cancelled) => {
                return Err(ModelCatalogError::Cancelled);
            }
            result = async {
                let response = request.send().await.map_err(|_| {
                    ModelCatalogError::Failed(format!("无法连接 {provider} 的模型目录接口。"))
                })?;
                if !response.status().is_success() {
                    return Err(ModelCatalogError::Failed(format!(
                        "{provider} 的模型目录接口返回了 {}。",
                        response.status().as_u16()
                    )));
                }
                response.json().await.map_err(|_| {
                    ModelCatalogError::Failed(format!("{provider} 的模型目录返回了无效 JSON。"))
                })
            } => result?,
        };
        models.extend(parse_claude_model_page(&value, &provider));
        match value.get("has_more").and_then(Value::as_bool) {
            Some(true) => {}
            _ => return Ok(models),
        }
        after_id = value
            .get("last_id")
            .and_then(Value::as_str)
            .filter(|last| !last.is_empty())
            .map(str::to_owned);
        if after_id.is_none() {
            return Ok(models);
        }
    }
    Ok(models)
}

fn parse_claude_model_page(value: &Value, provider: &str) -> Vec<ModelDescriptor> {
    value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?.trim();
            if id.is_empty() {
                return None;
            }
            let display_name = item
                .get("display_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(id);
            Some(ModelDescriptor {
                id: id.to_owned(),
                display_name: display_name.to_owned(),
                source: ModelSource::ClaudeApi,
                availability: ModelAvailability::Available,
                provider: Some(provider.to_owned()),
                is_default: false,
                supported_reasoning_efforts: Vec::new(),
                default_reasoning_effort: None,
            })
        })
        .collect()
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

pub fn prepare_run(request: &StartRun, cwd: &Path) -> Result<LaunchSpec, String> {
    use base64::Engine as _;
    let mut spec = build_launch_spec(
        &request.executable,
        cwd,
        &request.prompt,
        request.model.as_deref(),
        request.effort,
        request.session_id.as_deref(),
        request.permission_mode,
    );
    if !request.attachments.is_empty() {
        let mut content = vec![json!({"type": "text", "text": request.prompt})];
        for image in &request.attachments {
            let bytes = nexus_harness_core::read_image_attachment(image)?;
            content.push(json!({"type": "text", "text": image.label()}));
            content.push(json!({"type": "image", "source": {
                "type": "base64", "media_type": nexus_harness_core::image_media_type(&bytes).expect("validated image"),
                "data": base64::engine::general_purpose::STANDARD.encode(bytes)
            }}));
        }
        let mut frame = user_input(&request.prompt, None);
        frame["message"]["content"] = content.into();
        spec.stdin = format!("{frame}\n");
    }
    Ok(spec)
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

pub struct EventDecoder {
    harness: HarnessKind,
    pending_user_asks: HashMap<String, Value>,
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
        }
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
            self.pending_user_asks.clear();
        }
        Ok(decode_frame(&frame, self.harness))
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

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 清除进程中的 Anthropic 环境变量，保证目录测试不依赖开发机配置。
    fn scrub_anthropic_env() {
        for name in [
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
        ] {
            // SAFETY: 持有 ENV_LOCK 期间独占修改进程环境，测试结束后恢复。
            unsafe { std::env::remove_var(name) };
        }
    }

    async fn serve_model_catalog(body: &'static str) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(request).unwrap()
        });
        (format!("http://{address}/"), server)
    }

    // 环境变量必须在整个异步用例期间保持已清除，锁只能跨 await 持有。
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn catalog_prefers_api_discovery_and_falls_back_to_aliases() {
        let guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        scrub_anthropic_env();
        let executable = std::env::current_exe().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let user_config = tempfile::tempdir().unwrap();
        let endpoint = vec![
            EnvironmentVariable {
                name: "ANTHROPIC_BASE_URL".into(),
                value: "https://gateway.invalid/anthropic".into(),
            },
            EnvironmentVariable {
                name: "ANTHROPIC_AUTH_TOKEN".into(),
                value: "gateway-token".into(),
            },
        ];

        // 网关不可达或未实现 /v1/models 时回退 CLI 别名，官方登录用户不受影响。
        let models = discover_models_in(
            executable.to_str().unwrap(),
            cwd.path(),
            &endpoint,
            watch::channel(false).1,
            Some(user_config.path()),
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
        for model in &models {
            assert_eq!(model.source, ModelSource::ClaudeAliases);
            assert_eq!(model.availability, ModelAvailability::Unknown);
            assert!(model.supported_reasoning_efforts.is_empty());
            assert!(model.default_reasoning_effort.is_none());
        }

        // 网关返回目录时直接采用 API 模型，不再混合别名。
        let (base_url, server) = serve_model_catalog(
            r#"{"data":[{"type":"model","id":"glm-4.6","display_name":"GLM-4.6"}],"has_more":false}"#,
        )
        .await;
        let mut endpoint = endpoint;
        endpoint[0].value = base_url;
        let models = discover_models_in(
            executable.to_str().unwrap(),
            cwd.path(),
            &endpoint,
            watch::channel(false).1,
            Some(user_config.path()),
        )
        .await
        .unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "glm-4.6");
        assert_eq!(models[0].source, ModelSource::ClaudeApi);
        assert_eq!(models[0].availability, ModelAvailability::Available);
        assert!(server.await.unwrap().contains("GET /v1/models?limit=1000 "));

        // 未配置任何凭证时完全不发起请求，直接回退别名。
        let models = discover_models_in(
            executable.to_str().unwrap(),
            cwd.path(),
            &[],
            watch::channel(false).1,
            Some(user_config.path()),
        )
        .await
        .unwrap();
        assert_eq!(models[0].id, "sonnet");

        // 取消与缺失可执行文件的语义保持不变。
        let (cancel, receiver) = watch::channel(false);
        cancel.send_replace(true);
        assert_eq!(
            discover_models_in(
                "unused",
                cwd.path(),
                &[],
                receiver,
                Some(user_config.path())
            )
            .await,
            Err(ModelCatalogError::Cancelled)
        );
        assert!(matches!(
            discover_models_in(
                "nexus-missing-claude",
                cwd.path(),
                &[],
                watch::channel(false).1,
                Some(user_config.path())
            )
            .await,
            Err(ModelCatalogError::Failed(_))
        ));
        drop(guard);
    }

    #[tokio::test]
    async fn fetch_claude_models_sends_gateway_credentials() {
        let (base_url, server) = serve_model_catalog(
            r#"{"data":[{"id":"kimi-k2-turbo-preview","display_name":"K2 Turbo"}],"has_more":false}"#,
        )
        .await;
        let endpoint = AnthropicEndpoint {
            base_url: Some(base_url),
            api_key: Some("sk-key".into()),
            auth_token: Some("bearer-token".into()),
        };
        let models = fetch_claude_models(&endpoint, &watch::channel(false).1)
            .await
            .unwrap();
        let request = server.await.unwrap();
        assert!(
            request.contains("GET /v1/models?limit=1000 HTTP/1.1"),
            "actual request: {request}"
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer bearer-token")
        );
        assert!(request.to_ascii_lowercase().contains("x-api-key: sk-key"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("anthropic-version: 2023-06-01")
        );
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "kimi-k2-turbo-preview");
    }

    #[tokio::test]
    async fn catalog_cancellation_interrupts_stalled_headers_and_body() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        for send_headers in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let executable = std::env::current_exe().unwrap();
            let directory = tempfile::tempdir().unwrap();
            let environment = [
                EnvironmentVariable {
                    name: "ANTHROPIC_BASE_URL".into(),
                    value: format!("http://{}", listener.local_addr().unwrap()),
                },
                EnvironmentVariable {
                    name: "ANTHROPIC_API_KEY".into(),
                    value: "test-key".into(),
                },
                EnvironmentVariable {
                    name: "ANTHROPIC_AUTH_TOKEN".into(),
                    value: "test-token".into(),
                },
            ];
            let (cancel, receiver) = watch::channel(false);
            let discovery = discover_models_in(
                executable.to_str().unwrap(),
                directory.path(),
                &environment,
                receiver,
                Some(directory.path()),
            );
            tokio::pin!(discovery);
            let gateway = async {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = BufReader::new(socket);
                loop {
                    let mut line = String::new();
                    assert!(socket.read_line(&mut line).await.unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                }
                if send_headers {
                    socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n")
                        .await
                        .unwrap();
                }
                socket
            };
            let socket = tokio::select! {
                result = &mut discovery => panic!("catalog returned before cancellation: {result:?}"),
                socket = tokio::time::timeout(Duration::from_secs(5), gateway) => socket.unwrap(),
            };
            // 继续轮询请求以消费已发送的响应头，同时保持响应体未完成。
            assert!(
                tokio::time::timeout(Duration::from_millis(50), &mut discovery)
                    .await
                    .is_err()
            );
            cancel.send_replace(true);
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(1), &mut discovery).await,
                Ok(Err(ModelCatalogError::Cancelled)),
                "cancellation must interrupt the request (headers sent: {send_headers})"
            );
            drop(socket);
        }
    }

    #[test]
    fn endpoint_resolution_follows_profile_settings_then_process_env() {
        let project = tempfile::tempdir().unwrap();
        let user_config = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join(".claude")).unwrap();
        std::fs::write(
            project.path().join(".claude/settings.json"),
            r#"{"env":{"ANTHROPIC_BASE_URL":"https://project.example/anthropic"}}"#,
        )
        .unwrap();
        std::fs::write(
            project.path().join(".claude/settings.local.json"),
            r#"{"env":{"ANTHROPIC_BASE_URL":"https://local.example/","ANTHROPIC_AUTH_TOKEN":"local-token"}}"#,
        )
        .unwrap();
        std::fs::write(
            user_config.path().join("settings.json"),
            r#"{"env":{"ANTHROPIC_AUTH_TOKEN":"user-token","ANTHROPIC_API_KEY":""}}"#,
        )
        .unwrap();
        let no_process_env = |name: &str| -> Option<String> {
            let _ = name;
            None
        };

        // 项目 local > 项目 > 用户级；空值被跳过；无凭证时返回 None。
        let resolved = resolve_endpoint_with(
            &[],
            project.path(),
            Some(user_config.path()),
            no_process_env,
        )
        .unwrap();
        assert_eq!(resolved.base_url.as_deref(), Some("https://local.example"));
        assert_eq!(resolved.auth_token.as_deref(), Some("local-token"));
        assert!(resolved.api_key.is_none());

        // Provider Profile 显式环境变量优先于 settings 文件。
        let profile = vec![
            EnvironmentVariable {
                name: "ANTHROPIC_BASE_URL".into(),
                value: "https://profile.example/".into(),
            },
            EnvironmentVariable {
                name: "ANTHROPIC_API_KEY".into(),
                value: "profile-key".into(),
            },
        ];
        let resolved = resolve_endpoint_with(
            &profile,
            project.path(),
            Some(user_config.path()),
            no_process_env,
        )
        .unwrap();
        assert_eq!(
            resolved.base_url.as_deref(),
            Some("https://profile.example")
        );
        assert_eq!(resolved.api_key.as_deref(), Some("profile-key"));
        assert_eq!(resolved.auth_token.as_deref(), Some("local-token"));

        // 进程环境只填补缺口；仅有凭证、无 base URL 时使用官方默认地址。
        let bare_project = tempfile::tempdir().unwrap();
        let resolved = resolve_endpoint_with(&[], bare_project.path(), None, |name| {
            (name == "ANTHROPIC_AUTH_TOKEN").then(|| "process-token".into())
        })
        .unwrap();
        assert_eq!(
            resolved.base_url.as_deref(),
            Some("https://api.anthropic.com")
        );
        assert_eq!(resolved.auth_token.as_deref(), Some("process-token"));
    }

    #[test]
    fn claude_model_page_maps_api_catalog() {
        let models = parse_claude_model_page(
            &serde_json::json!({
                "data": [
                    {"type": "model", "id": "claude-sonnet-4-5", "display_name": "Claude Sonnet 4.5"},
                    {"id": "kimi-k2-turbo-preview"},
                    {"id": "  "},
                    {"display_name": "missing id"}
                ],
                "has_more": false
            }),
            "api.moonshot.ai",
        );
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "claude-sonnet-4-5");
        assert_eq!(models[0].display_name, "Claude Sonnet 4.5");
        assert_eq!(models[1].id, "kimi-k2-turbo-preview");
        assert_eq!(models[1].display_name, "kimi-k2-turbo-preview");
        for model in &models {
            assert_eq!(model.source, ModelSource::ClaudeApi);
            assert_eq!(model.source.harness(), HarnessKind::Claude);
            assert_eq!(model.availability, ModelAvailability::Available);
            assert_eq!(model.provider.as_deref(), Some("api.moonshot.ai"));
            assert!(!model.is_default);
        }
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
        for model_id in ["moonshotai/Kimi-K2.5", "GLM-5"] {
            let custom = build_launch_spec(
                "claude",
                Path::new("."),
                "test",
                Some(model_id),
                ThinkingEffort::Default,
                None,
                PermissionMode::AutoEdit,
            );
            assert!(
                custom
                    .args
                    .windows(2)
                    .any(|pair| pair == ["--model", model_id])
            );
            assert!(!custom.args.iter().any(|arg| arg == "--effort"));
        }
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
