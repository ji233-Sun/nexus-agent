use nexus_domain::{HarnessKind, PermissionMode, ThinkingEffort};
use nexus_harness_core::{
    ApprovalOption, ApprovalPrompt, DecodedEvent, InputFrame, LaunchSpec, LineDecoder,
    resolve_executable, tool_content,
};
use nexus_protocol::{HarnessProbe, StartRun};
use serde_json::{Value, json};
use std::{collections::HashSet, path::Path, time::Duration};

// Protocol sources and supported CLI generations are recorded in docs/harnesses.md.
pub(super) async fn probe(harness: HarnessKind, executable: &str) -> HarnessProbe {
    let resolved = resolve_executable(executable);
    let version = if let Some(path) = &resolved {
        tokio::time::timeout(
            Duration::from_secs(15),
            tokio::process::Command::new(path)
                .arg("--version")
                .kill_on_drop(true)
                .output(),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        None
    };
    let compatible =
        harness != HarnessKind::Pi || version.as_deref().is_some_and(pi_version_supported);
    HarnessProbe {
        harness,
        available: version.is_some() && compatible,
        authenticated: false,
        executable: resolved
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| executable.into()),
        message: if !compatible && version.is_some() {
            "Pi 需要 0.85.1 或更高版本，以支持 agent_settled RPC 终态。".into()
        } else if version.is_some() {
            format!("{harness} 已安装；身份状态未探测，将使用 CLI 原生登录状态。")
        } else {
            format!("无法执行 {harness}；请按官方文档安装并配置可执行文件路径。")
        },
        version,
    }
}

fn pi_version_supported(version: &str) -> bool {
    version
        .split_whitespace()
        .find_map(|word| {
            let mut parts = word.trim_start_matches('v').split('.');
            let major = parts.next()?.parse::<u64>().ok()?;
            let minor = parts.next()?.parse::<u64>().ok()?;
            let patch = parts
                .next()
                .unwrap_or("0")
                .split('-')
                .next()?
                .parse::<u64>()
                .ok()?;
            Some((major, minor, patch) >= (0, 85, 1))
        })
        .unwrap_or(false)
}

pub(super) fn prepare(
    request: &StartRun,
    cwd: &Path,
    title: Option<&str>,
) -> anyhow::Result<(LaunchSpec, Box<dyn LineDecoder>)> {
    let prompt = title.unwrap_or(&request.prompt);
    let mut spec = LaunchSpec {
        executable: request.executable.clone().into(),
        cwd: cwd.into(),
        args: Vec::new(),
        stdin: String::new(),
    };
    let mut assets = None;
    let decoder: Box<dyn LineDecoder> = match request.harness {
        HarnessKind::Pi => {
            spec.args.extend([
                "--mode".into(),
                if title.is_some() { "json" } else { "rpc" }.into(),
            ]);
            if title.is_some() {
                spec.args.extend(
                    [
                        "--print",
                        "--no-session",
                        "--no-tools",
                        "--no-extensions",
                        "--no-skills",
                        "--no-prompt-templates",
                    ]
                    .map(str::to_owned),
                );
                spec.stdin = prompt.into();
            } else {
                let directory = tempfile::tempdir()?;
                let extension = directory.path().join("nexus-permissions.js");
                let mode = serde_json::to_string(request.permission_mode.as_str())?;
                std::fs::write(
                    &extension,
                    format!(
                        "const mode = {mode};\n{}",
                        include_str!("pi-permissions.mjs")
                    ),
                )?;
                spec.args.extend([
                    "--extension".into(),
                    extension.to_string_lossy().into_owned(),
                ]);
                assets = Some(directory);
                spec.stdin = format!(
                    "{}\n{}\n",
                    json!({"type":"get_state","id":"nexus-session"}),
                    json!({"type":"prompt","id":"nexus-prompt","message":prompt})
                );
                if let Some(session) = &request.session_id {
                    spec.args.extend(["--session".into(), session.clone()]);
                }
            }
            if !request.effort.is_default() {
                let effort = match request.effort {
                    ThinkingEffort::None => "off",
                    ThinkingEffort::Max => "xhigh",
                    other => other.as_str(),
                };
                anyhow::ensure!(
                    ["off", "minimal", "low", "medium", "high", "xhigh"].contains(&effort),
                    "Pi 不支持推理强度 {effort}"
                );
                spec.args.extend(["--thinking".into(), effort.into()]);
            }
            Box::new(PiDecoder {
                _assets: assets,
                inner: nexus_harness_omp::EventDecoder,
                pending_error: None,
            })
        }
        HarnessKind::Kimi => {
            let session = if title.is_some() {
                uuid::Uuid::new_v4().to_string()
            } else {
                request
                    .session_id
                    .clone()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
            };
            spec.args.extend(["--session".into(), session.clone()]);
            if title.is_some() {
                let directory = tempfile::tempdir()?;
                std::fs::write(
                    directory.path().join("system.md"),
                    "Generate only a concise task title from the supplied text.",
                )?;
                std::fs::write(
                    directory.path().join("agent.yaml"),
                    "version: 1\nagent:\n  name: nexus-title\n  system_prompt_path: ./system.md\n  tools: []\n",
                )?;
                spec.args.extend([
                    "--print".into(),
                    "--output-format".into(),
                    "stream-json".into(),
                    "--agent-file".into(),
                    directory
                        .path()
                        .join("agent.yaml")
                        .to_string_lossy()
                        .into_owned(),
                ]);
                spec.stdin = prompt.into();
                assets = Some(directory);
            } else {
                spec.args.push("--wire".into());
                spec.stdin = format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":"nexus-prompt","method":"prompt","params":{"user_input":prompt}})
                );
                if request.permission_mode == PermissionMode::Yolo {
                    spec.args.push("--yolo".into());
                }
            }
            match request.effort {
                ThinkingEffort::Default => {}
                ThinkingEffort::None | ThinkingEffort::Off => {
                    spec.args.push("--no-thinking".into())
                }
                _ => spec.args.push("--thinking".into()),
            }
            Box::new(KimiDecoder {
                _assets: assets,
                session: Some(session),
                mode: request.permission_mode,
                text: String::new(),
                steering: HashSet::new(),
                tool: None,
            })
        }
        HarnessKind::Qoder | HarnessKind::CodeBuddy => {
            let qoder = request.harness == HarnessKind::Qoder;
            spec.args.extend(
                [
                    "--print",
                    "--input-format",
                    "stream-json",
                    "--output-format",
                    "stream-json",
                    "--include-partial-messages",
                    "--permission-prompt-tool",
                    "stdio",
                    "--permission-mode",
                ]
                .map(str::to_owned),
            );
            spec.args.push(
                match (qoder, title.is_some(), request.permission_mode) {
                    (true, true, _) => "dont_ask",
                    (false, true, _) => "dontAsk",
                    (_, _, PermissionMode::Ask) => "default",
                    (true, _, PermissionMode::AutoEdit) => "accept_edits",
                    (false, _, PermissionMode::AutoEdit) => "acceptEdits",
                    (true, _, PermissionMode::Yolo) => "bypass_permissions",
                    (false, _, PermissionMode::Yolo) => "bypassPermissions",
                }
                .into(),
            );
            if qoder && !request.effort.is_default() {
                spec.args
                    .extend(["--reasoning-effort".into(), request.effort.as_str().into()]);
            }
            if !qoder {
                spec.args
                    .extend(["--verbose".into(), "--replay-user-messages".into()]);
                if !request.effort.is_default() {
                    spec.args
                        .extend(["--effort".into(), request.effort.as_str().into()]);
                }
            }
            if title.is_some() {
                spec.args.extend([
                    "--tools".into(),
                    String::new(),
                    "--strict-mcp-config".into(),
                    "--mcp-config".into(),
                    "{\"mcpServers\":{}}".into(),
                ]);
            } else if let Some(session) = &request.session_id {
                spec.args.extend(["--resume".into(), session.clone()]);
            }
            spec.stdin = format!(
                "{}\n",
                json!({"type":"user","session_id":"","parent_tool_use_id":null,"message":{"role":"user","content":prompt}})
            );
            if qoder && title.is_some() {
                let input_format = spec
                    .args
                    .iter()
                    .position(|arg| arg == "--input-format")
                    .unwrap();
                spec.args[input_format + 1] = "text".into();
                spec.stdin = prompt.into();
            }
            let pending_prompt = if qoder && title.is_none() {
                let prompt = serde_json::from_str(spec.stdin.trim())?;
                spec.stdin = format!(
                    "{}\n",
                    json!({"type":"control_request","request_id":"nexus-initialize","request":{"subtype":"initialize","type":"initialize"}})
                );
                Some(InputFrame(prompt))
            } else {
                None
            };
            Box::new(StreamDecoder {
                inner: nexus_harness_claude::EventDecoder,
                harness: request.harness,
                pending_prompt,
            })
        }
        _ => unreachable!("existing adapters have dedicated routing"),
    };
    if let Some(model) = &request.model {
        spec.args.extend(["--model".into(), model.clone()]);
    }
    Ok((spec, decoder))
}

struct PiDecoder {
    _assets: Option<tempfile::TempDir>,
    inner: nexus_harness_omp::EventDecoder,
    pending_error: Option<String>,
}
impl PiDecoder {
    fn finish(&mut self) -> Vec<DecodedEvent> {
        let mut events = self
            .pending_error
            .take()
            .map(DecodedEvent::Error)
            .into_iter()
            .collect::<Vec<_>>();
        events.push(DecodedEvent::TurnCompleted);
        events
    }
}
impl LineDecoder for PiDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let frame: Value = serde_json::from_str(line)?;
        match frame["type"].as_str() {
            // agent_end precedes automatic compaction/retry; only settled is terminal.
            Some("agent_end") => Ok(Vec::new()),
            Some("agent_settled") => Ok(self.finish()),
            Some("response") if frame["command"] == "prompt" && frame["success"] == true => {
                // Extension commands/input handlers may accept without starting an agent.
                Ok(vec![DecodedEvent::WriteStdin(InputFrame(
                    json!({"type":"get_state", "id":"nexus-prompt-state"}),
                ))])
            }
            Some("response")
                if frame["command"] == "get_state"
                    && frame["id"] == "nexus-prompt-state"
                    && frame["success"] == true =>
            {
                let state = &frame["data"];
                Ok(
                    if state["isStreaming"] == false
                        && state["isCompacting"] == false
                        && state["pendingMessageCount"] == 0
                    {
                        self.finish()
                    } else {
                        Vec::new()
                    },
                )
            }
            Some("message_end" | "auto_retry_end") if self._assets.is_some() => {
                let events = self.inner.decode_line(line)?;
                if frame["message"]["role"] == "assistant"
                    && !matches!(
                        frame["message"]["stopReason"].as_str(),
                        Some("error" | "aborted")
                    )
                {
                    self.pending_error = None;
                }
                Ok(events
                    .into_iter()
                    .filter_map(|event| match event {
                        DecodedEvent::Error(message) => {
                            self.pending_error = Some(message);
                            None
                        }
                        other => Some(other),
                    })
                    .collect())
            }
            Some("response") if frame["command"] == "get_state" && frame["success"] == true => Ok(
                match frame["data"]["sessionFile"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                {
                    Some(session) => vec![DecodedEvent::SessionStarted(session.into())],
                    None => vec![
                        DecodedEvent::Error("Pi 未返回会话文件路径。".into()),
                        DecodedEvent::TurnCompleted,
                    ],
                },
            ),
            Some("extension_error") => Ok(vec![
                DecodedEvent::Error(tool_content(&frame)),
                DecodedEvent::TurnCompleted,
            ]),
            _ => self.inner.decode_line(line).map(|events| {
                events
                    .into_iter()
                    .map(|event| match event {
                        DecodedEvent::Status(text) => {
                            DecodedEvent::Status(text.replace("Oh My Pi", "Pi"))
                        }
                        DecodedEvent::Error(text) => {
                            DecodedEvent::Error(text.replace("Oh My Pi", "Pi"))
                        }
                        DecodedEvent::ApprovalRequested(mut approval) => {
                            approval.title = "Pi".into();
                            DecodedEvent::ApprovalRequested(approval)
                        }
                        other => other,
                    })
                    .collect()
            }),
        }
    }
    fn steer(&mut self, id: &str, prompt: &str) -> Option<InputFrame> {
        self.inner.steer(id, prompt)
    }
}

struct StreamDecoder {
    inner: nexus_harness_claude::EventDecoder,
    harness: HarnessKind,
    pending_prompt: Option<InputFrame>,
}
impl LineDecoder for StreamDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let mut frame: Value = serde_json::from_str(line)?;
        if frame["type"] == "control_response"
            && frame["response"]["request_id"] == "nexus-initialize"
        {
            return Ok(if frame["response"]["subtype"] == "success" {
                self.pending_prompt
                    .take()
                    .map(DecodedEvent::WriteStdin)
                    .into_iter()
                    .collect()
            } else {
                vec![
                    DecodedEvent::Error("Qoder 初始化失败。".into()),
                    DecodedEvent::TurnCompleted,
                ]
            });
        }
        if frame["type"] == "control_request" && frame["request"]["subtype"].is_null() {
            frame["request"]["subtype"] = frame["request"]["type"].clone();
        }
        if frame["type"] == "control_cancel" {
            frame["type"] = "control_cancel_request".into();
        }
        self.inner.decode_line(&frame.to_string()).map(|events| {
            events
                .into_iter()
                .map(|event| match event {
                    DecodedEvent::Status(text) => {
                        DecodedEvent::Status(text.replace("Claude", &self.harness.to_string()))
                    }
                    DecodedEvent::Error(text) => {
                        DecodedEvent::Error(text.replace("Claude", &self.harness.to_string()))
                    }
                    DecodedEvent::ApprovalRequested(mut approval)
                        if self.harness == HarnessKind::CodeBuddy =>
                    {
                        // CodeBuddy's SdkPermissionClient reads allowed/reason, unlike
                        // the behavior/message response used by Claude and Qoder.
                        let convert = |frame: &mut InputFrame| {
                            let response = &mut frame.0["response"]["response"];
                            let allowed = response["behavior"] == "allow";
                            let reason = response["message"].clone();
                            let input = response["updatedInput"].clone();
                            *response = json!({"allowed": allowed});
                            if !reason.is_null() {
                                response["reason"] = reason;
                            }
                            if !input.is_null() {
                                response["updatedInput"] = input;
                            }
                        };
                        for option in &mut approval.options {
                            convert(&mut option.response);
                        }
                        convert(&mut approval.cancel);
                        DecodedEvent::ApprovalRequested(approval)
                    }
                    other => other,
                })
                .collect()
        })
    }
    // Qoder does not promise a delivery receipt for streaming user messages.
    // CodeBuddy acknowledges inputs through --replay-user-messages.
    fn steer(&mut self, id: &str, prompt: &str) -> Option<InputFrame> {
        (self.harness == HarnessKind::CodeBuddy)
            .then(|| self.inner.steer(id, prompt))
            .flatten()
    }
}

struct KimiDecoder {
    _assets: Option<tempfile::TempDir>,
    session: Option<String>,
    mode: PermissionMode,
    text: String,
    steering: HashSet<String>,
    tool: Option<(String, String, String)>,
}
impl LineDecoder for KimiDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let frame: Value = serde_json::from_str(line)?;
        let mut events = Vec::new();
        if let Some(session) = self.session.take() {
            events.push(DecodedEvent::SessionStarted(session));
        }
        if let Some(id) = frame["id"].as_str()
            && (frame.get("result").is_some() || frame.get("error").is_some())
        {
            let error = frame["error"]["message"].as_str();
            if self.steering.remove(id) {
                events.push(if let Some(message) = error {
                    DecodedEvent::InputRejected {
                        id: id.into(),
                        message: message.into(),
                    }
                } else {
                    DecodedEvent::InputAccepted(id.into())
                });
            } else if id == "nexus-prompt" {
                if let Some(message) = error {
                    events.push(DecodedEvent::Error(message.into()));
                } else if frame["result"]["status"] != "finished" {
                    events.push(DecodedEvent::Error(format!(
                        "Kimi 轮次未完成：{}",
                        frame["result"]["status"]
                    )));
                }
                if !self.text.is_empty() {
                    events.push(DecodedEvent::MessageCompleted(std::mem::take(
                        &mut self.text,
                    )));
                }
                events.push(DecodedEvent::TurnCompleted);
            }
            return Ok(events);
        }
        if frame["role"] == "assistant" {
            let text = if let Some(text) = frame["content"].as_str() {
                text.into()
            } else {
                frame["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|p| p["type"] == "text")
                    .filter_map(|p| p["text"].as_str())
                    .collect::<String>()
            };
            if !text.is_empty() {
                events.push(DecodedEvent::MessageCompleted(text));
            }
            return Ok(events);
        }
        let event = if frame.get("params").is_some() {
            &frame["params"]
        } else {
            &frame
        };
        let payload = &event["payload"];
        if event["type"] != "ToolCallPart"
            && let Some((id, name, summary)) = self.tool.take()
        {
            events.push(DecodedEvent::ToolStarted { id, name, summary });
        }
        match event["type"].as_str() {
            Some("ContentPart") if payload["type"] == "text" => {
                if let Some(text) = payload["text"].as_str() { self.text.push_str(text); events.push(DecodedEvent::TextDelta(text.into())); }
            }
            Some("StepBegin" | "TurnEnd") if !self.text.is_empty() => events.push(DecodedEvent::MessageCompleted(std::mem::take(&mut self.text))),
            Some("ToolCall") => self.tool = Some((string(payload, "id"), string(&payload["function"], "name"), string(&payload["function"], "arguments"))),
            Some("ToolCallPart") => if let Some((_, _, arguments)) = &mut self.tool { arguments.push_str(&string(payload,"arguments_part")); },
            Some("ToolResult") => events.push(DecodedEvent::ToolCompleted { id: string(payload, "tool_call_id"), output: tool_content(&payload["return_value"]), is_error: payload["return_value"]["is_error"].as_bool().unwrap_or(false) }),
            Some("ApprovalRequest") => {
                let response = |answer: &str| InputFrame(json!({"jsonrpc":"2.0","id":frame["id"],"result":{"request_id":payload["id"],"response":answer}}));
                let deny = response("reject");
                if self.mode == PermissionMode::AutoEdit && matches!(payload["sender"].as_str(), Some("WriteFile" | "StrReplaceFile")) {
                    events.push(DecodedEvent::WriteStdin(response("approve")));
                } else {
                    events.push(DecodedEvent::ApprovalRequested(ApprovalPrompt { id: string(payload,"id"), title: string(payload,"sender"), details: string(payload,"description"),
                        options: vec![ApprovalOption { label:"Approve".into(), response:response("approve") }, ApprovalOption { label:"Deny".into(), response:deny.clone() }], cancel:deny, timeout_ms:None }));
                }
            }
            Some("ApprovalResponse" | "ApprovalRequestResolved") => events.push(DecodedEvent::ApprovalResolved(string(payload,"request_id"))),
            _ if frame["method"] == "request" => events.push(DecodedEvent::WriteStdin(InputFrame(json!({"jsonrpc":"2.0","id":frame["id"],"error":{"code":-32601,"message":"Unsupported request"}})))),
            _ => {},
        }
        Ok(events)
    }
    fn steer(&mut self, id: &str, prompt: &str) -> Option<InputFrame> {
        self.steering.insert(id.into());
        Some(InputFrame(
            json!({"jsonrpc":"2.0","id":id,"method":"steer","params":{"user_input":prompt}}),
        ))
    }
}
fn string(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(harness: HarnessKind) -> StartRun {
        StartRun {
            run_id: uuid::Uuid::new_v4(),
            task_id: uuid::Uuid::new_v4(),
            harness,
            executable: harness.default_executable().into(),
            cwd: ".".into(),
            prompt: "private prompt".into(),
            model: Some("provider/model".into()),
            effort: ThinkingEffort::Default,
            session_id: Some("session-1".into()),
            permission_mode: PermissionMode::Ask,
            title_generation: None,
            environment: Vec::new(),
        }
    }
    #[test]
    fn launch_modes_resume_and_model_do_not_leak_prompts_to_argv() {
        for harness in [
            HarnessKind::Pi,
            HarnessKind::Kimi,
            HarnessKind::Qoder,
            HarnessKind::CodeBuddy,
        ] {
            let request = request(harness);
            let (spec, _decoder) = prepare(&request, Path::new("."), None).unwrap();
            assert!(!spec.args.iter().any(|arg| arg.contains("private prompt")));
            assert!(
                spec.args
                    .windows(2)
                    .any(|pair| pair == ["--model", "provider/model"])
            );
            assert!(
                spec.args
                    .windows(2)
                    .any(|pair| (pair[0] == "--session" || pair[0] == "--resume")
                        && pair[1] == "session-1")
            );
            assert!(!spec.args.iter().any(|arg| arg == "--yolo"
                || arg == "bypassPermissions"
                || arg == "bypass_permissions"));
        }
    }
    #[test]
    fn native_reasoning_options_use_each_cli_flag() {
        for (harness, flag) in [
            (HarnessKind::Pi, "--thinking"),
            (HarnessKind::Qoder, "--reasoning-effort"),
            (HarnessKind::CodeBuddy, "--effort"),
        ] {
            let mut request = request(harness);
            request.effort = ThinkingEffort::High;
            let (spec, _) = prepare(&request, Path::new("."), None).unwrap();
            assert!(spec.args.windows(2).any(|pair| pair == [flag, "high"]));
        }
    }

    #[test]
    fn pi_uses_session_file_and_waits_for_settled_after_retry() {
        let (_, mut decoder) = prepare(&request(HarnessKind::Pi), Path::new("."), None).unwrap();
        assert_eq!(decoder.decode_line(r#"{"type":"response","command":"get_state","success":true,"data":{"sessionId":"short","sessionFile":"/tmp/session.jsonl"}}"#).unwrap(), vec![DecodedEvent::SessionStarted("/tmp/session.jsonl".into())]);
        assert!(decoder.decode_line(r#"{"type":"message_end","message":{"role":"assistant","stopReason":"error","errorMessage":"transient"}}"#).unwrap().is_empty());
        decoder.decode_line(r#"{"type":"message_end","message":{"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"recovered"}]}}"#).unwrap();
        assert!(
            decoder
                .decode_line(r#"{"type":"agent_end","willRetry":true}"#)
                .unwrap()
                .is_empty()
        );
        assert!(
            decoder
                .decode_line(r#"{"type":"agent_end","willRetry":false}"#)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            decoder.decode_line(r#"{"type":"agent_settled"}"#).unwrap(),
            vec![DecodedEvent::TurnCompleted]
        );
        assert_eq!(
            decoder.steer("next", "follow up").unwrap().0["type"],
            "steer"
        );
        assert!(pi_version_supported("pi 0.85.1"));
        assert!(!pi_version_supported("0.85.0"));
        assert!(!pi_version_supported("unknown"));
    }
    #[test]
    fn pi_accepted_extension_command_finishes_only_when_session_is_idle() {
        let (_, mut decoder) = prepare(&request(HarnessKind::Pi), Path::new("."), None).unwrap();
        let events = decoder
            .decode_line(
                r#"{"type":"response","id":"nexus-prompt","command":"prompt","success":true}"#,
            )
            .unwrap();
        assert!(
            matches!(&events[0], DecodedEvent::WriteStdin(frame) if frame.0["id"] == "nexus-prompt-state")
        );
        for (streaming, compacting, pending) in
            [(true, false, 0), (false, true, 0), (false, false, 1)]
        {
            assert!(decoder.decode_line(&json!({"type":"response","command":"get_state","id":"nexus-prompt-state","success":true,"data":{"isStreaming":streaming,"isCompacting":compacting,"pendingMessageCount":pending}}).to_string()).unwrap().is_empty());
        }
        assert_eq!(decoder.decode_line(r#"{"type":"response","command":"get_state","id":"nexus-prompt-state","success":true,"data":{"isStreaming":false,"isCompacting":false,"pendingMessageCount":0}}"#).unwrap(), vec![DecodedEvent::TurnCompleted]);
    }

    #[test]
    fn pi_permission_extension_allows_only_the_selected_policy() {
        let source = serde_json::to_string(include_str!("pi-permissions.mjs")).unwrap();
        let script = format!(
            "const source = {source};\n{}",
            r#"
import assert from 'node:assert/strict';
for (const mode of ['ask', 'auto_edit', 'yolo']) {
    const module = await import('data:text/javascript;base64,' + Buffer.from(`const mode = ${JSON.stringify(mode)};\n${source}`).toString('base64'));
    let gate;
    module.default({on(event, callback) { assert.equal(event, 'tool_call'); gate = callback; }});
    for (const toolName of ['read', 'write', 'edit', 'bash', 'unknown-tool']) {
        const automatic = mode === 'yolo' || toolName === 'read' || mode === 'auto_edit' && ['write', 'edit'].includes(toolName);
        for (const hasUI of [false, true]) {
            for (const confirmed of [false, true]) {
                let prompts = 0;
                const result = await gate({toolName, input: {}}, {hasUI, ui: {async confirm() { prompts++; return confirmed; }}});
                assert.equal(result?.block, automatic || hasUI && confirmed ? undefined : true);
                assert.equal(prompts, automatic || !hasUI ? 0 : 1);
            }
        }
    }
}
"#
        );
        let output = std::process::Command::new(
            resolve_executable("node")
                .expect("Node.js is required by the workspace frontend and Pi permission tests"),
        )
        .args(["--input-type=module", "-e", &script])
        .output()
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn pi_permission_extension_lives_as_long_as_decoder() {
        let (spec, decoder) = prepare(&request(HarnessKind::Pi), Path::new("."), None).unwrap();
        let extension = spec
            .args
            .windows(2)
            .find(|pair| pair[0] == "--extension")
            .unwrap()[1]
            .clone();
        assert!(Path::new(&extension).exists());
        assert!(
            std::fs::read_to_string(&extension)
                .unwrap()
                .starts_with("const mode = \"ask\";")
        );
        drop(decoder);
        assert!(!Path::new(&extension).exists());
    }
    #[test]
    fn kimi_wire_streams_tools_with_complete_arguments_and_correlates_steer() {
        let (_, mut decoder) = prepare(&request(HarnessKind::Kimi), Path::new("."), None).unwrap();
        let events = decoder.decode_line(r#"{"jsonrpc":"2.0","method":"event","params":{"type":"ContentPart","payload":{"type":"text","text":"hello"}}}"#).unwrap();
        assert!(events.contains(&DecodedEvent::SessionStarted("session-1".into())));
        assert!(events.contains(&DecodedEvent::TextDelta("hello".into())));
        decoder.decode_line(r#"{"method":"event","params":{"type":"ToolCall","payload":{"type":"function","id":"t1","function":{"name":"Shell","arguments":"{"}}}}"#).unwrap();
        decoder.decode_line(r#"{"method":"event","params":{"type":"ToolCallPart","payload":{"arguments_part":"\"command\":\"pwd\"}"}}}"#).unwrap();
        let events = decoder.decode_line(r#"{"method":"event","params":{"type":"ToolResult","payload":{"tool_call_id":"t1","return_value":{"is_error":false,"output":"/project","message":"","display":[]}}}}"#).unwrap();
        assert!(
            matches!(&events[0], DecodedEvent::ToolStarted { id, name, summary } if id == "t1" && name == "Shell" && summary == r#"{"command":"pwd"}"#)
        );
        assert!(
            matches!(&events[1], DecodedEvent::ToolCompleted { id, is_error: false, .. } if id == "t1")
        );
        assert_eq!(
            decoder.steer("next", "follow up").unwrap().0["method"],
            "steer"
        );
        assert_eq!(
            decoder.decode_line(r#"{"id":"next","result":{}}"#).unwrap(),
            vec![DecodedEvent::InputAccepted("next".into())]
        );
        let events = decoder
            .decode_line(r#"{"id":"nexus-prompt","result":{"status":"finished"}}"#)
            .unwrap();
        assert!(events.contains(&DecodedEvent::MessageCompleted("hello".into())));
        assert!(events.contains(&DecodedEvent::TurnCompleted));
    }
    #[test]
    fn kimi_approval_uses_rpc_id_and_native_request_id_and_denies_cancel() {
        let (_, mut decoder) = prepare(&request(HarnessKind::Kimi), Path::new("."), None).unwrap();
        let events = decoder.decode_line(r#"{"jsonrpc":"2.0","method":"request","id":"rpc-id","params":{"type":"ApprovalRequest","payload":{"id":"native-id","sender":"Shell","description":"Run command","tool_call_id":"t1"}}}"#).unwrap();
        let approval = events
            .into_iter()
            .find_map(|event| {
                if let DecodedEvent::ApprovalRequested(approval) = event {
                    Some(approval)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(approval.cancel.0["id"], "rpc-id");
        assert_eq!(approval.cancel.0["result"]["request_id"], "native-id");
        assert_eq!(approval.cancel.0["result"]["response"], "reject");
        assert_eq!(
            approval.options[0].response.0["result"]["response"],
            "approve"
        );
    }
    #[test]
    fn qoder_initializes_before_prompt_and_stream_protocol_preserves_permission_payloads() {
        for harness in [HarnessKind::Qoder, HarnessKind::CodeBuddy] {
            let (spec, mut decoder) = prepare(&request(harness), Path::new("."), None).unwrap();
            if harness == HarnessKind::Qoder {
                assert_eq!(
                    serde_json::from_str::<Value>(spec.stdin.trim()).unwrap()["request"]["type"],
                    "initialize"
                );
                let events = decoder.decode_line(r#"{"type":"control_response","response":{"subtype":"success","request_id":"nexus-initialize","response":{}}}"#).unwrap();
                assert!(
                    matches!(&events[0], DecodedEvent::WriteStdin(frame) if frame.0["message"]["content"] == "private prompt")
                );
            }
            let events = decoder.decode_line(r#"{"type":"control_request","request_id":"p1","request":{"type":"can_use_tool","tool_name":"Bash","input":{"command":"git status"}}}"#).unwrap();
            let DecodedEvent::ApprovalRequested(approval) = &events[0] else {
                panic!("approval")
            };
            assert_eq!(
                approval.options[0].response.0["response"]["response"]["updatedInput"]["command"],
                "git status"
            );
            if harness == HarnessKind::CodeBuddy {
                let accepted = &approval.options[0].response.0["response"]["response"];
                assert_eq!(accepted["allowed"], true);
                assert!(accepted.get("behavior").is_none());
                assert_eq!(approval.cancel.0["response"]["response"]["allowed"], false);
                assert!(approval.cancel.0["response"]["response"]["reason"].is_string());
            } else {
                assert_eq!(
                    approval.cancel.0["response"]["response"]["behavior"],
                    "deny"
                );
            }
            assert!(matches!(
                decoder
                    .decode_line(r#"{"type":"result","is_error":true,"errors":["denied"]}"#)
                    .unwrap()
                    .as_slice(),
                [DecodedEvent::Error(_), DecodedEvent::TurnCompleted]
            ));
        }
    }
    #[test]
    fn title_requests_disable_tools_and_kimi_print_messages_decode() {
        for harness in [
            HarnessKind::Pi,
            HarnessKind::Kimi,
            HarnessKind::Qoder,
            HarnessKind::CodeBuddy,
        ] {
            let (spec, mut decoder) =
                prepare(&request(harness), Path::new("."), Some("Title please")).unwrap();
            assert!(!spec.args.iter().any(|arg| arg == "session-1"));
            if harness == HarnessKind::Pi {
                assert!(spec.args.contains(&"--no-tools".into()));
            } else if harness == HarnessKind::Kimi {
                let agent = &spec
                    .args
                    .windows(2)
                    .find(|pair| pair[0] == "--agent-file")
                    .unwrap()[1];
                assert!(
                    std::fs::read_to_string(agent)
                        .unwrap()
                        .contains("tools: []")
                );
                assert!(
                    decoder
                        .decode_line(r#"{"role":"assistant","content":"A short title"}"#)
                        .unwrap()
                        .contains(&DecodedEvent::MessageCompleted("A short title".into()))
                );
            } else {
                assert!(
                    spec.args
                        .windows(2)
                        .any(|pair| pair[0] == "--tools" && pair[1].is_empty())
                );
            }
        }
    }
}
