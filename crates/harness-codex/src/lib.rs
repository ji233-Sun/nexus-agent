use std::{
    collections::HashSet,
    env,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use nexus_domain::{HarnessKind, ModelDescriptor, ModelReasoningEffort, ThinkingEffort};
pub use nexus_harness_core::{DecodedEvent, LaunchSpec};
use nexus_harness_core::{LineDecoder, resolve_executable, summarize_text, tool_content};
use nexus_protocol::{EnvironmentVariable, HarnessProbe};
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

pub fn build_launch_spec(
    executable: &str,
    cwd: &Path,
    prompt: &str,
    model: Option<&str>,
    effort: ThinkingEffort,
) -> LaunchSpec {
    let mut args = vec![
        "exec".into(),
        "--skip-git-repo-check".into(),
        "--json".into(),
        "--sandbox".into(),
        "workspace-write".into(),
        "--ephemeral".into(),
        "--color".into(),
        "never".into(),
    ];
    if !effort.is_default() {
        args.push("--config".into());
        args.push(format!("model_reasoning_effort=\"{}\"", effort.as_str()));
    }
    if let Some(model) = model {
        args.push("--model".into());
        args.push(model.into());
    }
    args.push("-".into());

    LaunchSpec {
        executable: PathBuf::from(executable),
        args,
        cwd: cwd.to_path_buf(),
        stdin: prompt.to_owned(),
    }
}

pub fn build_title_launch_spec(
    executable: &str,
    cwd: &Path,
    prompt: &str,
    model: Option<&str>,
    effort: ThinkingEffort,
) -> LaunchSpec {
    let mut spec = build_launch_spec(executable, cwd, prompt, model, effort);
    if let Some(sandbox) = spec.args.iter_mut().find(|arg| *arg == "workspace-write") {
        *sandbox = "read-only".into();
    }
    spec.args.insert(1, "--ignore-rules".into());
    spec
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCatalogError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for ModelCatalogError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("模型目录探测已取消"),
            Self::Failed(message) => formatter.write_str(message),
        }
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

#[derive(Default)]
pub struct EventDecoder;

impl LineDecoder for EventDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let frame: Value = serde_json::from_str(line)?;
        Ok(decode_frame(&frame))
    }
}

fn decode_frame(frame: &Value) -> Vec<DecodedEvent> {
    match frame.get("type").and_then(Value::as_str) {
        Some("thread.started") => vec![DecodedEvent::Status("Codex 会话已启动".into())],
        Some("turn.started") => vec![DecodedEvent::Status("Codex 正在处理任务…".into())],
        Some("item.started") => frame
            .get("item")
            .map(decode_started_item)
            .unwrap_or_default(),
        Some("item.updated") => frame
            .get("item")
            .map(decode_updated_item)
            .unwrap_or_default(),
        Some("item.completed") => frame
            .get("item")
            .map(decode_completed_item)
            .unwrap_or_default(),
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
    }
}

fn decode_started_item(item: &Value) -> Vec<DecodedEvent> {
    let id = item_id(item);
    match item.get("type").and_then(Value::as_str) {
        Some("command_execution") => vec![DecodedEvent::ToolStarted {
            id,
            name: "Command".into(),
            summary: item
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }],
        Some("mcp_tool_call") => vec![DecodedEvent::ToolStarted {
            id,
            name: mcp_tool_name(item),
            summary: item.get("arguments").map(tool_content).unwrap_or_default(),
        }],
        Some("web_search") => vec![DecodedEvent::ToolStarted {
            id,
            name: "Web Search".into(),
            summary: summarize_text(
                item.get("query")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
        }],
        Some("todo_list") => decode_todo_list(item),
        _ => Vec::new(),
    }
}

fn decode_updated_item(item: &Value) -> Vec<DecodedEvent> {
    match item.get("type").and_then(Value::as_str) {
        Some("todo_list") => decode_todo_list(item),
        _ => Vec::new(),
    }
}

fn decode_completed_item(item: &Value) -> Vec<DecodedEvent> {
    let id = item_id(item);
    match item.get("type").and_then(Value::as_str) {
        Some("agent_message") => item
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(|text| vec![DecodedEvent::MessageCompleted(text.to_owned())])
            .unwrap_or_default(),
        Some("command_execution") => {
            let status = item
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("failed");
            let exit_code = item.get("exit_code").and_then(Value::as_i64);
            let output = item
                .get("aggregated_output")
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
        Some("file_change") => {
            let summary = tool_content(item);
            let status = item
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("failed");
            vec![
                DecodedEvent::ToolStarted {
                    id: id.clone(),
                    name: "File Change".into(),
                    summary,
                },
                DecodedEvent::ToolCompleted {
                    id,
                    output: format!("文件修改状态：{status}"),
                    is_error: status != "completed",
                },
            ]
        }
        Some("mcp_tool_call") => {
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
        Some("web_search") => vec![DecodedEvent::ToolCompleted {
            id,
            output: item
                .get("query")
                .and_then(Value::as_str)
                .map(|query| format!("搜索完成：{query}"))
                .unwrap_or_else(|| "搜索已完成".into()),
            is_error: false,
        }],
        Some("todo_list") => decode_todo_list(item),
        Some("error") => item
            .get("message")
            .and_then(Value::as_str)
            .map(|message| {
                vec![DecodedEvent::Status(format!(
                    "Codex: {}",
                    summarize_text(message)
                ))]
            })
            .unwrap_or_default(),
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

fn decode_todo_list(item: &Value) -> Vec<DecodedEvent> {
    let summary = item
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| {
            let marker = if item
                .get("completed")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "✓"
            } else {
                "·"
            };
            let text = item.get("text").and_then(Value::as_str).unwrap_or_default();
            format!("{marker} {text}")
        })
        .collect::<Vec<_>>()
        .join("  ");
    (!summary.is_empty())
        .then(|| DecodedEvent::Status(format!("Codex 计划：{}", summarize_text(&summary))))
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_spec_allows_selected_non_git_directories() {
        let spec = build_launch_spec(
            "/usr/local/bin/codex",
            Path::new("/tmp/project"),
            "secret prompt",
            None,
            ThinkingEffort::XHigh,
        );
        assert_eq!(spec.args.first().map(String::as_str), Some("exec"));
        assert!(spec.args.iter().any(|arg| arg == "--skip-git-repo-check"));
        assert!(spec.args.iter().any(|arg| arg == "--json"));
        assert!(
            spec.args
                .windows(2)
                .any(|pair| pair == ["--sandbox", "workspace-write"])
        );
        assert!(
            spec.args
                .windows(2)
                .any(|pair| pair == ["--config", "model_reasoning_effort=\"xhigh\""])
        );
        assert_eq!(spec.args.last().map(String::as_str), Some("-"));
        assert!(!spec.args.iter().any(|arg| arg.contains("secret prompt")));
        assert_eq!(spec.stdin, "secret prompt");
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
        let spec = build_launch_spec(
            "/usr/local/bin/codex",
            Path::new("/tmp/project"),
            "prompt",
            None,
            ThinkingEffort::Default,
        );
        assert!(!spec.args.iter().any(|arg| arg == "--model"));
        assert!(!spec.args.iter().any(|arg| arg == "--config"));
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
        let mut decoder = EventDecoder;
        let started = r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"cargo test","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#;
        assert!(matches!(
            decoder.decode_line(started).unwrap().as_slice(),
            [DecodedEvent::ToolStarted { id, name, summary }]
                if id == "item_1" && name == "Command" && summary == "cargo test"
        ));

        let completed = r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"cargo test","aggregated_output":"ok","exit_code":0,"status":"completed"}}"#;
        assert_eq!(
            decoder.decode_line(completed).unwrap(),
            vec![DecodedEvent::ToolCompleted {
                id: "item_1".into(),
                output: "ok".into(),
                is_error: false,
            }]
        );

        let message = r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"done"}}"#;
        assert_eq!(
            decoder.decode_line(message).unwrap(),
            vec![DecodedEvent::MessageCompleted("done".into())]
        );

        let long_text = "完整命令和输出\n".repeat(100);
        let started = serde_json::json!({"type": "item.started", "item": {
            "id": "long", "type": "command_execution", "command": long_text
        }});
        assert!(matches!(
            decoder.decode_line(&started.to_string()).unwrap().as_slice(),
            [DecodedEvent::ToolStarted { summary, .. }] if summary == &long_text
        ));
        let completed = serde_json::json!({"type": "item.completed", "item": {
            "id": "long", "type": "command_execution", "aggregated_output": long_text,
            "status": "completed", "exit_code": 1
        }});
        assert!(matches!(
            decoder.decode_line(&completed.to_string()).unwrap().as_slice(),
            [DecodedEvent::ToolCompleted { output, is_error: true, .. }] if output == &long_text
        ));
        let item = serde_json::json!({"id": "edit", "type": "file_change", "status": "completed",
            "changes": [{"kind": "update", "path": "main.rs", "diff": long_text}]
        });
        let completed = serde_json::json!({"type": "item.completed", "item": item});
        assert!(matches!(
            decoder.decode_line(&completed.to_string()).unwrap().as_slice(),
            [DecodedEvent::ToolStarted { summary, .. }, DecodedEvent::ToolCompleted { .. }]
                if serde_json::from_str::<Value>(summary).unwrap() == item
        ));
    }

    #[test]
    fn malformed_frames_are_recoverable() {
        let mut decoder = EventDecoder;
        assert!(decoder.decode_line("not json").is_err());
        assert_eq!(
            decoder
                .decode_line(r#"{"type":"turn.failed","error":{"message":"denied"}}"#)
                .unwrap(),
            vec![DecodedEvent::Error("denied".into())]
        );
        assert_eq!(
            decoder.decode_line(r#"{"type":"turn.completed"}"#).unwrap(),
            Vec::<DecodedEvent>::new()
        );
    }
}
