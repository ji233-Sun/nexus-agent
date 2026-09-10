mod kimi;
use nexus_domain::{HarnessKind, UserAskAnswer};
use nexus_harness_core::{DecodedEvent, InputFrame, LaunchSpec, LineDecoder};
use nexus_protocol::StartRun;
use serde_json::{Value, json};
use std::path::Path;

pub fn prepare_run(run: &StartRun, cwd: &Path) -> (LaunchSpec, Box<dyn LineDecoder>) {
    if run.harness == HarnessKind::Kimi {
        let (spec, decoder) = kimi::prepare(run, cwd);
        return (spec, Box::new(decoder));
    }
    let qoder = matches!(run.harness, HarnessKind::Qoder | HarnessKind::QoderCn);
    let mut args: Vec<String> = [
        "--print",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--include-partial-messages",
        "--permission-mode",
        "default",
    ]
    .map(Into::into)
    .to_vec();
    if qoder {
        args.extend(["--permission-prompt-tool".into(), "stdio".into()]);
    } else {
        args.extend(["--verbose".into(), "--replay-user-messages".into()]);
    }
    if let Some(session) = &run.session_id {
        args.extend(["--resume".into(), session.into()]);
    }
    if let Some(model) = &run.model {
        args.extend(["--model".into(), model.into()]);
    }
    append_effort_args(&mut args, run.harness, run.effort);
    let initialize = json!({"type":"control_request","request_id":"nexus-initialize","request":{"subtype":"initialize"}});
    (
        LaunchSpec {
            executable: run.executable.clone().into(),
            cwd: cwd.into(),
            args,
            stdin: format!("{initialize}\n"),
        },
        Box::new(StreamDecoder {
            inner: Default::default(),
            prompt: Some(run.prompt.clone()),
            permission: run.permission_mode,
            qoder,
        }),
    )
}
pub fn append_effort_args(
    args: &mut Vec<String>,
    harness: HarnessKind,
    effort: nexus_domain::ThinkingEffort,
) {
    let qoder = matches!(harness, HarnessKind::Qoder | HarnessKind::QoderCn);
    if matches!(
        effort,
        nexus_domain::ThinkingEffort::Off | nexus_domain::ThinkingEffort::None
    ) {
        args.extend(if qoder {
            ["--thinking".into(), "disabled".into()]
        } else {
            [
                "--settings".into(),
                r#"{"alwaysThinkingEnabled":false}"#.into(),
            ]
        });
    } else if !effort.is_default() {
        args.extend([
            if qoder {
                "--reasoning-effort"
            } else {
                "--effort"
            }
            .into(),
            effort.as_str().into(),
        ]);
    }
}

struct StreamDecoder {
    inner: nexus_harness_claude::EventDecoder,
    prompt: Option<String>,
    permission: nexus_domain::PermissionMode,
    qoder: bool,
}
impl LineDecoder for StreamDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let frame: Value = serde_json::from_str(line)?;
        if frame["type"] == "control_response"
            && frame["response"]["request_id"] == "nexus-initialize"
        {
            if frame["response"]["subtype"] != "success" {
                return Ok(vec![
                    DecodedEvent::Error("CLI 初始化失败，请检查登录与配置。".into()),
                    DecodedEvent::TurnCompleted,
                ]);
            }
            return Ok(self
                .prompt
                .take()
                .map(|prompt| {
                    vec![DecodedEvent::WriteStdin(InputFrame(
                        json!({"type":"user","message":{"role":"user","content":prompt}}),
                    ))]
                })
                .unwrap_or_default());
        }
        if frame["type"] == "control_request"
            && frame["request"]["subtype"] == "can_use_tool"
            && frame["request"]["tool_name"] != "AskUserQuestion"
        {
            let edit = matches!(
                frame["request"]["tool_name"].as_str(),
                Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit")
            );
            if self.permission == nexus_domain::PermissionMode::Yolo
                || (self.permission == nexus_domain::PermissionMode::AutoEdit && edit)
            {
                return Ok(vec![DecodedEvent::WriteStdin(InputFrame(
                    json!({"type":"control_response","response":{"subtype":"success","request_id":frame["request_id"],"response":{"behavior":"allow","updatedInput":frame["request"]["input"]}}}),
                ))]);
            }
        }
        self.inner.decode_line(line)
    }
    fn steer(&mut self, id: &str, prompt: &str) -> Option<InputFrame> {
        if self.qoder {
            None
        } else {
            self.inner.steer(id, prompt)
        }
    }
    fn answer_user_ask(&mut self, id: &str, answers: &[UserAskAnswer]) -> Option<InputFrame> {
        self.inner.answer_user_ask(id, answers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_domain::{PermissionMode, ThinkingEffort, UserAskAnswerValue};

    fn stream(permission: PermissionMode) -> StreamDecoder {
        StreamDecoder {
            inner: Default::default(),
            prompt: Some("hello".into()),
            permission,
            qoder: true,
        }
    }

    #[test]
    fn initialize_sends_prompt_only_after_success_and_failure_finishes() {
        let mut decoder = stream(PermissionMode::Ask);
        let frames = decoder.decode_line(r#"{"type":"control_response","response":{"request_id":"nexus-initialize","subtype":"success"}}"#).unwrap();
        assert!(
            matches!(&frames[..], [DecodedEvent::WriteStdin(frame)] if frame.0["message"]["content"] == "hello")
        );
        assert!(decoder.decode_line(r#"{"type":"control_response","response":{"request_id":"nexus-initialize","subtype":"success"}}"#).unwrap().is_empty());
        let frames = stream(PermissionMode::Ask).decode_line(r#"{"type":"control_response","response":{"request_id":"nexus-initialize","subtype":"error"}}"#).unwrap();
        assert!(matches!(
            &frames[..],
            [DecodedEvent::Error(_), DecodedEvent::TurnCompleted]
        ));
    }

    #[test]
    fn native_permissions_keep_questions_interactive_in_yolo() {
        for (mode, tool, automatic) in [
            (PermissionMode::Ask, "Write", false),
            (PermissionMode::AutoEdit, "Write", true),
            (PermissionMode::AutoEdit, "Bash", false),
            (PermissionMode::Yolo, "Bash", true),
        ] {
            let frames = stream(mode).decode_line(&json!({"type":"control_request","request_id":"approval","request":{"subtype":"can_use_tool","tool_name":tool,"input":{"command":"echo hi"}}}).to_string()).unwrap();
            assert_eq!(
                matches!(&frames[..], [DecodedEvent::WriteStdin(_)]),
                automatic
            );
            if !automatic {
                assert!(matches!(&frames[..], [DecodedEvent::ApprovalRequested(_)]));
            }
        }
        let mut decoder = stream(PermissionMode::Yolo);
        let frames = decoder.decode_line(&json!({"type":"control_request","request_id":"ask","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which?","header":"Choice","multiSelect":true,"options":[{"label":"A","description":"first"},{"label":"B","description":"second"}]}]}}}).to_string()).unwrap();
        let [DecodedEvent::UserAskRequested(ask)] = &frames[..] else {
            panic!("{frames:?}")
        };
        let answer = decoder
            .answer_user_ask(
                "ask",
                &[UserAskAnswer {
                    question_id: ask.questions[0].id.clone(),
                    value: UserAskAnswerValue::Selected(
                        ask.questions[0]
                            .options
                            .iter()
                            .map(|o| o.id.clone())
                            .collect(),
                    ),
                }],
            )
            .unwrap();
        assert_eq!(
            answer.0["response"]["response"]["updatedInput"]["answers"]["Which?"],
            "A, B"
        );
    }

    #[test]
    fn native_launch_uses_supported_flags_and_disables_thinking_explicitly() {
        for harness in [
            HarnessKind::Qoder,
            HarnessKind::QoderCn,
            HarnessKind::Codebuddy,
        ] {
            let run: StartRun = serde_json::from_value(json!({
                "run_id":uuid::Uuid::new_v4(),"task_id":uuid::Uuid::new_v4(),"cwd":"/tmp","prompt":"hello","permission_mode":"ask","effort":ThinkingEffort::Off,
                "harness":harness,"executable":harness.default_executable()
            })).unwrap();
            let (spec, _) = prepare_run(&run, Path::new("/tmp"));
            if harness == HarnessKind::Codebuddy {
                assert!(
                    spec.args
                        .windows(2)
                        .any(|a| a == ["--settings", r#"{"alwaysThinkingEnabled":false}"#])
                );
            } else {
                assert!(!spec.args.iter().any(|a| a == "--verbose"));
                assert!(
                    spec.args
                        .windows(2)
                        .any(|a| a == ["--thinking", "disabled"])
                );
            }
            assert!(!spec.args.iter().any(|a| a == "--acp"));
        }
    }
}
