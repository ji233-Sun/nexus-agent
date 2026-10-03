use nexus_domain::{
    ModelAvailability, ModelDescriptor, ModelSource, PermissionMode, ThinkingEffort,
};
use nexus_harness_core::{DecodedEvent, InputFrame, LaunchSpec, LineDecoder, tool_content};
use nexus_protocol::{StartRun, TextGenerationConfig};
use serde_json::Value;
use std::path::Path;

pub fn prepare_run(run: &StartRun, cwd: &Path) -> (LaunchSpec, EventDecoder) {
    let mut args = print_args(run.model.as_deref(), run.effort);
    match run.permission_mode {
        PermissionMode::Ask => {}
        PermissionMode::AutoEdit => {
            args.extend(["--permission-mode".into(), "auto-accept".into()]);
        }
        PermissionMode::Yolo => args.push("--yolo".into()),
    }
    if let Some(session) = &run.session_id {
        args.extend(["--resume".into(), session.clone()]);
    }
    (
        LaunchSpec {
            executable: run.executable.clone().into(),
            cwd: cwd.into(),
            args,
            stdin: run.prompt.clone(),
        },
        EventDecoder::default(),
    )
}

pub fn prepare_text_generation(
    request: &TextGenerationConfig,
    cwd: &Path,
    prompt: &str,
) -> (LaunchSpec, EventDecoder) {
    let mut args = print_args(request.model.as_deref(), request.effort);
    args.push("--no-session".into());
    (
        LaunchSpec {
            executable: request.executable.clone().into(),
            cwd: cwd.into(),
            args,
            stdin: prompt.into(),
        },
        EventDecoder::default(),
    )
}

fn print_args(model: Option<&str>, effort: ThinkingEffort) -> Vec<String> {
    let mut args = [
        "--print",
        "--output-format",
        "json",
        "--skip-onboarding",
        "--no-auto-update",
        "--trust",
    ]
    .map(Into::into)
    .to_vec();
    if let Some(model) = model {
        args.extend(["--model".into(), model.into()]);
    }
    if matches!(
        effort,
        ThinkingEffort::Low
            | ThinkingEffort::Medium
            | ThinkingEffort::High
            | ThinkingEffort::XHigh
            | ThinkingEffort::Max
            | ThinkingEffort::Ultra
    ) {
        args.extend(["--effort".into(), effort.as_str().into()]);
    }
    args
}

pub fn parse_catalog(output: &str) -> Result<Vec<ModelDescriptor>, String> {
    let mut models = Vec::new();
    for line in output.lines() {
        let Some((id, description)) = line.split_once("  ") else {
            continue;
        };
        let id = id.trim();
        if id.is_empty()
            || id.chars().any(char::is_whitespace)
            || id.ends_with(':')
            || description.trim().is_empty()
        {
            continue;
        }
        models.push(ModelDescriptor {
            id: id.into(),
            display_name: id.into(),
            source: ModelSource::CommandCodeCli,
            availability: ModelAvailability::Unknown,
            provider: id.split_once('/').map(|(provider, _)| provider.into()),
            is_default: description.contains("(default)"),
            supported_reasoning_efforts: Vec::new(),
            default_reasoning_effort: None,
        });
    }
    if models.is_empty() {
        Err("Command Code 未返回可选模型。".into())
    } else {
        Ok(models)
    }
}

#[derive(Default)]
pub struct EventDecoder {
    session_reported: bool,
    message_reported: bool,
}

impl LineDecoder for EventDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let frame: Value = serde_json::from_str(line)?;
        let mut events = Vec::new();
        match frame["type"].as_str() {
            Some("event") => {
                let event = &frame["event"];
                match event["type"].as_str() {
                    Some("run_start") => {
                        self.report_session(event["sessionId"].as_str(), &mut events)
                    }
                    Some("text_delta") => {
                        if let Some(delta) = event["delta"].as_str() {
                            events.push(DecodedEvent::TextDelta(delta.into()));
                        }
                    }
                    Some("message_end") => {
                        let text = event["content"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|block| block["type"] == "text")
                            .filter_map(|block| block["text"].as_str())
                            .collect::<String>();
                        if !text.is_empty() {
                            self.message_reported = true;
                            events.push(DecodedEvent::MessageCompleted(text));
                        }
                    }
                    Some("tool_queued") => {
                        if let (Some(id), Some(name)) =
                            (event["toolCallId"].as_str(), event["toolName"].as_str())
                        {
                            events.push(DecodedEvent::ToolStarted {
                                id: id.into(),
                                name: name.into(),
                                summary: name.into(),
                            });
                        }
                    }
                    Some("tool_completed") => tool_finished(event, false, &mut events),
                    Some("tool_errored" | "tool_denied" | "tool_hook_blocked") => {
                        tool_finished(event, true, &mut events)
                    }
                    _ => {}
                }
            }
            Some("result") => {
                self.report_session(frame["sessionId"].as_str(), &mut events);
                match frame["subtype"].as_str() {
                    Some("success") => {
                        if !self.message_reported
                            && let Some(text) =
                                frame["finalText"].as_str().filter(|text| !text.is_empty())
                        {
                            events.push(DecodedEvent::MessageCompleted(text.into()));
                        }
                        events.push(DecodedEvent::TurnCompleted);
                    }
                    Some("max_turns") => {
                        if !self.message_reported
                            && let Some(text) =
                                frame["finalText"].as_str().filter(|text| !text.is_empty())
                        {
                            events.push(DecodedEvent::MessageCompleted(text.into()));
                        }
                        events.push(DecodedEvent::Error(
                            "Command Code 达到最大轮次，请续聊以继续任务。".into(),
                        ));
                    }
                    Some("error") => events.push(DecodedEvent::Error(
                        frame["error"]
                            .as_str()
                            .or_else(|| frame["error"]["message"].as_str())
                            .unwrap_or("Command Code 执行失败。")
                            .into(),
                    )),
                    _ => events.push(DecodedEvent::Error("Command Code 返回了未知结果。".into())),
                }
            }
            _ => {}
        }
        Ok(events)
    }

    fn steer(&mut self, _id: &str, _prompt: &str) -> Option<InputFrame> {
        None
    }
}

impl EventDecoder {
    fn report_session(&mut self, session: Option<&str>, events: &mut Vec<DecodedEvent>) {
        if !self.session_reported
            && let Some(session) = session.filter(|session| !session.is_empty())
        {
            self.session_reported = true;
            events.push(DecodedEvent::SessionStarted(session.into()));
        }
    }
}

fn tool_finished(event: &Value, is_error: bool, events: &mut Vec<DecodedEvent>) {
    if let Some(id) = event["toolCallId"].as_str() {
        let output = if is_error {
            event
                .get("error")
                .or_else(|| event.get("hookOutput"))
                .filter(|value| !value.is_null())
                .map(tool_content)
                .unwrap_or_else(|| "Command Code 未执行该工具。".into())
        } else {
            tool_content(&event["result"])
        };
        events.push(DecodedEvent::ToolCompleted {
            id: id.into(),
            output,
            is_error,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_domain::HarnessKind;
    use serde_json::json;

    fn run(mode: PermissionMode) -> StartRun {
        serde_json::from_value(json!({
            "run_id": uuid::Uuid::new_v4(),
            "task_id": uuid::Uuid::new_v4(),
            "harness": HarnessKind::CommandCode,
            "executable": "cmd",
            "cwd": "/tmp",
            "prompt": "private prompt",
            "effort": "high",
            "permission_mode": mode,
        }))
        .unwrap()
    }

    #[test]
    fn headless_launch_keeps_prompt_on_stdin_and_maps_permissions() {
        for (mode, flag) in [
            (PermissionMode::Ask, None),
            (PermissionMode::AutoEdit, Some("--permission-mode")),
            (PermissionMode::Yolo, Some("--yolo")),
        ] {
            let mut run = run(mode);
            run.session_id = Some("saved-session".into());
            run.model = Some("deepseek/deepseek-v4-flash".into());
            let (spec, _) = prepare_run(&run, Path::new("/tmp"));
            assert_eq!(spec.stdin, "private prompt");
            assert!(!spec.args.iter().any(|arg| arg.contains("private prompt")));
            assert!(
                spec.args
                    .windows(2)
                    .any(|args| args == ["--resume", "saved-session"])
            );
            assert!(
                spec.args
                    .windows(2)
                    .any(|args| args == ["--model", "deepseek/deepseek-v4-flash"])
            );
            assert!(
                spec.args
                    .windows(2)
                    .any(|args| args == ["--effort", "high"])
            );
            assert_eq!(
                spec.args
                    .iter()
                    .find(|arg| matches!(arg.as_str(), "--permission-mode" | "--yolo"))
                    .map(String::as_str),
                flag
            );
            if mode == PermissionMode::AutoEdit {
                assert!(
                    spec.args
                        .windows(2)
                        .any(|args| args == ["--permission-mode", "auto-accept"])
                );
            }
        }
        let request = TextGenerationConfig {
            harness: HarnessKind::CommandCode,
            executable: "cmd".into(),
            model: None,
            effort: ThinkingEffort::Default,
            environment: Vec::new(),
        };
        let (spec, _) = prepare_text_generation(&request, Path::new("/tmp"), "title prompt");
        assert!(spec.args.contains(&"--no-session".into()));
        assert_eq!(spec.stdin, "title prompt");
    }

    #[test]
    fn model_list_uses_only_catalog_rows() {
        let models = parse_catalog("Available models  ·  2 models\n\nOpen Source\n\ndeepseek/deepseek-v4-flash  fast reasoning (default)\ngpt-5.5  frontier model\n\nPass the full id, or just the short name\nDocs:  https://commandcode.ai/docs\n").unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "deepseek/deepseek-v4-flash");
        assert_eq!(models[0].provider.as_deref(), Some("deepseek"));
        assert!(models[0].is_default);
        assert_eq!(models[1].id, "gpt-5.5");
        assert_eq!(models[1].source, ModelSource::CommandCodeCli);
    }

    #[test]
    fn json_stream_preserves_session_text_tools_and_failure() {
        let mut decoder = EventDecoder::default();
        assert_eq!(
            decoder
                .decode_line(
                    r#"{"type":"event","event":{"type":"run_start","sessionId":"session-1"}}"#
                )
                .unwrap(),
            vec![DecodedEvent::SessionStarted("session-1".into())]
        );
        assert_eq!(
            decoder
                .decode_line(r#"{"type":"event","event":{"type":"text_delta","delta":"hello"}}"#)
                .unwrap(),
            vec![DecodedEvent::TextDelta("hello".into())]
        );
        let started = decoder.decode_line(r#"{"type":"event","event":{"type":"tool_queued","toolCallId":"tool-1","toolName":"read_file"}}"#).unwrap();
        assert!(
            matches!(&started[..], [DecodedEvent::ToolStarted { id, name, .. }] if id == "tool-1" && name == "read_file")
        );
        let finished = decoder.decode_line(r#"{"type":"event","event":{"type":"tool_completed","toolCallId":"tool-1","result":[{"type":"text","text":"file"}]}}"#).unwrap();
        assert!(
            matches!(&finished[..], [DecodedEvent::ToolCompleted { id, is_error: false, .. }] if id == "tool-1")
        );
        let denied = decoder
            .decode_line(r#"{"type":"event","event":{"type":"tool_denied","toolCallId":"tool-2"}}"#)
            .unwrap();
        assert!(
            matches!(&denied[..], [DecodedEvent::ToolCompleted { output, is_error: true, .. }] if output == "Command Code 未执行该工具。")
        );
        assert!(
            decoder
                .decode_line(r#"{"type":"event","event":{"type":"future_event"}}"#)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            decoder.decode_line(r#"{"type":"result","subtype":"success","sessionId":"session-1","finalText":"hello"}"#).unwrap(),
            vec![DecodedEvent::MessageCompleted("hello".into()), DecodedEvent::TurnCompleted]
        );
        let mut decoder = EventDecoder::default();
        assert_eq!(
            decoder.decode_line(r#"{"type":"event","event":{"type":"message_end","content":[{"type":"text","text":"done"}]}}"#).unwrap(),
            vec![DecodedEvent::MessageCompleted("done".into())]
        );
        assert_eq!(
            decoder
                .decode_line(r#"{"type":"result","subtype":"success","finalText":"done"}"#)
                .unwrap(),
            vec![DecodedEvent::TurnCompleted]
        );
        let failure = EventDecoder::default()
            .decode_line(
                r#"{"type":"result","subtype":"error","error":"auth required","finalText":""}"#,
            )
            .unwrap();
        assert_eq!(failure, vec![DecodedEvent::Error("auth required".into())]);
    }
}
