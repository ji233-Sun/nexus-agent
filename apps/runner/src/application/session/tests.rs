use super::*;
use nexus_domain::{
    HarnessKind, ThinkingEffort, UserAskAnswer, UserAskAnswerMode, UserAskAnswerValue,
    UserAskQuestion,
};
use nexus_harness_core::UserAskRequest;
use nexus_harness_core::{DecodedEvent, InputFrame};
use nexus_protocol::EventEnvelope;
use serde_json::Value;
use serde_json::json;
use std::collections::HashSet;
use std::{path::Path, process::Command as StdCommand};

struct FakeUserAskDecoder {
    omp: nexus_harness_omp::EventDecoder,
    requests: HashSet<String>,
    supports_answers: bool,
}

impl Default for FakeUserAskDecoder {
    fn default() -> Self {
        Self {
            omp: nexus_harness_omp::EventDecoder::default(),
            requests: HashSet::new(),
            supports_answers: true,
        }
    }
}

impl LineDecoder for FakeUserAskDecoder {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error> {
        let frame: Value = serde_json::from_str(line)?;
        match frame.get("type").and_then(Value::as_str) {
            Some("nexus_test.user_ask.requested") => {
                let native_request_id = frame
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let questions = serde_json::from_value(frame["questions"].clone())?;
                self.requests.insert(native_request_id.clone());
                Ok(vec![DecodedEvent::UserAskRequested(UserAskRequest {
                    native_request_id,
                    questions,
                    timeout_ms: None,
                    resolve_on_send: false,
                })])
            }
            Some("nexus_test.user_ask.finished") => {
                let native_request_id = frame
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                self.requests.remove(&native_request_id);
                Ok(vec![DecodedEvent::UserAskFinished {
                    native_request_id,
                    status: serde_json::from_value(frame["status"].clone())?,
                    message: frame
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                }])
            }
            _ => self.omp.decode_line(line),
        }
    }

    fn steer(&mut self, message_id: &str, prompt: &str) -> Option<InputFrame> {
        self.omp.steer(message_id, prompt)
    }

    fn answer_user_ask(
        &mut self,
        native_request_id: &str,
        answers: &[UserAskAnswer],
    ) -> Option<InputFrame> {
        (self.supports_answers && self.requests.contains(native_request_id)).then(|| {
            InputFrame(json!({
                "type": "nexus_test.user_ask.answer",
                "id": native_request_id,
                "answers": answers,
            }))
        })
    }
}

fn compile_fake_harness(directory: &Path) -> std::path::PathBuf {
    let executable = directory.join(format!("fake-harness{}", std::env::consts::EXE_SUFFIX));
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_harness.rs");
    let output = StdCommand::new("rustc")
        .args(["--edition=2024", "-D", "warnings"])
        .arg(fixture)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    executable
}

fn prepared_user_ask_run(
    directory: &Path,
    executable: &Path,
    prompt: &str,
) -> (StartRun, LaunchSpec) {
    let executable = executable.to_string_lossy().into_owned();
    let request = StartRun {
        attachments: Vec::new(),
        transport: nexus_domain::HarnessTransport::Acp,
        title_generation: None,
        permission_mode: nexus_domain::PermissionMode::AutoEdit,
        run_id: Uuid::new_v4(),
        task_id: Uuid::new_v4(),
        session_id: None,
        cwd: directory.to_string_lossy().into_owned(),
        prompt: prompt.into(),
        harness: HarnessKind::Omp,
        executable: executable.clone(),
        model: None,
        effort: ThinkingEffort::Medium,
        environment: Vec::new(),
    };
    let spec = nexus_harness_omp::build_launch_spec(
        &executable,
        directory,
        prompt,
        None,
        ThinkingEffort::Medium,
        None,
        nexus_domain::PermissionMode::AutoEdit,
    );
    (request, spec)
}

fn prepared_native_user_ask_run(
    directory: &Path,
    executable: &Path,
    harness: HarnessKind,
) -> (StartRun, LaunchSpec, Box<dyn LineDecoder>) {
    let request = StartRun {
        attachments: Vec::new(),
        transport: nexus_domain::HarnessTransport::Cli,
        title_generation: None,
        permission_mode: nexus_domain::PermissionMode::AutoEdit,
        run_id: Uuid::new_v4(),
        task_id: Uuid::new_v4(),
        session_id: None,
        cwd: directory.to_string_lossy().into_owned(),
        prompt: "user-ask-native".into(),
        harness,
        executable: executable.to_string_lossy().into_owned(),
        model: None,
        effort: ThinkingEffort::Medium,
        environment: Vec::new(),
    };
    let (spec, decoder) = crate::infrastructure::harness::prepare(&request, directory).unwrap();
    (request, spec, decoder)
}

async fn receive_user_ask(
    events: &mut mpsc::Receiver<EventEnvelope>,
) -> (Uuid, Vec<UserAskQuestion>, Vec<Event>) {
    let mut received = Vec::new();
    loop {
        let envelope = timeout(Duration::from_secs(10), events.recv())
            .await
            .expect("runner event timeout")
            .expect("runner event channel closed");
        if let Event::RunUserAskRequested {
            request_id,
            questions,
            ..
        } = &envelope.event
        {
            let result = (*request_id, questions.clone(), received);
            return result;
        }
        received.push(envelope.event);
    }
}

async fn collect_remaining_events(mut events: mpsc::Receiver<EventEnvelope>) -> Vec<Event> {
    let mut received = Vec::new();
    while let Some(envelope) = events.recv().await {
        received.push(envelope.event);
    }
    received
}

fn user_ask_answers() -> Vec<UserAskAnswer> {
    vec![
        UserAskAnswer {
            question_id: "note".into(),
            value: UserAskAnswerValue::Text("Keep compatibility".into()),
        },
        UserAskAnswer {
            question_id: "checks".into(),
            value: UserAskAnswerValue::Selected(vec!["tests".into(), "build".into()]),
        },
        UserAskAnswer {
            question_id: "target".into(),
            value: UserAskAnswerValue::Selected(vec!["workspace".into()]),
        },
    ]
}

#[tokio::test]
async fn omp_user_ask_round_trip_cancel_and_timeout() {
    let binaries = tempfile::tempdir().unwrap();
    let executable = compile_fake_harness(binaries.path());
    for (prompt, expected) in [
        ("omp-text-input", UserAskStatus::Answered),
        ("omp-text-editor", UserAskStatus::Answered),
        ("omp-text-select", UserAskStatus::Answered),
        ("omp-text-custom", UserAskStatus::Answered),
        ("omp-text-custom-only", UserAskStatus::Answered),
        ("omp-text-confirm", UserAskStatus::Answered),
        ("omp-text-cancel", UserAskStatus::Cancelled),
        ("omp-text-timeout", UserAskStatus::Expired),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let (request, spec) = prepared_user_ask_run(directory.path(), &executable, prompt);
        let run_id = request.run_id;
        let (cancel_tx, cancel) = watch::channel(false);
        let (input, input_rx) = mpsc::unbounded_channel();
        let user_asks = PendingUserAsks::default();
        let (emitter, mut events) = Emitter::channel();
        let task = tokio::spawn(run_prepared_harness(
            request,
            spec,
            Box::new(nexus_harness_omp::EventDecoder::default()),
            cancel,
            input_rx,
            user_asks.clone(),
            emitter,
        ));
        let (request_id, questions, _) = receive_user_ask(&mut events).await;
        assert_eq!(questions[0].prompt, "Branch name");
        let answers = vec![UserAskAnswer {
            question_id: "ui_1".into(),
            value: if questions[0].options.is_empty() || prompt == "omp-text-custom" {
                UserAskAnswerValue::Text("feature/修复\nsecond line".into())
            } else if prompt == "omp-text-select" {
                UserAskAnswerValue::Selected(vec![questions[0].options[0].id.clone()])
            } else {
                UserAskAnswerValue::Selected(vec![questions[0].options[1].id.clone()])
            },
        }];
        if expected == UserAskStatus::Answered {
            input
                .send(RunInput::UserAsk(
                    user_asks.claim_answer(request_id, answers.clone()).unwrap(),
                ))
                .unwrap();
        }
        let mut sent = false;
        loop {
            let event = timeout(Duration::from_secs(5), events.recv())
                .await
                .unwrap()
                .unwrap()
                .event;
            match event {
                Event::RunUserAskAnswerSent { request_id: id, .. } => {
                    assert_eq!(id, request_id);
                    sent = true;
                }
                Event::RunUserAskFinished {
                    run_id: id,
                    request_id: ask_id,
                    status,
                    ..
                } => {
                    assert_eq!((id, ask_id, status), (run_id, request_id, expected));
                    break;
                }
                Event::RunExited { .. } => panic!("dialog must finish before the run"),
                Event::RunUserAskRequested { .. } => panic!("custom input must not ask twice"),
                _ => {}
            }
        }
        assert_eq!(sent, expected == UserAskStatus::Answered);
        assert!(user_asks.claim_answer(request_id, answers).is_err());
        if expected != UserAskStatus::Answered {
            cancel_tx.send(true).unwrap();
        }
        assert_eq!(
            timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
                .0,
            if expected == UserAskStatus::Answered {
                RunStatus::Completed
            } else {
                RunStatus::Cancelled
            }
        );
        let remaining = collect_remaining_events(events).await;
        assert!(!remaining.iter().any(|event| matches!(
            event,
            Event::RunUserAskFinished { .. } | Event::RunUserAskRequested { .. }
        )));
        if expected == UserAskStatus::Answered {
            let frame: Value = serde_json::from_str(
                &std::fs::read_to_string(directory.path().join("user-ask-input.json")).unwrap(),
            )
            .unwrap();
            let expected = match prompt {
                "omp-text-confirm" => {
                    json!({"type": "extension_ui_response", "id": "ui_1", "confirmed": false})
                }
                "omp-text-select" => {
                    json!({"type": "extension_ui_response", "id": "ui_1", "value": "Tests"})
                }
                "omp-text-custom" | "omp-text-custom-only" => {
                    json!({"type": "extension_ui_response", "id": "ui_1", "value": "Other (type your own)"})
                }
                _ => {
                    json!({"type": "extension_ui_response", "id": "ui_1", "value": "feature/修复\nsecond line"})
                }
            };
            assert_eq!(frame, expected);
            if matches!(prompt, "omp-text-custom" | "omp-text-custom-only") {
                let frame: Value = serde_json::from_str(
                    &std::fs::read_to_string(directory.path().join("user-ask-custom-input.json"))
                        .unwrap(),
                )
                .unwrap();
                assert_eq!(
                    frame,
                    json!({
                        "type": "extension_ui_response", "id": "ui_2", "value": "feature/修复\nsecond line"
                    })
                );
            }
        } else {
            assert!(!directory.path().join("user-ask-input.json").exists());
        }
    }
}

#[tokio::test]
async fn native_stream_user_ask_round_trip() {
    let binaries = tempfile::tempdir().unwrap();
    let executable = compile_fake_harness(binaries.path());
    for harness in [
        HarnessKind::Claude,
        HarnessKind::Codex,
        HarnessKind::Qoder,
        HarnessKind::QoderCn,
        HarnessKind::Codebuddy,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let (request, spec, decoder) =
            prepared_native_user_ask_run(directory.path(), &executable, harness);
        let run_id = request.run_id;
        let (_cancel, cancel) = watch::channel(false);
        let (input, input_rx) = mpsc::unbounded_channel();
        let user_asks = PendingUserAsks::default();
        let (emitter, mut events) = Emitter::channel();
        let task = tokio::spawn(run_prepared_harness(
            request,
            spec,
            decoder,
            cancel,
            input_rx,
            user_asks.clone(),
            emitter,
        ));

        let (request_id, questions, mut received) = receive_user_ask(&mut events).await;
        assert_eq!(questions.len(), 2);
        assert_eq!(questions[0].prompt, "Which checks?");
        assert_eq!(questions[1].prompt, "Branch name?");
        let answers = vec![
            UserAskAnswer {
                question_id: questions[0].id.clone(),
                value: UserAskAnswerValue::Selected(if harness != HarnessKind::Codex {
                    vec!["Tests".into(), "Clippy".into()]
                } else {
                    vec!["Tests".into()]
                }),
            },
            UserAskAnswer {
                question_id: questions[1].id.clone(),
                value: UserAskAnswerValue::Text("feature/native-ask".into()),
            },
        ];
        let answer = user_asks.claim_answer(request_id, answers.clone()).unwrap();
        assert!(user_asks.claim_answer(request_id, answers.clone()).is_err());
        input.send(RunInput::UserAsk(answer)).unwrap();

        assert_eq!(
            timeout(Duration::from_secs(10), task)
                .await
                .unwrap()
                .unwrap(),
            (RunStatus::Completed, Some(0))
        );
        received.extend(collect_remaining_events(events).await);
        assert_eq!(
            received
                .iter()
                .filter(|event| matches!(event,
                Event::RunUserAskAnswerSent { run_id: id, request_id: ask }
                    if *id == run_id && *ask == request_id))
                .count(),
            1
        );
        assert_eq!(
            received
                .iter()
                .filter(|event| matches!(event,
                Event::RunUserAskFinished { run_id: id, request_id: ask,
                    status: UserAskStatus::Answered, .. }
                    if *id == run_id && *ask == request_id))
                .count(),
            1
        );
        assert!(
            !received
                .iter()
                .any(|event| matches!(event, Event::RunApprovalRequested { .. }))
        );
        assert!(user_asks.claim_answer(request_id, answers).is_err());

        let frame: Value = serde_json::from_str(
            &std::fs::read_to_string(directory.path().join("user-ask-input.json")).unwrap(),
        )
        .unwrap();
        match harness {
            HarnessKind::Claude
            | HarnessKind::Qoder
            | HarnessKind::QoderCn
            | HarnessKind::Codebuddy => assert_eq!(
                frame,
                json!({
                    "type": "control_response",
                    "response": {"subtype": "success", "request_id": "ask-claude", "response": {
                        "behavior": "allow", "updatedInput": {
                            "questions": [
                                {"question": "Which checks?", "multiSelect": true,
                                 "options": [{"label": "Tests"}, {"label": "Clippy"}]},
                                {"question": "Branch name?"}
                            ],
                            "answers": {"Which checks?": "Tests, Clippy", "Branch name?": "feature/native-ask"}
                        }
                    }}
                })
            ),
            HarnessKind::Codex => assert_eq!(
                frame,
                json!({
                    "id": 77,
                    "result": {"answers": {
                        "checks": {"answers": ["Tests"]},
                        "branch": {"answers": ["feature/native-ask"]}
                    }}
                })
            ),
            HarnessKind::Omp
            | HarnessKind::Pi
            | HarnessKind::Kimi
            | HarnessKind::Opencode
            | HarnessKind::Deepseek
            | HarnessKind::CommandCode => {
                unreachable!()
            }
        }
    }
}

#[tokio::test]
async fn codex_async_user_ask_keeps_session_alive_and_requires_native_receipt() {
    let binaries = tempfile::tempdir().unwrap();
    let executable = compile_fake_harness(binaries.path());
    for scenario in ["live", "completed", "rejected", "cancelled"] {
        let directory = tempfile::tempdir().unwrap();
        let (mut request, _, _) =
            prepared_native_user_ask_run(directory.path(), &executable, HarnessKind::Codex);
        request.prompt = format!("codex-async-{scenario}");
        let (spec, decoder) =
            crate::infrastructure::harness::prepare(&request, directory.path()).unwrap();
        let (cancel_tx, cancel) = watch::channel(false);
        let (input, input_rx) = mpsc::unbounded_channel();
        let user_asks = PendingUserAsks::default();
        let (emitter, mut events) = Emitter::channel();
        let task = tokio::spawn(run_prepared_harness(
            request,
            spec,
            decoder,
            cancel,
            input_rx,
            user_asks.clone(),
            emitter,
        ));
        let (request_id, questions, mut received) = receive_user_ask(&mut events).await;
        if scenario != "live" {
            loop {
                let event = timeout(Duration::from_secs(10), events.recv())
                    .await
                    .unwrap()
                    .unwrap()
                    .event;
                let waiting = matches!(&event, Event::RunStatusChanged { message: Some(message), .. } if message.contains("等待 User Ask"));
                received.push(event);
                if waiting {
                    break;
                }
            }
            assert!(
                !task.is_finished(),
                "async questions must survive model turn completion"
            );
        }
        let answers = vec![
            UserAskAnswer {
                question_id: questions[0].id.clone(),
                value: UserAskAnswerValue::Selected(vec![questions[0].options[1].id.clone()]),
            },
            UserAskAnswer {
                question_id: questions[1].id.clone(),
                value: UserAskAnswerValue::Text("每天半小时".into()),
            },
        ];
        if scenario == "cancelled" {
            cancel_tx.send(true).unwrap();
        } else {
            input
                .send(RunInput::UserAsk(
                    user_asks.claim_answer(request_id, answers.clone()).unwrap(),
                ))
                .unwrap();
        }
        let (status, _) = timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            status,
            match scenario {
                "cancelled" => RunStatus::Cancelled,
                "rejected" => RunStatus::Failed,
                _ => RunStatus::Completed,
            }
        );
        received.extend(collect_remaining_events(events).await);
        let finishes = received
            .iter()
            .filter_map(|event| match event {
                Event::RunUserAskFinished {
                    request_id: id,
                    status,
                    ..
                } if *id == request_id => Some(*status),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            finishes,
            vec![match scenario {
                "cancelled" => UserAskStatus::Cancelled,
                "rejected" => UserAskStatus::Failed,
                _ => UserAskStatus::Answered,
            }]
        );
        assert!(user_asks.claim_answer(request_id, answers).is_err());
        if scenario != "cancelled" {
            let frame: Value = serde_json::from_str(
                &std::fs::read_to_string(directory.path().join("user-ask-input.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(frame["method"], "turn/start");
            assert_eq!(
                frame["params"]["input"][0]["text"],
                "User Ask answers:\n\nChoose a skill?\n乐器\n\nAny details?\n每天半小时"
            );
            if scenario != "rejected" {
                assert!(received.iter().any(|event| matches!(event, Event::RunMessageCompleted { text, .. } if text == "answered")));
            }
        }
    }
}

#[tokio::test]
async fn fake_user_ask_round_trip_keeps_the_run_and_answer_mapping() {
    let binaries = tempfile::tempdir().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let executable = compile_fake_harness(binaries.path());
    let (request, spec) = prepared_user_ask_run(directory.path(), &executable, "user-ask");
    let run_id = request.run_id;
    let (_cancel, cancel) = watch::channel(false);
    let (input, input_rx) = mpsc::unbounded_channel();
    let user_asks = PendingUserAsks::default();
    let (emitter, mut events) = Emitter::channel();
    let task = tokio::spawn(run_prepared_harness(
        request,
        spec,
        Box::new(FakeUserAskDecoder::default()),
        cancel,
        input_rx,
        user_asks.clone(),
        emitter,
    ));

    let (request_id, questions, mut received) = receive_user_ask(&mut events).await;
    assert_eq!(
        questions
            .iter()
            .map(|question| question.id.as_str())
            .collect::<Vec<_>>(),
        ["target", "checks", "note"]
    );
    assert!(matches!(
        questions[0].answer_mode,
        UserAskAnswerMode::Choice {
            multiple: false,
            allow_custom: false
        }
    ));
    assert!(matches!(
        questions[1].answer_mode,
        UserAskAnswerMode::Choice { multiple: true, .. }
    ));
    assert_eq!(
        questions[0].options[0].description.as_deref(),
        Some("Core packages")
    );

    let answer = user_asks
        .claim_answer(request_id, user_ask_answers())
        .unwrap();
    assert!(
        user_asks
            .claim_answer(request_id, user_ask_answers())
            .is_err()
    );
    input.send(RunInput::UserAsk(answer)).unwrap();

    let result = timeout(Duration::from_secs(10), task)
        .await
        .expect("run timeout")
        .unwrap();
    received.extend(collect_remaining_events(events).await);
    assert_eq!(result, (RunStatus::Completed, Some(0)));
    assert!(received.iter().any(|event| matches!(event,
        Event::RunStarted { run_id: id, .. } if *id == run_id)));
    assert!(received.iter().any(|event| matches!(event,
        Event::RunUserAskAnswerSent { run_id: id, request_id: request }
            if *id == run_id && *request == request_id)));
    assert!(received.iter().any(|event| matches!(event,
        Event::RunUserAskFinished {
            run_id: id,
            request_id: request,
            status: UserAskStatus::Answered,
            ..
        } if *id == run_id && *request == request_id)));
    assert!(received.iter().any(|event| matches!(event,
        Event::RunMessageCompleted { run_id: id, text }
            if *id == run_id && text == "answered")));

    let frame: Value = serde_json::from_str(
        &std::fs::read_to_string(directory.path().join("user-ask-input.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(frame["type"], "nexus_test.user_ask.answer");
    assert_eq!(frame["id"], "native-ask-42");
    assert_eq!(frame["answers"][0]["question_id"], "target");
    assert_eq!(frame["answers"][0]["value"]["value"][0], "workspace");
    assert_eq!(frame["answers"][1]["question_id"], "checks");
    assert_eq!(frame["answers"][1]["value"]["value"][0], "tests");
    assert_eq!(frame["answers"][1]["value"]["value"][1], "build");
    assert_eq!(frame["answers"][2]["question_id"], "note");
    assert_eq!(frame["answers"][2]["value"]["value"], "Keep compatibility");
}

#[tokio::test]
async fn unsupported_user_ask_encoding_fails_without_claiming_delivery() {
    let binaries = tempfile::tempdir().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let executable = compile_fake_harness(binaries.path());
    let (request, spec) = prepared_user_ask_run(directory.path(), &executable, "user-ask");
    let (_cancel, cancel) = watch::channel(false);
    let (input, input_rx) = mpsc::unbounded_channel();
    let user_asks = PendingUserAsks::default();
    let (emitter, mut events) = Emitter::channel();
    let task = tokio::spawn(run_prepared_harness(
        request,
        spec,
        Box::new(FakeUserAskDecoder {
            supports_answers: false,
            ..FakeUserAskDecoder::default()
        }),
        cancel,
        input_rx,
        user_asks.clone(),
        emitter,
    ));

    let (request_id, _, mut received) = receive_user_ask(&mut events).await;
    let answer = user_asks
        .claim_answer(request_id, user_ask_answers())
        .unwrap();
    input.send(RunInput::UserAsk(answer)).unwrap();

    let result = timeout(Duration::from_secs(10), task)
        .await
        .expect("run timeout")
        .unwrap();
    received.extend(collect_remaining_events(events).await);
    assert_eq!(result.0, RunStatus::Failed);
    assert!(received.iter().any(|event| matches!(event,
        Event::RunUserAskFinished {
            request_id: id,
            status: UserAskStatus::Failed,
            ..
        } if *id == request_id)));
    assert!(!received.iter().any(|event| matches!(event,
        Event::RunUserAskAnswerSent { request_id: id, .. } if *id == request_id)));
}

#[tokio::test]
async fn user_ask_cleanup_covers_cancellation_and_harness_exit() {
    let binaries = tempfile::tempdir().unwrap();
    let executable = compile_fake_harness(binaries.path());
    for (prompt, expected_ask, expected_run) in [
        (
            "user-ask-cancel",
            UserAskStatus::Cancelled,
            RunStatus::Cancelled,
        ),
        (
            "user-ask-exit",
            UserAskStatus::Expired,
            RunStatus::Completed,
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let (request, spec) = prepared_user_ask_run(directory.path(), &executable, prompt);
        let (cancel, cancel_rx) = watch::channel(false);
        let (_input, input_rx) = mpsc::unbounded_channel();
        let user_asks = PendingUserAsks::default();
        let (emitter, mut events) = Emitter::channel();
        let task = tokio::spawn(run_prepared_harness(
            request,
            spec,
            Box::new(FakeUserAskDecoder::default()),
            cancel_rx,
            input_rx,
            user_asks.clone(),
            emitter,
        ));

        let (request_id, _, mut received) = receive_user_ask(&mut events).await;
        if expected_run == RunStatus::Cancelled {
            cancel.send_replace(true);
        }
        let result = timeout(Duration::from_secs(10), task)
            .await
            .expect("run timeout")
            .unwrap();
        received.extend(collect_remaining_events(events).await);
        assert_eq!(result.0, expected_run);
        assert!(received.iter().any(|event| matches!(event,
            Event::RunUserAskFinished { request_id: id, status, .. }
                if *id == request_id && *status == expected_ask)));
        assert!(
            user_asks
                .claim_answer(request_id, user_ask_answers())
                .is_err()
        );
    }
}

#[tokio::test]
async fn queued_user_ask_answer_loses_cleanly_to_cancellation() {
    let binaries = tempfile::tempdir().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let executable = compile_fake_harness(binaries.path());
    let (request, spec) = prepared_user_ask_run(directory.path(), &executable, "user-ask");
    let run_id = request.run_id;
    let (cancel, cancel_rx) = watch::channel(false);
    let (input, input_rx) = mpsc::unbounded_channel();
    let user_asks = PendingUserAsks::default();
    let (emitter, mut events) = Emitter::channel();
    let cancellation_emitter = emitter.clone();
    let task = tokio::spawn(run_prepared_harness(
        request,
        spec,
        Box::new(FakeUserAskDecoder::default()),
        cancel_rx,
        input_rx,
        user_asks.clone(),
        emitter,
    ));

    let (request_id, _, mut received) = receive_user_ask(&mut events).await;
    let answer = user_asks
        .claim_answer(request_id, user_ask_answers())
        .unwrap();
    input.send(RunInput::UserAsk(answer)).unwrap();
    finish_user_asks(
        run_id,
        &user_asks,
        UserAskStatus::Cancelled,
        None,
        &cancellation_emitter,
    )
    .await;
    cancel.send_replace(true);
    drop(cancellation_emitter);

    let result = timeout(Duration::from_secs(10), task)
        .await
        .expect("run timeout")
        .unwrap();
    received.extend(collect_remaining_events(events).await);
    assert_eq!(result.0, RunStatus::Cancelled);
    assert_eq!(
        received
            .iter()
            .filter(|event| matches!(event,
                Event::RunUserAskFinished { request_id: id, .. } if *id == request_id))
            .count(),
        1
    );
    assert!(received.iter().any(|event| matches!(event,
        Event::RunUserAskFinished {
            request_id: id,
            status: UserAskStatus::Cancelled,
            ..
        } if *id == request_id)));
}
