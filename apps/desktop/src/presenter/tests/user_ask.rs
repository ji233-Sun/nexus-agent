use super::*;

#[test]
fn user_ask_reply_uses_the_active_run_without_creating_a_prompt_or_run() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("ask me", "claude"));
    let run_id = presenter.model().conversation.active_run.unwrap();
    let request_id = Uuid::new_v4();
    runner.emit(Event::RunUserAskRequested {
        run_id: Uuid::new_v4(),
        request_id,
        questions: user_ask_questions(),
    });
    runner.emit(Event::RunUserAskRequested {
        run_id,
        request_id,
        questions: user_ask_questions(),
    });
    presenter.drain_events();
    assert_eq!(presenter.model().conversation.pending_user_asks.len(), 1);
    assert_eq!(
        presenter.model().conversation.pending_user_asks[0]
            .questions
            .len(),
        2
    );

    let answers = user_ask_answers();
    assert!(presenter.answer_user_ask(request_id, answers.clone()));
    assert_eq!(
        presenter.model().conversation.pending_user_asks[0].submission,
        UserAskSubmissionState::Submitting
    );
    assert!(!presenter.answer_user_ask(request_id, answers.clone()));
    assert!(matches!(
        &runner.0.borrow().commands.last().unwrap().command,
        Command::RunUserAskAnswer {
            run_id: id,
            request_id: request,
            answers: sent,
        } if *id == run_id && *request == request_id && *sent == answers
    ));

    runner.emit(Event::RunUserAskAnswerRejected {
        run_id,
        request_id,
        message: "invalid".into(),
    });
    presenter.drain_events();
    assert_eq!(
        presenter.model().conversation.pending_user_asks[0].submission,
        UserAskSubmissionState::Pending
    );
    assert_eq!(
        presenter.model().conversation.pending_user_asks[0]
            .error
            .as_deref(),
        Some("invalid")
    );
    assert!(presenter.answer_user_ask(request_id, answers));
    runner.emit(Event::RunUserAskAnswerSent { run_id, request_id });
    presenter.drain_events();
    assert_eq!(
        presenter.model().conversation.pending_user_asks[0].submission,
        UserAskSubmissionState::Sent
    );
    runner.emit(Event::RunUserAskAnswerRejected {
        run_id,
        request_id,
        message: "duplicate".into(),
    });
    presenter.drain_events();
    assert_eq!(
        presenter.model().conversation.pending_user_asks[0].submission,
        UserAskSubmissionState::Sent
    );
    runner.emit(Event::RunUserAskFinished {
        run_id,
        request_id,
        status: UserAskStatus::Answered,
        message: None,
    });
    presenter.drain_events();
    assert!(presenter.model().conversation.pending_user_asks.is_empty());
    assert_eq!(presenter.model().conversation.messages.len(), 3);
    assert!(
        presenter.model().conversation.messages[1]
            .content
            .contains("Agent 提问")
    );
    assert!(
        presenter.model().conversation.messages[1]
            .content
            .contains("Target?")
    );
    assert!(
        presenter.model().conversation.messages[2]
            .content
            .contains("User Ask 已回答")
    );
    assert!(
        presenter.model().conversation.messages[2]
            .content
            .contains("Workspace")
    );
    assert!(
        presenter.model().conversation.messages[2]
            .content
            .contains("Keep it focused")
    );
    assert_eq!(
        runner
            .0
            .borrow()
            .commands
            .iter()
            .filter(|command| matches!(command.command, Command::RunStart(_)))
            .count(),
        1
    );
}

#[test]
fn user_ask_drafts_cover_choice_text_navigation_and_retry_without_duplicate_submission() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("ask me", "claude"));
    let run_id = presenter.model().conversation.active_run.unwrap();
    let request_id = Uuid::new_v4();
    runner.emit(Event::RunUserAskRequested {
        run_id,
        request_id,
        questions: rich_user_ask_questions(),
    });
    presenter.drain_events();

    assert!(!presenter.can_submit_user_ask(request_id));
    assert!(presenter.set_user_ask_question(request_id, 3));
    assert!(!presenter.set_user_ask_question(request_id, 4));
    assert_eq!(
        presenter.model().conversation.pending_user_asks[0].active_question,
        3
    );
    assert!(presenter.toggle_user_ask_collapsed(request_id));
    assert!(presenter.model().conversation.pending_user_asks[0].collapsed);
    assert!(presenter.toggle_user_ask_collapsed(request_id));
    assert!(!presenter.model().conversation.pending_user_asks[0].collapsed);

    assert!(presenter.set_user_ask_option(request_id, "target", "library", true));
    assert!(presenter.set_user_ask_option(request_id, "target", "workspace", true));
    assert!(presenter.set_user_ask_option(request_id, "checks", "tests", true));
    assert!(presenter.set_user_ask_option(request_id, "checks", "clippy", true));
    assert!(presenter.set_user_ask_option(request_id, "scope", "focused", true));
    assert!(presenter.set_user_ask_text(request_id, "scope", String::new()));
    assert_eq!(
        presenter.model().conversation.pending_user_asks[0]
            .drafts
            .get("scope"),
        Some(&UserAskAnswerValue::Selected(vec!["focused".into()]))
    );
    assert!(presenter.set_user_ask_text(request_id, "scope", "entire workspace".into(),));
    assert!(presenter.set_user_ask_text(request_id, "note", "keep it focused".into()));
    assert!(!presenter.set_user_ask_text(request_id, "target", "invalid".into()));
    assert!(!presenter.set_user_ask_option(request_id, "note", "tests", true));
    assert!(!presenter.set_user_ask_option(request_id, "checks", "unknown", true));
    assert!(presenter.can_submit_user_ask(request_id));

    let expected = vec![
        UserAskAnswer {
            question_id: "target".into(),
            value: UserAskAnswerValue::Selected(vec!["workspace".into()]),
        },
        UserAskAnswer {
            question_id: "checks".into(),
            value: UserAskAnswerValue::Selected(vec!["tests".into(), "clippy".into()]),
        },
        UserAskAnswer {
            question_id: "scope".into(),
            value: UserAskAnswerValue::Text("entire workspace".into()),
        },
        UserAskAnswer {
            question_id: "note".into(),
            value: UserAskAnswerValue::Text("keep it focused".into()),
        },
    ];
    assert!(presenter.submit_user_ask(request_id));
    assert!(!presenter.submit_user_ask(request_id));
    assert!(matches!(
        &runner.0.borrow().commands.last().unwrap().command,
        Command::RunUserAskAnswer { answers, .. } if answers == &expected
    ));
    assert_eq!(
        presenter.model().conversation.pending_user_asks[0].submitted_answers,
        Some(expected.clone())
    );

    runner.emit(Event::RunUserAskAnswerRejected {
        run_id,
        request_id,
        message: "try again".into(),
    });
    presenter.drain_events();
    let request = &presenter.model().conversation.pending_user_asks[0];
    assert_eq!(request.submission, UserAskSubmissionState::Pending);
    assert_eq!(request.error.as_deref(), Some("try again"));
    assert_eq!(request.submitted_answers, None);
    assert_eq!(
        request.drafts.get("note"),
        Some(&UserAskAnswerValue::Text("keep it focused".into()))
    );
    assert!(presenter.can_submit_user_ask(request_id));
    assert!(presenter.submit_user_ask(request_id));
    assert_eq!(
        runner
            .0
            .borrow()
            .commands
            .iter()
            .filter(|command| matches!(command.command, Command::RunUserAskAnswer { .. }))
            .count(),
        2
    );
}

#[test]
fn user_ask_terminal_events_preserve_request_order_and_write_read_only_history() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("ask twice", "claude"));
    let run_id = presenter.model().conversation.active_run.unwrap();
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    for request_id in [first, second] {
        runner.emit(Event::RunUserAskRequested {
            run_id,
            request_id,
            questions: user_ask_questions(),
        });
    }
    presenter.drain_events();
    assert_eq!(
        presenter
            .model()
            .conversation
            .pending_user_asks
            .iter()
            .map(|request| request.request_id)
            .collect::<Vec<_>>(),
        vec![first, second]
    );

    runner.emit(Event::RunUserAskFinished {
        run_id,
        request_id: first,
        status: UserAskStatus::Cancelled,
        message: Some("native request closed".into()),
    });
    presenter.drain_events();
    assert_eq!(presenter.model().conversation.pending_user_asks.len(), 1);
    assert_eq!(
        presenter.model().conversation.pending_user_asks[0].request_id,
        second
    );
    assert_eq!(
        presenter
            .model()
            .latest_log_text(presenter.model().language),
        "Agent 正在等待你的回答。"
    );
    assert!(
        presenter
            .model()
            .conversation
            .messages
            .iter()
            .any(|message| {
                message.content.contains("User Ask 已取消")
                    && message.content.contains("native request closed")
            })
    );

    runner.emit(Event::RunExited {
        run_id,
        status: RunStatus::Failed,
        exit_code: Some(1),
    });
    presenter.drain_events();
    assert!(presenter.model().conversation.pending_user_asks.is_empty());
    assert!(
        presenter
            .model()
            .conversation
            .messages
            .iter()
            .any(|message| {
                message.content.contains("User Ask 已失效") && message.content.contains("Target?")
            })
    );
    assert!(!presenter.submit_user_ask(second));
}

#[test]
fn user_ask_send_failure_and_run_exit_leave_no_stale_desktop_state() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("ask me", "claude"));
    let run_id = presenter.model().conversation.active_run.unwrap();
    let request_id = Uuid::new_v4();
    runner.emit(Event::RunUserAskRequested {
        run_id,
        request_id,
        questions: user_ask_questions(),
    });
    presenter.drain_events();

    runner.0.borrow_mut().fail_send = true;
    assert!(!presenter.answer_user_ask(request_id, user_ask_answers()));
    assert_eq!(
        presenter.model().conversation.pending_user_asks[0].submission,
        UserAskSubmissionState::Pending
    );
    assert!(
        presenter.model().conversation.pending_user_asks[0]
            .error
            .is_some()
    );
    runner.0.borrow_mut().fail_send = false;

    runner.emit(Event::RunExited {
        run_id,
        status: RunStatus::Failed,
        exit_code: Some(1),
    });
    presenter.drain_events();
    assert!(presenter.model().conversation.pending_user_asks.is_empty());
    assert!(presenter.model().conversation.active_run.is_none());
    assert!(!presenter.answer_user_ask(request_id, user_ask_answers()));
}
