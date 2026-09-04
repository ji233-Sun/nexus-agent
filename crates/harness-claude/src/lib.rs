use std::path::{Path, PathBuf};

use nexus_domain::{HarnessKind, ThinkingEffort};
pub use nexus_harness_core::{DecodedEvent, LaunchSpec};
use nexus_harness_core::{LineDecoder, resolve_executable, summarize_json};
use nexus_protocol::HarnessProbe;
use serde_json::Value;
use tokio::process::Command;

pub fn build_launch_spec(
    executable: &str,
    cwd: &Path,
    prompt: &str,
    model: Option<&str>,
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
    if let Some(model) = model {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_spec_includes_model_and_effort_without_prompt_in_argv() {
        let spec = build_launch_spec(
            "/usr/local/bin/claude",
            Path::new("/tmp/project"),
            "secret prompt",
            Some("opus"),
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
