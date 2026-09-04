use std::{
    env,
    path::{Path, PathBuf},
};

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
        "exec".into(),
        "--skip-git-repo-check".into(),
        "--json".into(),
        "--sandbox".into(),
        "workspace-write".into(),
        "--ephemeral".into(),
        "--color".into(),
        "never".into(),
        "--config".into(),
        format!("model_reasoning_effort=\"{}\"", effort.as_str()),
    ];
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
            summary: summarize_text(
                item.get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
        }],
        Some("mcp_tool_call") => vec![DecodedEvent::ToolStarted {
            id,
            name: mcp_tool_name(item),
            summary: item
                .get("arguments")
                .map(summarize_json)
                .unwrap_or_default(),
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
                .map(summarize_text)
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
            let summary = summarize_file_changes(item);
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
                .or_else(|| item.get("result").map(summarize_json))
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

fn summarize_file_changes(item: &Value) -> String {
    let summary = item
        .get("changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|change| {
            let kind = change
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("update");
            let path = change
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("{kind}: {path}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    summarize_text(&summary)
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
