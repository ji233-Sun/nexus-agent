use std::{
    env,
    path::{Path, PathBuf},
};

use nexus_domain::{ClaudeModel, ThinkingEffort};
use nexus_protocol::HarnessProbe;
use serde_json::Value;
use tokio::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub stdin: String,
}

pub fn build_launch_spec(
    executable: &str,
    cwd: &Path,
    prompt: &str,
    model: ClaudeModel,
    effort: ThinkingEffort,
) -> LaunchSpec {
    let mut args = vec![
        "--print".into(),
        "--input-format".into(),
        "text".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--include-partial-messages".into(),
        "--permission-mode".into(),
        "acceptEdits".into(),
        "--no-session-persistence".into(),
        "--effort".into(),
        effort.as_str().into(),
    ];
    if let Some(model) = model.cli_value() {
        args.push("--model".into());
        args.push(model.into());
    }

    LaunchSpec {
        executable: PathBuf::from(executable),
        args,
        cwd: cwd.to_path_buf(),
        stdin: prompt.to_owned(),
    }
}

pub async fn probe(configured_executable: &str) -> HarnessProbe {
    let executable = resolve_executable(configured_executable);
    let Some(executable) = executable else {
        return HarnessProbe {
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
            available: false,
            authenticated: false,
            executable: executable.display().to_string(),
            version: None,
            message: "Claude Code 存在，但无法执行。请检查文件权限。".into(),
        };
    };
    if !version.status.success() {
        return HarnessProbe {
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

pub fn resolve_executable(configured: &str) -> Option<PathBuf> {
    let configured_path = PathBuf::from(configured);
    if configured_path.components().count() > 1 {
        return is_executable_file(&configured_path).then_some(configured_path);
    }

    if let Some(path) = env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|path| path.join(configured))
            .find(|path| is_executable_file(path))
    }) {
        return Some(path);
    }

    let mut candidates = vec![
        PathBuf::from("/opt/homebrew/bin").join(configured),
        PathBuf::from("/usr/local/bin").join(configured),
    ];
    if let Some(home) = env::var_os("HOME") {
        candidates.insert(0, PathBuf::from(home).join(".local/bin").join(configured));
    }
    candidates.into_iter().find(|path| is_executable_file(path))
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecodedEvent {
    TextDelta(String),
    MessageCompleted(String),
    ToolStarted {
        id: String,
        name: String,
        summary: String,
    },
    ToolCompleted {
        id: String,
        output: String,
        is_error: bool,
    },
    Status(String),
}

#[derive(Default)]
pub struct EventDecoder;

impl EventDecoder {
    pub fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let frame: Value = serde_json::from_str(line)?;
        Ok(decode_frame(&frame))
    }
}

fn decode_frame(frame: &Value) -> Vec<DecodedEvent> {
    match frame.get("type").and_then(Value::as_str) {
        Some("stream_event") => decode_stream_event(frame),
        Some("assistant") => decode_assistant(frame),
        Some("user") => decode_tool_results(frame),
        Some("system") => frame
            .get("subtype")
            .and_then(Value::as_str)
            .map(|subtype| vec![DecodedEvent::Status(format!("Claude: {subtype}"))])
            .unwrap_or_default(),
        Some("result") => Vec::new(),
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
                let summary = block.get("input").map(summarize_json).unwrap_or_default();
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
            output: block.get("content").map(summarize_json).unwrap_or_default(),
            is_error: block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
        .collect()
}

fn summarize_json(value: &Value) -> String {
    const MAX_CHARS: usize = 400;
    let raw = value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string());
    if raw.chars().count() <= MAX_CHARS {
        raw
    } else {
        let mut summary: String = raw.chars().take(MAX_CHARS).collect();
        summary.push('…');
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_spec_includes_model_and_effort_without_prompt_in_argv() {
        let spec = build_launch_spec(
            "/usr/local/bin/claude",
            Path::new("/tmp/project"),
            "secret prompt",
            ClaudeModel::Opus,
            ThinkingEffort::XHigh,
        );
        assert!(spec.args.windows(2).any(|pair| pair == ["--model", "opus"]));
        assert!(
            spec.args
                .windows(2)
                .any(|pair| pair == ["--effort", "xhigh"])
        );
        assert!(!spec.args.iter().any(|arg| arg.contains("secret prompt")));
        assert_eq!(spec.stdin, "secret prompt");
    }

    #[test]
    fn decoder_maps_text_and_tool_events() {
        let mut decoder = EventDecoder;
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
    }

    #[test]
    fn malformed_frames_are_recoverable() {
        let mut decoder = EventDecoder;
        assert!(decoder.decode_line("not json").is_err());
        assert_eq!(
            decoder
                .decode_line(r#"{"type":"result","result":"done"}"#)
                .unwrap(),
            Vec::<DecodedEvent>::new()
        );
    }
}
