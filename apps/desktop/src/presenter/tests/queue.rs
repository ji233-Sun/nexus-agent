use super::*;

#[test]
fn queued_messages_start_one_at_a_time_and_resume_the_same_session() {
    for harness in HarnessKind::ALL {
        let (mut presenter, runner, _directory) = fixture();
        presenter.select_harness(harness, "claude");
        presenter
            .model
            .harnesses
            .insert(harness, ready_probe(harness));
        assert!(presenter.submit("first", harness.default_executable()));
        let task_id = presenter.model().conversation.active_task.unwrap();
        let mut run_id = presenter.model().conversation.active_run.unwrap();
        assert!(presenter.submit(" second ", harness.default_executable()));
        assert!(presenter.submit("third", harness.default_executable()));
        assert_eq!(presenter.model().conversation.messages.len(), 1);
        assert!(!presenter.submit(" \n ", harness.default_executable()));
        runner.emit(Event::RunSessionStarted {
            run_id,
            session_id: "queued-session".into(),
        });
        for (prompt, remaining) in [("second", 1), ("third", 0)] {
            runner.emit(Event::RunExited {
                run_id,
                status: RunStatus::Completed,
                exit_code: Some(0),
            });
            presenter.drain_events();
            let request = last_start(&runner);
            assert_ne!(request.run_id, run_id);
            assert_eq!(request.task_id, task_id);
            assert_eq!(request.session_id.as_deref(), Some("queued-session"));
            assert_eq!(request.prompt, prompt);
            assert_eq!(request.harness, harness);
            assert_eq!(
                presenter.model().conversation.queued_messages.len(),
                remaining
            );
            run_id = request.run_id;
        }
        runner.emit(Event::RunExited {
            run_id,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        assert!(presenter.model().conversation.active_run.is_none());
        assert_eq!(presenter.model().conversation.tasks.len(), 1);
        assert_eq!(
            presenter
                .model()
                .conversation
                .messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", "third"]
        );
    }
}

#[test]
fn failed_or_cancelled_runs_keep_the_queue_for_explicit_retry() {
    for status in [
        RunStatus::Failed,
        RunStatus::Cancelled,
        RunStatus::Interrupted,
        RunStatus::Completed,
    ] {
        let (mut presenter, runner, _directory) = fixture();
        assert!(presenter.submit("first", "claude"));
        let run_id = presenter.model().conversation.active_run.unwrap();
        let task_id = presenter.model().conversation.active_task.unwrap();
        assert!(presenter.submit("keep me", "claude"));
        assert!(presenter.submit("remove me", "claude"));
        let message_id = presenter.model().conversation.queued_messages[0].id;
        presenter.remove_queued_message(presenter.model().conversation.queued_messages[1].id);
        if status == RunStatus::Completed {
            // Stop can race with a successful exit; it must still pause the queue.
            presenter.cancel();
            assert!(!presenter.submit("too late", "claude"));
        }
        runner.emit(Event::RunSessionStarted {
            run_id,
            session_id: "saved-session".into(),
        });
        runner.emit(Event::RunExited {
            run_id,
            status,
            exit_code: None,
        });
        presenter.drain_events();
        assert!(presenter.model().conversation.active_run.is_none());
        assert_eq!(presenter.model().conversation.queued_messages.len(), 1);
        presenter.new_task();
        assert!(!presenter.send_queued_message(message_id));
        presenter.select_task(task_id);
        assert!(presenter.send_queued_message(message_id));
        assert_eq!(last_start(&runner).prompt, "keep me");
        assert!(presenter.model().conversation.queued_messages.is_empty());
    }
}

#[test]
fn queued_message_survives_missing_session_and_runner_send_failure() {
    for missing_session in [true, false] {
        let (mut presenter, runner, _directory) = fixture();
        assert!(presenter.submit("first", "claude"));
        let run_id = presenter.model().conversation.active_run.unwrap();
        assert!(presenter.submit("keep me", "claude"));
        if !missing_session {
            runner.emit(Event::RunSessionStarted {
                run_id,
                session_id: "saved-session".into(),
            });
            runner.0.borrow_mut().fail_send = true;
        }
        runner.emit(Event::RunExited {
            run_id,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        assert!(presenter.model().conversation.active_run.is_none());
        assert_eq!(
            presenter.model().conversation.queued_messages[0].prompt,
            "keep me"
        );
        assert!(
            presenter
                .model()
                .latest_log_text(presenter.model().language)
                .contains(if missing_session {
                    "无法继续对话"
                } else {
                    "Runner 不可用"
                })
        );
        if !missing_session {
            let task_id = presenter.model().conversation.selected_task.unwrap();
            let message_id = presenter.model().conversation.queued_messages[0].id;
            for _ in 0..2 {
                let stored = presenter.storage.messages(task_id).unwrap();
                assert_eq!(
                    stored.len(),
                    1,
                    "failed queue attempts must not add user messages"
                );
                assert_eq!(stored[0].content, "first");
                assert_eq!(
                    presenter
                        .storage
                        .tasks(
                            presenter
                                .model()
                                .conversation
                                .selected_project
                                .as_ref()
                                .unwrap()
                                .id
                        )
                        .unwrap()[0]
                        .status,
                    RunStatus::Completed
                );
                assert!(!presenter.send_queued_message(message_id));
                assert_eq!(
                    presenter.model().conversation.queued_messages[0].id,
                    message_id
                );
            }
            runner.0.borrow_mut().fail_send = false;
            assert!(presenter.send_queued_message(message_id));
            let request = last_start(&runner);
            assert_eq!(request.task_id, task_id);
            assert_eq!(request.session_id.as_deref(), Some("saved-session"));
            assert!(presenter.model().conversation.queued_messages.is_empty());
            presenter.select_task(task_id);
            assert_eq!(
                presenter
                    .model()
                    .conversation
                    .messages
                    .iter()
                    .map(|message| message.content.as_str())
                    .collect::<Vec<_>>(),
                ["first", "keep me"]
            );
            assert_eq!(presenter.model().conversation.messages[1].sequence, 2);
            assert_eq!(
                runner
                    .0
                    .borrow()
                    .commands
                    .iter()
                    .filter(|command| matches!(command.command, Command::RunStart(_)))
                    .count(),
                2
            );
        }
    }
}

#[test]
fn steer_uses_the_active_run_and_dequeues_only_after_a_matching_receipt() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("first", "claude"));
    let run_id = presenter.model().conversation.active_run.unwrap();
    assert!(presenter.submit("ordinary queue", "claude"));
    assert!(presenter.submit("urgent correction", "claude"));
    let message_id = presenter.model().conversation.queued_messages[1].id;
    assert!(presenter.steer_queued_message(message_id));
    assert_eq!(
        presenter.model().conversation.queued_messages[0].id,
        message_id
    );
    assert_eq!(presenter.model().conversation.messages.len(), 1);
    assert_eq!(
        presenter.model().conversation.steering_message,
        Some(message_id)
    );
    assert!(!presenter.steer_queued_message(message_id));
    presenter.remove_queued_message(message_id);
    assert_eq!(presenter.model().conversation.queued_messages.len(), 2);
    assert!(
        matches!(&runner.0.borrow().commands.last().unwrap().command,
        Command::RunSteer { run_id: id, message_id: received, prompt }
            if *id == run_id && *received == message_id && prompt == "urgent correction")
    );
    for event in [
        Event::RunInputAccepted {
            run_id: Uuid::new_v4(),
            message_id,
        },
        Event::RunInputAccepted {
            run_id,
            message_id: Uuid::new_v4(),
        },
    ] {
        runner.emit(event);
    }
    presenter.drain_events();
    assert_eq!(presenter.model().conversation.queued_messages.len(), 2);
    for _ in 0..2 {
        runner.emit(Event::RunInputAccepted { run_id, message_id });
    }
    presenter.drain_events();
    assert!(presenter.model().conversation.steering_message.is_none());
    assert_eq!(presenter.model().conversation.queued_messages.len(), 1);
    let user_messages = presenter
        .model()
        .conversation
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .collect::<Vec<_>>();
    assert_eq!(user_messages.len(), 2);
    assert_eq!(user_messages[1].content, "urgent correction");
    assert_eq!(user_messages[1].run_id, run_id);
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
fn accepted_steer_is_saved_without_changing_another_tasks_timeline() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("other conversation", "claude"));
    let other_task = presenter.model().conversation.active_task.unwrap();
    runner.emit(Event::RunExited {
        run_id: presenter.model().conversation.active_run.unwrap(),
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    presenter.new_task();

    assert!(presenter.submit("active conversation", "claude"));
    let task_id = presenter.model().conversation.active_task.unwrap();
    let run_id = presenter.model().conversation.active_run.unwrap();
    assert!(presenter.submit("correction for active conversation", "claude"));
    let message_id = presenter.model().conversation.queued_messages[0].id;
    assert!(presenter.steer_queued_message(message_id));
    presenter.select_task(other_task);
    let visible_message_ids = presenter
        .model()
        .conversation
        .messages
        .iter()
        .map(|message| message.id)
        .collect::<Vec<_>>();

    runner.emit(Event::RunInputAccepted { run_id, message_id });
    presenter.drain_events();

    assert!(presenter.model().conversation.queued_messages.is_empty());
    assert!(presenter.model().conversation.steering_message.is_none());
    assert_eq!(
        presenter.model().conversation.selected_task,
        Some(other_task)
    );
    assert_eq!(
        presenter
            .model()
            .conversation
            .messages
            .iter()
            .map(|message| message.id)
            .collect::<Vec<_>>(),
        visible_message_ids
    );
    assert_eq!(presenter.storage.messages(other_task).unwrap().len(), 1);
    presenter.select_task(task_id);
    let messages = &presenter.model().conversation.messages;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].content, "correction for active conversation");
    assert_eq!(messages[1].task_id, task_id);
    assert_eq!(messages[1].run_id, run_id);
}

#[test]
fn rejected_steer_falls_back_to_the_next_turn_and_survives_cancellation() {
    for cancelled in [false, true] {
        let (mut presenter, runner, _directory) = fixture();
        assert!(presenter.submit("first", "claude"));
        let run_id = presenter.model().conversation.active_run.unwrap();
        assert!(presenter.submit("ordinary queue", "claude"));
        assert!(presenter.submit("correction", "claude"));
        let message_id = presenter.model().conversation.queued_messages[1].id;
        assert!(presenter.steer_queued_message(message_id));
        runner.emit(Event::RunSessionStarted {
            run_id,
            session_id: "saved-session".into(),
        });
        if cancelled {
            presenter.cancel();
        }
        runner.emit(Event::RunInputRejected {
            run_id,
            message_id,
            message: "turn ended".into(),
        });
        presenter.drain_events();
        assert_eq!(presenter.model().conversation.queued_messages.len(), 2);
        assert!(presenter.model().conversation.steering_message.is_none());
        runner.emit(Event::RunExited {
            run_id,
            status: if cancelled {
                RunStatus::Cancelled
            } else {
                RunStatus::Completed
            },
            exit_code: None,
        });
        presenter.drain_events();
        if cancelled {
            assert_eq!(presenter.model().conversation.queued_messages.len(), 2);
            assert!(presenter.model().conversation.active_run.is_none());
        } else {
            let next = last_start(&runner);
            assert_ne!(next.run_id, run_id);
            assert_eq!(next.prompt, "correction");
            assert_eq!(next.session_id.as_deref(), Some("saved-session"));
            assert_eq!(
                presenter.model().conversation.queued_messages[0].prompt,
                "ordinary queue"
            );
        }
        runner.emit(Event::RunInputAccepted { run_id, message_id });
        presenter.drain_events();
        assert_eq!(
            presenter.model().conversation.queued_messages.len(),
            if cancelled { 2 } else { 1 }
        );
    }
}

#[test]
fn steer_send_failure_preserves_queue_order_and_stopping_blocks_steer() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("first", "claude"));
    assert!(presenter.submit("second", "claude"));
    assert!(presenter.submit("third", "claude"));
    let message_id = presenter.model().conversation.queued_messages[1].id;
    runner.0.borrow_mut().fail_send = true;
    assert!(!presenter.steer_queued_message(message_id));
    assert_eq!(
        presenter.model().conversation.queued_messages[0].prompt,
        "second"
    );
    assert!(presenter.model().conversation.steering_message.is_none());
    runner.0.borrow_mut().fail_send = false;
    presenter.cancel();
    assert!(!presenter.steer_queued_message(message_id));
}
