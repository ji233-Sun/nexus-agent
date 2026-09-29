use super::*;

#[test]
fn launch_spec_includes_model_and_effort_without_prompt_in_argv() {
    for (mode, value) in [
        (PermissionMode::Ask, "default"),
        (PermissionMode::AutoEdit, "acceptEdits"),
        (PermissionMode::Yolo, "bypassPermissions"),
    ] {
        let spec = build_launch_spec(
            "claude",
            Path::new("."),
            "test",
            None,
            ThinkingEffort::Default,
            Some("session"),
            mode,
        );
        assert!(
            spec.args
                .windows(2)
                .any(|pair| pair == ["--permission-mode", value])
        );
        assert!(
            spec.args
                .windows(2)
                .any(|pair| pair == ["--permission-prompt-tool", "stdio"])
        );
        assert_eq!(
            spec.args
                .iter()
                .any(|arg| arg == "--allow-dangerously-skip-permissions"),
            mode == PermissionMode::Yolo
        );
    }
    let defaults = build_launch_spec(
        "claude",
        Path::new("."),
        "test",
        None,
        ThinkingEffort::Default,
        None,
        PermissionMode::AutoEdit,
    );
    assert!(
        !defaults
            .args
            .iter()
            .any(|arg| arg == "--model" || arg == "--effort")
    );
    for model_id in ["moonshotai/Kimi-K2.5", "GLM-5"] {
        let custom = build_launch_spec(
            "claude",
            Path::new("."),
            "test",
            Some(model_id),
            ThinkingEffort::Default,
            None,
            PermissionMode::AutoEdit,
        );
        assert!(
            custom
                .args
                .windows(2)
                .any(|pair| pair == ["--model", model_id])
        );
        assert!(!custom.args.iter().any(|arg| arg == "--effort"));
    }
    let spec = build_launch_spec(
        "/usr/local/bin/claude",
        Path::new("/tmp/project"),
        "secret prompt",
        Some("opus"),
        ThinkingEffort::XHigh,
        None,
        PermissionMode::AutoEdit,
    );
    assert!(spec.args.windows(2).any(|pair| pair == ["--model", "opus"]));
    assert!(
        spec.args
            .windows(2)
            .any(|pair| pair == ["--effort", "xhigh"])
    );
    assert!(!spec.args.iter().any(|arg| arg.contains("secret prompt")));
    assert!(
        spec.args
            .windows(2)
            .any(|pair| pair == ["--input-format", "stream-json"])
    );
    assert!(spec.args.iter().any(|arg| arg == "--replay-user-messages"));
    assert_eq!(
        serde_json::from_str::<Value>(&spec.stdin).unwrap()["message"]["content"],
        "secret prompt"
    );
    assert!(
        !spec
            .args
            .iter()
            .any(|arg| arg == "--no-session-persistence" || arg == "--resume")
    );
    let resumed = build_launch_spec(
        "claude",
        Path::new("/tmp/project"),
        "follow-up",
        None,
        ThinkingEffort::High,
        Some("existing-session"),
        PermissionMode::AutoEdit,
    );
    assert!(
        resumed
            .args
            .windows(2)
            .any(|pair| pair == ["--resume", "existing-session"])
    );
    assert!(
        resumed
            .args
            .windows(2)
            .any(|pair| pair == ["--permission-mode", "acceptEdits"])
    );
    assert!(
        !resumed
            .args
            .iter()
            .any(|arg| arg == "--no-session-persistence" || arg == "--fork-session")
    );
    assert_eq!(
        serde_json::from_str::<Value>(&resumed.stdin).unwrap()["message"]["content"],
        "follow-up"
    );
}

#[test]
fn tool_approval_waits_for_user_and_preserves_input_and_cancellation() {
    let mut decoder = EventDecoder::default();
    let frame = json!({"type": "control_request", "request_id": "approval-1", "request": {
        "subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "echo \"审批\"", "timeout": 1000}
    }});
    let events = decoder.decode_line(&frame.to_string()).unwrap();
    let [DecodedEvent::ApprovalRequested(prompt)] = events.as_slice() else {
        panic!("expected approval")
    };
    assert!(prompt.details.contains("审批"));
    assert_eq!(
        prompt.options[0].response.0["response"]["response"]["updatedInput"],
        frame["request"]["input"]
    );
    assert_eq!(
        prompt.options[0].response.0["response"]["request_id"],
        "approval-1"
    );
    assert_eq!(
        prompt.options[1].response.0["response"]["response"]["behavior"],
        "deny"
    );
    assert_eq!(prompt.cancel, prompt.options[1].response);
    assert_eq!(
        decoder
            .decode_line(r#"{"type":"control_cancel_request","request_id":"approval-1"}"#)
            .unwrap(),
        vec![DecodedEvent::ApprovalResolved(prompt.id.clone())]
    );
}

#[test]
fn ask_user_question_maps_questions_and_builds_sdk_response() {
    let mut decoder = EventDecoder::default();
    let input = json!({
        "questions": [
            {"question": "Which features?", "header": "Features", "multiSelect": true,
             "options": [
                {"label": "Fast", "description": "Optimize latency"},
                {"label": "Safe", "description": "Prefer checks"}
             ]},
            {"question": "Anything else?", "header": "Notes"}
        ],
        "metadata": {"preserve": true}
    });
    let frame = json!({"type": "control_request", "request_id": "ask-1", "request": {
        "subtype": "can_use_tool", "tool_name": "AskUserQuestion", "input": input
    }});
    let events = decoder.decode_line(&frame.to_string()).unwrap();
    let [DecodedEvent::UserAskRequested(request)] = events.as_slice() else {
        panic!("expected user ask")
    };
    assert_eq!(request.native_request_id, "ask-1");
    assert_eq!(request.timeout_ms, None);
    assert!(request.resolve_on_send);
    assert_eq!(request.questions[0].id, "Which features?");
    assert_eq!(request.questions[0].prompt, "Which features?");
    assert_eq!(
        request.questions[0].answer_mode,
        UserAskAnswerMode::Choice {
            multiple: true,
            allow_custom: true
        }
    );
    assert_eq!(request.questions[0].options[0].id, "Fast");
    assert_eq!(request.questions[0].options[0].label, "Fast");
    assert_eq!(
        request.questions[0].options[0].description.as_deref(),
        Some("Optimize latency")
    );
    assert_eq!(request.questions[1].id, "Anything else?");
    assert_eq!(request.questions[1].answer_mode, UserAskAnswerMode::Text);

    let response = decoder
        .answer_user_ask(
            "ask-1",
            &[
                UserAskAnswer {
                    question_id: "Which features?".into(),
                    value: UserAskAnswerValue::Selected(vec!["Fast".into(), "Safe".into()]),
                },
                UserAskAnswer {
                    question_id: "Anything else?".into(),
                    value: UserAskAnswerValue::Text("custom text".into()),
                },
            ],
        )
        .unwrap();
    assert_eq!(response.0["response"]["response"]["behavior"], "allow");
    let mut expected_input = input;
    expected_input["answers"] = json!({
        "Which features?": "Fast, Safe", "Anything else?": "custom text"
    });
    assert_eq!(
        response.0["response"]["response"]["updatedInput"],
        expected_input
    );
    assert_eq!(
        response.0["response"]["response"]["updatedInput"]["answers"],
        json!({
            "Which features?": "Fast, Safe", "Anything else?": "custom text"
        })
    );
    assert!(decoder.answer_user_ask("ask-1", &[]).is_none());
}

#[test]
fn ask_user_question_cancel_clears_pending_and_rejects_late_answer() {
    let mut decoder = EventDecoder::default();
    let frame = json!({"type": "control_request", "request_id": "ask-cancel", "request": {
        "subtype": "can_use_tool", "tool_name": "AskUserQuestion",
        "input": {"questions": [{"question": "Continue?", "options": [{"label": "Yes"}]}]}
    }});
    assert!(matches!(
        decoder.decode_line(&frame.to_string()).unwrap().as_slice(),
        [DecodedEvent::UserAskRequested(_)]
    ));
    assert_eq!(
        decoder
            .decode_line(r#"{"type":"control_cancel_request","request_id":"ask-cancel"}"#)
            .unwrap(),
        vec![DecodedEvent::UserAskFinished {
            native_request_id: "ask-cancel".into(),
            status: UserAskStatus::Cancelled,
            message: None,
        }]
    );
    assert!(
        decoder
            .answer_user_ask(
                "ask-cancel",
                &[UserAskAnswer {
                    question_id: "Continue?".into(),
                    value: UserAskAnswerValue::Selected(vec!["Yes".into()]),
                }]
            )
            .is_none()
    );
}

#[test]
fn title_launch_spec_disables_tools_and_edit_approval() {
    let spec = build_title_launch_spec(
        "/usr/local/bin/claude",
        Path::new("/tmp/project"),
        "title prompt",
        Some("sonnet"),
        ThinkingEffort::Low,
    );

    assert!(
        spec.args
            .windows(2)
            .any(|pair| pair == ["--permission-mode", "dontAsk"])
    );
    assert!(spec.args.windows(2).any(|pair| pair == ["--tools", ""]));
    assert!(!spec.args.iter().any(|arg| arg == "acceptEdits"));
    assert!(!spec.args.iter().any(|arg| arg.contains("title prompt")));
}

#[test]
fn decoder_maps_text_and_tool_events() {
    let mut decoder = EventDecoder::default();
    assert_eq!(
        decoder
            .decode_line(r#"{"type":"system","subtype":"init","session_id":"existing-session"}"#)
            .unwrap(),
        vec![
            DecodedEvent::SessionStarted("existing-session".into()),
            DecodedEvent::Status("Claude: init".into())
        ]
    );
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

    let code = "let value = \"完整内容\";\n".repeat(100);
    let input = serde_json::json!({"file_path": "src/main.rs", "content": code});
    let tool = serde_json::json!({"type": "assistant", "message": {"content": [
        {"type": "tool_use", "id": "write-1", "name": "Write", "input": input}
    ]}});
    assert!(matches!(
        decoder.decode_line(&tool.to_string()).unwrap().as_slice(),
        [DecodedEvent::ToolStarted { summary, .. }]
            if serde_json::from_str::<Value>(summary).unwrap() == input
    ));
    let result = serde_json::json!({"type": "user", "message": {"content": [
        {"type": "tool_result", "tool_use_id": "write-1", "content": code, "is_error": true}
    ]}});
    assert!(matches!(
        decoder.decode_line(&result.to_string()).unwrap().as_slice(),
        [DecodedEvent::ToolCompleted { id, output, is_error: true }]
            if id == "write-1" && output == &code
    ));
}

#[test]
fn malformed_frames_are_recoverable() {
    let mut decoder = EventDecoder::default();
    assert!(decoder.decode_line("not json").is_err());
    assert_eq!(
        decoder
            .decode_line(r#"{"type":"result","result":"done"}"#)
            .unwrap(),
        vec![DecodedEvent::TurnCompleted]
    );
}

#[test]
fn unrelated_results_keep_the_user_turn_and_pending_question_open() {
    let mut decoder = EventDecoder::default();
    decoder
        .steer("user-message", "continue with this image")
        .unwrap();
    let ask = json!({"type":"control_request","request_id":"ask","request":{
        "subtype":"can_use_tool","tool_name":"AskUserQuestion",
        "input":{"questions":[{"question":"Continue?"}]}
    }});
    assert!(matches!(
        decoder.decode_line(&ask.to_string()).unwrap().as_slice(),
        [DecodedEvent::UserAskRequested(_)]
    ));
    for result in [
        json!({"type":"result","subtype":"success","is_error":false,"result":"",
            "origin":{"kind":"task-notification"}}),
        json!({"type":"result","subtype":"error_during_execution","is_error":true,
            "errors":["background task stopped"],"origin":{"kind":"task-notification"}}),
        json!({"type":"result","is_error":false,"user_message_uuid":"previous-message"}),
    ] {
        assert!(decoder.decode_line(&result.to_string()).unwrap().is_empty());
    }
    assert!(
        decoder
            .answer_user_ask(
                "ask",
                &[UserAskAnswer {
                    question_id: "Continue?".into(),
                    value: UserAskAnswerValue::Text("yes".into()),
                }]
            )
            .is_some()
    );
    assert_eq!(
        decoder
            .decode_line(r#"{"type":"result","is_error":false,"user_message_uuid":"user-message"}"#)
            .unwrap(),
        vec![DecodedEvent::TurnCompleted]
    );
}

#[test]
fn background_results_finish_when_they_answer_our_prompt_or_steer() {
    for echoed_ids in [
        json!({"user_message_uuid":"user-message"}),
        json!({"user_message_uuid":"steer-message"}),
        json!({"user_message_uuid":"another-message", "user_message_uuids":["user-message","another-message"]}),
        json!({"user_message_uuid":"another-message", "user_message_uuids":["steer-message","another-message"]}),
    ] {
        let mut decoder = EventDecoder::default();
        decoder
            .steer("user-message", "continue with this image")
            .unwrap();
        decoder
            .steer("steer-message", "also inspect the text")
            .unwrap();
        let mut result = json!({"type":"result","subtype":"success","is_error":false,
            "result":"done","origin":{"kind":"task-notification"}});
        result
            .as_object_mut()
            .unwrap()
            .extend(echoed_ids.as_object().unwrap().clone());
        assert_eq!(
            decoder.decode_line(&result.to_string()).unwrap(),
            vec![DecodedEvent::TurnCompleted]
        );
        result["is_error"] = true.into();
        result["errors"] = json!(["user turn failed"]);
        assert_eq!(
            decoder.decode_line(&result.to_string()).unwrap(),
            vec![
                DecodedEvent::Error("[\"user turn failed\"]".into()),
                DecodedEvent::TurnCompleted,
            ]
        );
    }
}

#[test]
fn steering_replays_the_message_id_and_reports_failed_turns() {
    let mut decoder = EventDecoder::default();
    let frame = decoder
        .steer("message-id", "new instruction\n第二行")
        .unwrap();
    assert_eq!(frame.0["uuid"], "message-id");
    assert_eq!(frame.0["message"]["content"], "new instruction\n第二行");
    assert_eq!(
        decoder.decode_line(&frame.0.to_string()).unwrap(),
        vec![DecodedEvent::InputAccepted("message-id".into())]
    );
    assert!(
        matches!(decoder.decode_line(r#"{"type":"result","is_error":true,"errors":["denied"]}"#).unwrap().as_slice(),
        [DecodedEvent::Error(message), DecodedEvent::TurnCompleted] if message.contains("denied"))
    );
}
