use super::*;
use nexus_domain::ThinkingEffort;

fn request() -> StartRun {
    StartRun {
        attachments: Vec::new(),
        transport: nexus_domain::HarnessTransport::Acp,
        title_generation: None,
        permission_mode: nexus_domain::PermissionMode::AutoEdit,
        run_id: Default::default(),
        task_id: Default::default(),
        session_id: None,
        cwd: "/tmp/project".into(),
        prompt: "secret prompt".into(),
        harness: HarnessKind::Codex,
        executable: "codex".into(),
        model: None,
        effort: ThinkingEffort::Default,
        environment: vec![EnvironmentVariable {
            name: "CODEX_API_KEY".into(),
            value: String::new(),
        }],
    }
}

fn input(events: &[DecodedEvent]) -> &Value {
    match events.last().unwrap() {
        DecodedEvent::WriteStdin(frame) => &frame.0,
        event => panic!("expected stdin frame, got {event:?}"),
    }
}

fn decoder() -> EventDecoder {
    let (_, mut decoder) = prepare_run(&request(), Path::new("/tmp/project"));
    decoder
        .decode_line(r#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#)
        .unwrap();
    decoder
        .decode_line(r#"{"id":3,"result":{"turn":{"id":"turn-1"}}}"#)
        .unwrap();
    decoder
}

#[test]
fn launch_spec_preserves_permissions_model_effort_and_session_resume() {
    for (mode, sandbox, policy) in [
        (PermissionMode::Ask, "read-only", "on-request"),
        (PermissionMode::AutoEdit, "workspace-write", "on-request"),
        (PermissionMode::Yolo, "danger-full-access", "never"),
    ] {
        for session_id in [None, Some("existing-session".into())] {
            let mut request = request();
            request.permission_mode = mode;
            request.session_id = session_id;
            let (_, mut decoder) = prepare_run(&request, Path::new(&request.cwd));
            let events = decoder.decode_line(r#"{"id":0,"result":{}}"#).unwrap();
            let frame = input(&events);
            assert_eq!(frame["params"]["sandbox"], sandbox);
            assert_eq!(frame["params"]["approvalPolicy"], policy);
        }
    }
    for session_id in [None, Some("existing-thread")] {
        let mut request = request();
        request.attachments = vec![nexus_domain::Attachment {
            path: "/tmp/captured page.png".into(),
            source_name: "报告.pdf".into(),
            page: Some(12),
            kind: nexus_domain::AttachmentKind::Image,
        }];
        request.session_id = session_id.map(str::to_owned);
        request.model = Some("gpt-test".into());
        request.effort = ThinkingEffort::XHigh;
        let (spec, mut decoder) = prepare_run(&request, Path::new(&request.cwd));
        assert_eq!(spec.args, ["app-server"]);
        assert!(!spec.args.iter().any(|arg| arg.contains(&request.prompt)));
        assert_eq!(
            serde_json::from_str::<Value>(&spec.stdin).unwrap()["method"],
            "initialize"
        );
        let events = decoder.decode_line(r#"{"id":0,"result":{}}"#).unwrap();
        let frame = input(&events);
        assert_eq!(
            frame["method"],
            if session_id.is_some() {
                "thread/resume"
            } else {
                "thread/start"
            }
        );
        assert_eq!(frame["params"]["cwd"], request.cwd);
        assert_eq!(frame["params"]["approvalPolicy"], "on-request");
        assert_eq!(frame["params"]["sandbox"], "workspace-write");
        assert_eq!(frame["params"]["model"], "gpt-test");
        if let Some(id) = session_id {
            assert_eq!(frame["params"]["threadId"], id);
        }
        let events = decoder
            .decode_line(r#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#)
            .unwrap();
        assert_eq!(events[0], DecodedEvent::SessionStarted("thread-1".into()));
        let frame = input(&events);
        assert_eq!(frame["method"], "turn/start");
        assert_eq!(frame["params"]["input"][0]["text"], request.prompt);
        assert_eq!(frame["params"]["input"][1]["text"], "报告.pdf · page 12");
        assert_eq!(
            frame["params"]["input"][2],
            json!({"type":"localImage", "path":"/tmp/captured page.png"})
        );
        assert_eq!(frame["params"]["effort"], "xhigh");
    }
}

#[test]
fn approvals_preserve_rpc_ids_scope_and_file_change_details() {
    let mut decoder = decoder();
    decoder.decode_line(r#"{"method":"item/started","params":{"item":{"id":"patch","type":"fileChange","changes":[{"path":"src/lib.rs","diff":"+new content"}]}}}"#).unwrap();
    for id in [json!(17), json!("approval-17")] {
        for method in [
            "item/commandExecution/requestApproval",
            "item/fileChange/requestApproval",
            "item/permissions/requestApproval",
        ] {
            let frame = json!({"id": id, "method": method, "params": {"threadId":"thread-1", "turnId":"turn-1", "itemId":"patch", "command":"echo test", "cwd":"/tmp/project", "reason":"needs access", "permissions":{"network":{"enabled":true}}}});
            let events = decoder.decode_line(&frame.to_string()).unwrap();
            let [DecodedEvent::ApprovalRequested(prompt)] = events.as_slice() else {
                panic!("expected approval")
            };
            assert_eq!(prompt.options[0].response.0["id"], id);
            assert!(prompt.details.contains("src/lib.rs"));
            assert!(prompt.details.contains("needs access"));
            if method.contains("permissions") {
                assert_eq!(
                    prompt.options[0].response.0["result"]["permissions"],
                    frame["params"]["permissions"]
                );
                assert_eq!(prompt.options[0].response.0["result"]["scope"], "turn");
                assert_eq!(prompt.cancel.0["result"]["permissions"], json!({}));
            } else {
                assert_eq!(prompt.options[0].response.0["result"]["decision"], "accept");
                assert_eq!(
                    prompt.options[1].response.0["result"]["decision"],
                    "decline"
                );
            }
            let resolved = json!({"method":"serverRequest/resolved","params":{"threadId":"thread-1","requestId":id}});
            assert_eq!(
                decoder.decode_line(&resolved.to_string()).unwrap(),
                vec![DecodedEvent::ApprovalResolved(prompt.id.clone())]
            );
        }
    }
    let events = decoder.decode_line(r#"{"id":18,"method":"item/commandExecution/requestApproval","params":{"threadId":"other","turnId":"turn-1"}}"#).unwrap();
    assert!(input(&events).get("error").is_some());
    let events = decoder.decode_line(r#"{"id":19,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-1","turnId":"turn-1","availableDecisions":["decline","cancel"]}}"#).unwrap();
    let [DecodedEvent::ApprovalRequested(prompt)] = events.as_slice() else {
        panic!("expected approval")
    };
    assert_eq!(prompt.options.len(), 1);
    assert_eq!(prompt.options[0].label, "Deny");
}

#[test]
fn user_input_maps_questions_and_preserves_numeric_and_string_rpc_ids() {
    let mut decoder = decoder();
    let questions = json!([
        {
            "id": "path-kind",
            "question": "Choose exactly one:\n路径",
            "isOther": true,
            "options": [
                {"label": "Fast", "description": "Quick \"path\""},
                {"label": "Safe", "description": "保守"}
            ]
        },
        {
            "id": "details",
            "question": "Exact details?",
            "options": null
        }
    ]);

    for id in [json!(27), json!("27")] {
        let frame = json!({
            "id": id,
            "method": "item/tool/requestUserInput",
            "params": {"threadId": "thread-1", "turnId": "turn-1", "questions": questions}
        });
        let events = decoder.decode_line(&frame.to_string()).unwrap();
        let [DecodedEvent::UserAskRequested(request)] = events.as_slice() else {
            panic!("expected user ask")
        };
        assert_eq!(request.native_request_id, id.to_string());
        assert_eq!(request.timeout_ms, None);
        assert!(request.resolve_on_send);
        assert_eq!(request.questions[0].id, "path-kind");
        assert_eq!(request.questions[0].prompt, "Choose exactly one:\n路径");
        assert_eq!(request.questions[0].options[0].label, "Fast");
        assert_eq!(
            request.questions[0].options[0].description.as_deref(),
            Some("Quick \"path\"")
        );
        assert_eq!(
            request.questions[0].answer_mode,
            UserAskAnswerMode::Choice {
                multiple: false,
                allow_custom: true
            }
        );
        assert_eq!(request.questions[1].answer_mode, UserAskAnswerMode::Text);

        let response = decoder
            .answer_user_ask(
                &id.to_string(),
                &[
                    UserAskAnswer {
                        question_id: "path-kind".into(),
                        value: UserAskAnswerValue::Selected(vec!["Safe".into()]),
                    },
                    UserAskAnswer {
                        question_id: "details".into(),
                        value: UserAskAnswerValue::Text("line 1\n\"原样\"".into()),
                    },
                ],
            )
            .unwrap();
        assert_eq!(
            response.0,
            json!({"id": id, "result": {"answers": {
                "path-kind": {"answers": ["Safe"]},
                "details": {"answers": ["line 1\n\"原样\""]}
            }}})
        );
        assert!(decoder.answer_user_ask(&id.to_string(), &[]).is_none());
    }
}

#[test]
fn user_input_rejects_wrong_scope_and_resolved_requests_cannot_be_answered() {
    let mut decoder = decoder();
    let out_of_scope = json!({
        "id": "ask-out",
        "method": "item/tool/requestUserInput",
        "params": {"threadId": "other", "turnId": "turn-1", "questions": [
            {"id": "q", "question": "Question?", "options": []}
        ]}
    });
    let events = decoder.decode_line(&out_of_scope.to_string()).unwrap();
    assert_eq!(input(&events)["id"], "ask-out");
    assert!(input(&events).get("error").is_some());
    assert!(decoder.answer_user_ask("\"ask-out\"", &[]).is_none());

    let ask = json!({
        "id": "ask-live",
        "method": "item/tool/requestUserInput",
        "params": {"threadId": "thread-1", "turnId": "turn-1", "questions": [
            {"id": "q", "question": "Question?"}
        ]}
    });
    decoder.decode_line(&ask.to_string()).unwrap();
    let resolved = json!({
        "method": "serverRequest/resolved",
        "params": {"threadId": "thread-1", "turnId": "turn-1", "requestId": "ask-live"}
    });
    assert_eq!(
        decoder.decode_line(&resolved.to_string()).unwrap(),
        vec![DecodedEvent::UserAskFinished {
            native_request_id: "\"ask-live\"".into(),
            status: UserAskStatus::Cancelled,
            message: None,
        }]
    );
    assert!(
        decoder
            .answer_user_ask(
                "\"ask-live\"",
                &[UserAskAnswer {
                    question_id: "q".into(),
                    value: UserAskAnswerValue::Text("late".into()),
                }],
            )
            .is_none()
    );
}

fn async_question(id: &str) -> Value {
    json!({"method": "item/completed", "params": {
        "threadId": "thread-1", "turnId": "turn-1", "item": {
            "id": id, "type": "agentMessage", "delivery": "async",
            "phase": "final_answer", "text": "Choose a skill?\n- 外语\n- 乐器",
            "questions": [
                {"title": "Choose a skill?", "options": ["外语", "乐器"]},
                {"title": "Any details?"}
            ]
        }
    }})
}

#[test]
fn async_user_input_survives_turn_completion_and_waits_for_answer_receipt() {
    for completed in [false, true] {
        let mut decoder = decoder();
        let frame = async_question("call-ask");
        let events = decoder.decode_line(&frame.to_string()).unwrap();
        let [DecodedEvent::UserAskRequested(request)] = events.as_slice() else {
            panic!("expected structured async User Ask, got {events:?}")
        };
        assert!(!request.resolve_on_send);
        assert_eq!(request.questions.len(), 2);
        assert_eq!(request.questions[0].prompt, "Choose a skill?");
        assert_eq!(
            request.questions[0].answer_mode,
            UserAskAnswerMode::Choice {
                multiple: false,
                allow_custom: true,
            }
        );
        assert_eq!(request.questions[1].answer_mode, UserAskAnswerMode::Text);
        assert!(decoder.decode_line(&frame.to_string()).unwrap().is_empty());
        if completed {
            let events = decoder.decode_line(r#"{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}}"#).unwrap();
            assert!(!events.contains(&DecodedEvent::TurnCompleted));
        }
        let answers = [
            UserAskAnswer {
                question_id: request.questions[0].id.clone(),
                value: if completed {
                    UserAskAnswerValue::Text("做菜".into())
                } else {
                    UserAskAnswerValue::Selected(vec![request.questions[0].options[1].id.clone()])
                },
            },
            UserAskAnswer {
                question_id: request.questions[1].id.clone(),
                value: UserAskAnswerValue::Text("每天\n半小时".into()),
            },
        ];
        let response = decoder
            .answer_user_ask(&request.native_request_id, &answers)
            .unwrap()
            .0;
        assert_eq!(response["method"], "turn/start");
        assert_eq!(response["params"]["threadId"], "thread-1");
        assert_eq!(
            response["params"]["input"][0]["text"],
            format!(
                "User Ask answers:\n\nChoose a skill?\n{}\n\nAny details?\n每天\n半小时",
                if completed { "做菜" } else { "乐器" }
            )
        );
        assert!(
            decoder
                .answer_user_ask(&request.native_request_id, &answers)
                .is_none()
        );
        let receipt = json!({"id": response["id"], "result": {"turn": {"id": "turn-2"}}});
        assert_eq!(
            decoder.decode_line(&receipt.to_string()).unwrap(),
            vec![DecodedEvent::UserAskFinished {
                native_request_id: request.native_request_id.clone(),
                status: UserAskStatus::Answered,
                message: None,
            }]
        );
        assert_eq!(
            decoder.steer("message", "continue").unwrap().0["params"]["expectedTurnId"],
            "turn-2"
        );
        assert!(decoder.decode_line(&frame.to_string()).unwrap().is_empty());
        assert_eq!(decoder.decode_line(r#"{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-2","status":"completed"}}}"#).unwrap(), vec![DecodedEvent::TurnCompleted]);
    }
}

#[test]
fn async_user_input_rejects_foreign_turns_and_reports_failed_delivery() {
    let mut decoder = decoder();
    let mut frame = async_question("call-ask");
    frame["params"]["turnId"] = "old-turn".into();
    assert!(decoder.decode_line(&frame.to_string()).unwrap().is_empty());
    frame["params"]["turnId"] = "turn-1".into();
    let events = decoder.decode_line(&frame.to_string()).unwrap();
    let [DecodedEvent::UserAskRequested(request)] = events.as_slice() else {
        panic!("expected User Ask")
    };
    let answers = request
        .questions
        .iter()
        .map(|question| UserAskAnswer {
            question_id: question.id.clone(),
            value: UserAskAnswerValue::Text("custom".into()),
        })
        .collect::<Vec<_>>();
    let response = decoder
        .answer_user_ask(&request.native_request_id, &answers)
        .unwrap()
        .0;
    let events = decoder.decode_line(&json!({"id": response["id"], "error": {"code": -32600, "message": "Cannot accept input"}}).to_string()).unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        DecodedEvent::UserAskFinished {
            status: UserAskStatus::Failed,
            ..
        }
    )));
    assert!(events.contains(&DecodedEvent::TurnCompleted));
    assert!(
        decoder
            .answer_user_ask(&request.native_request_id, &answers)
            .is_none()
    );
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
    let (spec, mut decoder) = prepare_run(&request(), Path::new("/tmp/project"));
    assert_eq!(spec.args, ["app-server"]);
    let events = decoder.decode_line(r#"{"id":0,"result":{}}"#).unwrap();
    assert!(input(&events)["params"].get("model").is_none());
    let events = decoder
        .decode_line(r#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#)
        .unwrap();
    assert!(input(&events)["params"].get("effort").is_none());
}

#[test]
fn api_key_login_is_ephemeral_and_redacted() {
    let mut request = request();
    request.environment[0].value = "test-secret".into();
    let (spec, mut decoder) = prepare_run(&request, Path::new(&request.cwd));
    assert!(
        spec.args
            .windows(2)
            .any(|pair| pair == ["--config", "cli_auth_credentials_store=\"ephemeral\""])
    );
    let events = decoder.decode_line(r#"{"id":0,"result":{}}"#).unwrap();
    assert_eq!(input(&events)["method"], "account/login/start");
    assert_eq!(input(&events)["params"]["apiKey"], "test-secret");
    assert!(!format!("{events:?}").contains("test-secret"));
    let events = decoder
        .decode_line(r#"{"id":1,"error":{"message":"test-secret"}}"#)
        .unwrap();
    assert!(!format!("{events:?}").contains("test-secret"));
    assert!(matches!(
        events.as_slice(),
        [DecodedEvent::Error(_), DecodedEvent::TurnCompleted]
    ));
}

#[test]
fn decoder_maps_messages_and_command_events() {
    let mut decoder = decoder();
    let started = r#"{"method":"item/started","params":{"item":{"id":"item_1","type":"commandExecution","command":"cargo test","status":"inProgress"}}}"#;
    assert!(matches!(decoder.decode_line(started).unwrap().as_slice(),
        [DecodedEvent::ToolStarted { id, name, summary }]
            if id == "item_1" && name == "Command" && summary == "cargo test"));
    let completed = r#"{"method":"item/completed","params":{"item":{"id":"item_1","type":"commandExecution","aggregatedOutput":"ok","exitCode":0,"status":"completed"}}}"#;
    assert_eq!(
        decoder.decode_line(completed).unwrap(),
        vec![DecodedEvent::ToolCompleted {
            id: "item_1".into(),
            output: "ok".into(),
            is_error: false,
        }]
    );
    let message = r#"{"method":"item/completed","params":{"item":{"id":"item_2","type":"agentMessage","text":"done"}}}"#;
    assert_eq!(
        decoder.decode_line(message).unwrap(),
        vec![DecodedEvent::MessageCompleted("done".into())]
    );

    let long_text = "完整命令和输出\n".repeat(100);
    let started = json!({"method": "item/started", "params": {"item": {
        "id": "long", "type": "commandExecution", "command": long_text
    }}});
    assert!(
        matches!(decoder.decode_line(&started.to_string()).unwrap().as_slice(),
        [DecodedEvent::ToolStarted { summary, .. }] if summary == &long_text)
    );
    let completed = json!({"method": "item/completed", "params": {"item": {
        "id": "long", "type": "commandExecution", "aggregatedOutput": long_text,
        "status": "completed", "exitCode": 1
    }}});
    assert!(
        matches!(decoder.decode_line(&completed.to_string()).unwrap().as_slice(),
        [DecodedEvent::ToolCompleted { output, is_error: true, .. }] if output == &long_text)
    );
    let item = json!({"id": "edit", "type": "fileChange", "status": "completed",
        "changes": [{"kind": "update", "path": "main.rs", "diff": long_text}]
    });
    let started = json!({"method": "item/started", "params": {"item": item}});
    assert!(
        matches!(decoder.decode_line(&started.to_string()).unwrap().as_slice(),
        [DecodedEvent::ToolStarted { summary, .. }]
            if serde_json::from_str::<Value>(summary).unwrap() == item)
    );
    let completed = json!({"method": "item/completed", "params": {"item": item}});
    assert!(matches!(
        decoder
            .decode_line(&completed.to_string())
            .unwrap()
            .as_slice(),
        [DecodedEvent::ToolCompleted {
            is_error: false,
            ..
        }]
    ));
}

#[test]
fn steer_targets_the_current_turn_and_matches_native_receipts() {
    let (_, mut unstarted) = prepare_run(&request(), Path::new("/tmp/project"));
    assert!(unstarted.steer("message-1", "update").is_none());
    assert!(
        unstarted
            .decode_line(r#"{"id":"message-1","result":{}}"#)
            .unwrap()
            .is_empty()
    );
    let mut decoder = decoder();
    let frame = decoder.steer("message-1", "update\n第二行").unwrap();
    assert_eq!(frame.0["method"], "turn/steer");
    assert_eq!(frame.0["params"]["threadId"], "thread-1");
    assert_eq!(frame.0["params"]["expectedTurnId"], "turn-1");
    assert_eq!(frame.0["params"]["clientUserMessageId"], "message-1");
    assert_eq!(frame.0["params"]["input"][0]["text"], "update\n第二行");
    assert_eq!(
        decoder
            .decode_line(r#"{"id":"message-1","result":{"turnId":"turn-1"}}"#)
            .unwrap(),
        vec![DecodedEvent::InputAccepted("message-1".into())]
    );
    assert!(
        decoder
            .decode_line(r#"{"id":"message-1","result":{"turnId":"old-turn"}}"#)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        decoder
            .decode_line(r#"{"id":"message-1","error":{"message":"turn ended"}}"#)
            .unwrap(),
        vec![DecodedEvent::InputRejected {
            id: "message-1".into(),
            message: "turn ended".into()
        }]
    );
}

#[test]
fn malformed_frames_are_recoverable_and_only_current_turn_is_terminal() {
    let mut decoder = decoder();
    assert!(decoder.decode_line("not json").is_err());
    assert!(decoder.decode_line(r#"{"method":"turn/completed","params":{"threadId":"other-thread","turn":{"id":"turn-1","status":"completed"}}}"#).unwrap().is_empty());
    assert!(decoder.decode_line(r#"{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"old-turn","status":"completed"}}}"#).unwrap().is_empty());
    assert_eq!(decoder.decode_line(r#"{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"failed","error":{"message":"denied"}}}}"#).unwrap(),
        vec![DecodedEvent::Error("denied".into()), DecodedEvent::TurnCompleted]);
    assert_eq!(decoder.decode_line(r#"{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}}"#).unwrap(),
        vec![DecodedEvent::TurnCompleted]);
}
