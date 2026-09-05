use std::path::{Path, PathBuf};

use nexus_domain::{HarnessKind, ThinkingEffort};
pub use nexus_harness_core::{DecodedEvent, LaunchSpec};
use nexus_harness_core::{LineDecoder, resolve_executable, summarize_json, summarize_text};
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
        "--mode".into(),
        "json".into(),
        "--no-session".into(),
        "--no-title".into(),
        "--approval-mode".into(),
        "write".into(),
        "--thinking".into(),
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
            harness: HarnessKind::Omp,
            available: false,
            authenticated: false,
            executable: configured_executable.to_owned(),
            version: None,
            message: "未找到 Oh My Pi。请安装后在设置中填写 omp 可执行文件路径。".into(),
        };
    };

    let version = Command::new(&executable).arg("--version").output().await;
    let Ok(version) = version else {
        return HarnessProbe {
            harness: HarnessKind::Omp,
            available: false,
            authenticated: false,
            executable: executable.display().to_string(),
            version: None,
            message: "Oh My Pi 存在，但无法执行。请检查文件权限。".into(),
        };
    };
    if !version.status.success() {
        return HarnessProbe {
            harness: HarnessKind::Omp,
            available: false,
            authenticated: false,
            executable: executable.display().to_string(),
            version: None,
            message: "Oh My Pi 版本探测失败。".into(),
        };
    }
    let version = String::from_utf8_lossy(&version.stdout).trim().to_owned();
    let authenticated = Command::new(&executable)
        .args(["models", "--json"])
        .output()
        .await
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<Value>(&output.stdout).ok())
        .and_then(|value| value.get("models").and_then(Value::as_array).map(Vec::len))
        .is_some_and(|count| count > 0);

    HarnessProbe {
        harness: HarnessKind::Omp,
        available: true,
        authenticated,
        executable: executable.display().to_string(),
        version: Some(version),
        message: if authenticated {
            "Oh My Pi 已就绪".into()
        } else {
            "Oh My Pi 尚无可用模型，请先完成登录或配置 Provider。".into()
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
        Some("agent_start") => vec![DecodedEvent::Status("Oh My Pi 会话已启动".into())],
        Some("turn_start") => vec![DecodedEvent::Status("Oh My Pi 正在处理任务…".into())],
        Some("message_update") => frame
            .get("assistantMessageEvent")
            .filter(|event| event.get("type").and_then(Value::as_str) == Some("text_delta"))
            .and_then(|event| event.get("delta").and_then(Value::as_str))
            .filter(|text| !text.is_empty())
            .map(|text| vec![DecodedEvent::TextDelta(text.to_owned())])
            .unwrap_or_default(),
        Some("message_end") => decode_message(frame),
        Some("tool_execution_start") => vec![DecodedEvent::ToolStarted {
            id: event_string(frame, "toolCallId", "unknown"),
            name: event_string(frame, "toolName", "Tool"),
            summary: frame.get("args").map(summarize_json).unwrap_or_default(),
        }],
        Some("tool_execution_end") => vec![DecodedEvent::ToolCompleted {
            id: event_string(frame, "toolCallId", "unknown"),
            output: frame
                .get("result")
                .map(summarize_tool_result)
                .unwrap_or_default(),
            is_error: frame
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }],
        Some("notice") => frame
            .get("message")
            .and_then(Value::as_str)
            .map(|message| {
                if frame.get("level").and_then(Value::as_str) == Some("error") {
                    DecodedEvent::Error(summarize_text(message))
                } else {
                    DecodedEvent::Status(format!("Oh My Pi: {}", summarize_text(message)))
                }
            })
            .into_iter()
            .collect(),
        Some("auto_retry_start") => vec![DecodedEvent::Status("Oh My Pi 正在重试请求…".into())],
        Some("auto_retry_end") if frame.get("success").and_then(Value::as_bool) == Some(false) => {
            frame
                .get("finalError")
                .and_then(Value::as_str)
                .map(|message| vec![DecodedEvent::Error(summarize_text(message))])
                .unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

fn decode_message(frame: &Value) -> Vec<DecodedEvent> {
    let Some(message) = frame.get("message") else {
        return Vec::new();
    };
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return Vec::new();
    }
    if matches!(
        message.get("stopReason").and_then(Value::as_str),
        Some("error" | "aborted")
    ) && let Some(error) = message.get("errorMessage").and_then(Value::as_str)
    {
        return vec![DecodedEvent::Error(summarize_text(error))];
    }

    let text = message
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<String>();
    (!text.is_empty())
        .then_some(DecodedEvent::MessageCompleted(text))
        .into_iter()
        .collect()
}

fn summarize_tool_result(result: &Value) -> String {
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        summarize_json(result)
    } else {
        summarize_text(&text)
    }
}

fn event_string(frame: &Value, key: &str, fallback: &str) -> String {
    frame
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_spec_uses_json_print_mode_without_prompt_in_argv() {
        let spec = build_launch_spec(
            "/usr/local/bin/omp",
            Path::new("/tmp/project"),
            "secret prompt",
            Some("deepseek/deepseek-v4-pro"),
            ThinkingEffort::High,
        );
        assert!(spec.args.windows(2).any(|pair| pair == ["--mode", "json"]));
        assert!(
            spec.args
                .windows(2)
                .any(|pair| pair == ["--approval-mode", "write"])
        );
        assert!(
            spec.args
                .windows(2)
                .any(|pair| pair == ["--thinking", "high"])
        );
        assert!(
            spec.args
                .windows(2)
                .any(|pair| pair == ["--model", "deepseek/deepseek-v4-pro"])
        );
        assert!(!spec.args.iter().any(|arg| arg.contains("secret prompt")));
        assert_eq!(spec.stdin, "secret prompt");
    }

    #[test]
    fn decoder_maps_stream_messages_and_tool_events() {
        let mut decoder = EventDecoder;
        let delta = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"你好"}}"#;
        assert_eq!(
            decoder.decode_line(delta).unwrap(),
            vec![DecodedEvent::TextDelta("你好".into())]
        );

        let started = r#"{"type":"tool_execution_start","toolCallId":"t1","toolName":"read","args":{"path":"README.md"}}"#;
        assert!(matches!(
            decoder.decode_line(started).unwrap().as_slice(),
            [DecodedEvent::ToolStarted { id, name, summary }]
                if id == "t1" && name == "read" && summary.contains("README.md")
        ));

        let completed = r#"{"type":"tool_execution_end","toolCallId":"t1","toolName":"read","result":{"content":[{"type":"text","text":"contents"}]},"isError":false}"#;
        assert_eq!(
            decoder.decode_line(completed).unwrap(),
            vec![DecodedEvent::ToolCompleted {
                id: "t1".into(),
                output: "contents".into(),
                is_error: false,
            }]
        );

        let message = r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"stopReason":"stop"}}"#;
        assert_eq!(
            decoder.decode_line(message).unwrap(),
            vec![DecodedEvent::MessageCompleted("done".into())]
        );
    }

    #[test]
    fn decoder_surfaces_provider_errors() {
        let mut decoder = EventDecoder;
        let message = r#"{"type":"message_end","message":{"role":"assistant","content":[],"stopReason":"error","errorMessage":"denied"}}"#;
        assert_eq!(
            decoder.decode_line(message).unwrap(),
            vec![DecodedEvent::Error("denied".into())]
        );
        assert!(decoder.decode_line("not json").is_err());
    }
}
