use super::*;

#[test]
fn tool_events_preserve_ids_and_full_payloads_after_reloading_a_task() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("run tools", "claude"));
    let run_id = presenter.model().conversation.active_run.unwrap();
    let task_id = presenter.model().conversation.active_task.unwrap();
    let long_text = "完整内容\n".repeat(100);
    for tool_id in ["a", "b"] {
        runner.emit(Event::RunToolStarted {
            run_id,
            tool_id: tool_id.into(),
            name: "Bash".into(),
            summary: long_text.clone(),
        });
    }
    for tool_id in ["b", "a"] {
        runner.emit(Event::RunToolCompleted {
            run_id,
            tool_id: tool_id.into(),
            output: long_text.clone(),
            is_error: tool_id == "b",
        });
    }
    presenter.drain_events();
    presenter.select_task(task_id);
    let messages = &presenter.model().conversation.messages;
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[1].tool.as_ref().unwrap().id, "a");
    assert_eq!(messages[2].tool.as_ref().unwrap().id, "b");
    assert_eq!(messages[3].tool.as_ref().unwrap().id, "b");
    assert!(messages[3].tool.as_ref().unwrap().is_error);
    assert!(!messages[4].tool.as_ref().unwrap().is_error);
    assert_eq!(messages[1].content, format!("Bash\n{long_text}"));
    assert_eq!(messages[4].content, long_text);
    let items = crate::model::tools::timeline_items(
        messages,
        &presenter.model().conversation.completed_runs,
    );
    let crate::model::tools::TimelineItem::Tools(batch) = &items[1] else {
        panic!("expected one tool batch")
    };
    assert_eq!(items.len(), 2);
    assert_eq!(batch.len(), 2);
    assert_eq!(batch[0].result.unwrap().id, messages[4].id);
    assert_eq!(batch[1].result.unwrap().id, messages[3].id);
    assert!(!batch[0].is_error());
    assert!(batch[1].is_error());
}

#[test]
fn permission_modes_are_snapshotted_per_turn_and_restored_per_harness_and_conversation() {
    let (mut presenter, runner, _directory) = fixture();
    presenter.select_permission_mode(PermissionMode::Ask);
    assert!(presenter.submit("first", "claude"));
    let first = last_start(&runner);
    assert_eq!(first.permission_mode, PermissionMode::Ask);
    runner.emit(Event::RunSessionStarted {
        run_id: first.run_id,
        session_id: "session".into(),
    });
    presenter.drain_events();
    presenter.select_permission_mode(PermissionMode::Yolo);
    assert!(presenter.submit("second", "claude"));
    let queued = presenter.model().conversation.queued_messages[0].id;
    assert!(!presenter.steer_queued_message(queued));
    presenter.select_permission_mode(PermissionMode::AutoEdit);
    assert!(presenter.submit("third", "claude"));
    presenter.select_permission_mode(PermissionMode::Ask);
    assert_eq!(last_start(&runner).permission_mode, PermissionMode::Ask);
    assert_eq!(
        presenter.model().conversation.active_permission_mode,
        Some(PermissionMode::Ask)
    );
    runner.emit(Event::RunExited {
        run_id: first.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    let second = last_start(&runner);
    assert_eq!(second.task_id, first.task_id);
    assert_eq!(second.permission_mode, PermissionMode::Yolo);
    assert_eq!(second.session_id.as_deref(), Some("session"));
    assert_eq!(
        presenter.model().conversation.active_permission_mode,
        Some(PermissionMode::Yolo)
    );
    assert_eq!(
        presenter.model().conversation.queued_messages[0].permission_mode,
        PermissionMode::AutoEdit
    );
    runner.emit(Event::RunExited {
        run_id: second.run_id,
        status: RunStatus::Failed,
        exit_code: Some(1),
    });
    presenter.drain_events();
    presenter.select_task(first.task_id);
    assert_eq!(
        presenter.model().conversation.permission_mode,
        PermissionMode::Yolo
    );
    let config = presenter
        .storage
        .conversation_config(first.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(config.permission_mode, PermissionMode::Yolo);
    presenter.new_task();
    assert_eq!(
        presenter.model().conversation.permission_mode,
        PermissionMode::Ask
    );
    assert!(presenter.select_harness(HarnessKind::Omp, "claude"));
    assert_eq!(
        presenter.model().conversation.permission_mode,
        PermissionMode::AutoEdit
    );
    presenter.select_permission_mode(PermissionMode::Yolo);
    assert!(presenter.select_harness(HarnessKind::Claude, "omp"));
    assert_eq!(
        presenter.model().conversation.permission_mode,
        PermissionMode::Ask
    );
    assert!(presenter.select_harness(HarnessKind::Omp, "claude"));
    assert_eq!(
        presenter.model().conversation.permission_mode,
        PermissionMode::Yolo
    );
}

#[test]
fn approvals_ignore_stale_events_validate_choices_and_remain_retryable_until_resolved() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("first", "claude"));
    let run_id = presenter.model().conversation.active_run.unwrap();
    let request = nexus_protocol::ApprovalRequest {
        request_id: Uuid::new_v4(),
        title: "Bash".into(),
        details: "echo test".into(),
        options: vec!["Approve".into(), "Deny".into()],
    };
    runner.emit(Event::RunApprovalRequested {
        run_id: Uuid::new_v4(),
        request: request.clone(),
    });
    presenter.drain_events();
    assert!(presenter.model().conversation.pending_approvals.is_empty());
    for _ in 0..2 {
        runner.emit(Event::RunApprovalRequested {
            run_id,
            request: request.clone(),
        });
    }
    let second = nexus_protocol::ApprovalRequest {
        request_id: Uuid::new_v4(),
        ..request.clone()
    };
    runner.emit(Event::RunApprovalRequested {
        run_id,
        request: second.clone(),
    });
    presenter.drain_events();
    assert_eq!(presenter.model().conversation.pending_approvals.len(), 2);
    assert!(!presenter.respond_approval(Uuid::new_v4(), request.request_id, Some(0)));
    assert!(!presenter.respond_approval(run_id, second.request_id, Some(0)));
    assert!(!presenter.respond_approval(run_id, request.request_id, Some(2)));
    runner.0.borrow_mut().fail_send = true;
    assert!(!presenter.respond_approval(run_id, request.request_id, Some(0)));
    assert!(presenter.model().conversation.responding_approval.is_none());
    runner.0.borrow_mut().fail_send = false;
    assert!(presenter.respond_approval(run_id, request.request_id, Some(1)));
    assert!(!presenter.respond_approval(run_id, request.request_id, Some(1)));
    assert_eq!(presenter.model().conversation.pending_approvals.len(), 2);
    assert!(matches!(runner.0.borrow().commands.last().unwrap().command,
        Command::RunApprovalRespond { run_id: run, request_id, option: Some(1) } if run == run_id && request_id == request.request_id));
    runner.emit(Event::RunApprovalResolved {
        run_id,
        request_id: request.request_id,
    });
    presenter.drain_events();
    assert_eq!(presenter.model().conversation.pending_approvals.len(), 1);
    assert!(presenter.respond_approval(run_id, second.request_id, None));
    presenter.cancel();
    assert!(presenter.model().conversation.pending_approvals.is_empty());
    assert!(presenter.model().conversation.responding_approval.is_none());
    runner.emit(Event::RunApprovalRequested {
        run_id,
        request: second,
    });
    presenter.drain_events();
    assert!(presenter.model().conversation.pending_approvals.is_empty());
    assert!(!presenter.respond_approval(run_id, request.request_id, Some(0)));
}

#[test]
fn invalid_submissions_never_create_tasks_or_send_commands() {
    let (mut presenter, runner, _directory) = fixture();
    let project = presenter
        .model
        .conversation
        .selected_project
        .take()
        .unwrap();
    assert!(!presenter.submit("hello", "claude"));
    assert_eq!(
        presenter
            .model()
            .latest_log_text(presenter.model().language),
        "任务缺少绑定目录"
    );
    presenter.model.conversation.selected_project = Some(project.clone());
    assert!(!presenter.submit(" \n ", "claude"));
    assert_eq!(
        presenter
            .model()
            .latest_log_text(presenter.model().language),
        "Prompt 不能为空。"
    );
    assert!(!presenter.submit("hello", " "));
    for (available, authenticated) in [(false, false), (true, false), (false, true)] {
        let probe = presenter
            .model
            .harnesses
            .get_mut(&HarnessKind::Claude)
            .unwrap();
        probe.available = available;
        probe.authenticated = authenticated;
        assert!(!presenter.model().can_submit());
        assert!(!presenter.submit("hello", "claude"));
    }
    assert!(presenter.storage.tasks(project.id).unwrap().is_empty());
    assert!(runner.0.borrow().commands.is_empty());
    assert!(presenter.model.conversation.active_run_started_at.is_none());
    assert!(
        presenter
            .model()
            .conversation
            .active_run_elapsed_seconds
            .is_none()
    );
}

#[test]
fn submit_persists_configuration_and_queues_without_starting_concurrent_runs() {
    let (mut presenter, runner, _directory) = fixture();
    presenter.select_catalog_model(Some("opus".into()));
    presenter.select_effort(ThinkingEffort::XHigh);
    assert!(presenter.model().can_submit());
    assert!(presenter.submit("  explain this project\n", "claude-custom"));
    assert!(!presenter.model().can_submit());
    assert!(presenter.submit("follow-up", "claude-custom"));
    assert_eq!(presenter.model().conversation.queued_messages.len(), 1);
    assert_eq!(
        presenter.model().conversation.queued_messages[0].prompt,
        "follow-up"
    );
    let state = runner.0.borrow();
    assert_eq!(state.commands.len(), 1);
    let Command::RunStart(request) = &state.commands[0].command else {
        panic!("expected start");
    };
    assert_eq!(request.prompt, "explain this project");
    assert!(request.session_id.is_none());
    assert_eq!(request.model.as_deref(), Some("opus"));
    assert_eq!(request.effort, ThinkingEffort::Default);
    assert_eq!(
        request.executable,
        ready_probe(HarnessKind::Claude).executable
    );
    assert_eq!(
        presenter.model().conversation.active_run,
        Some(request.run_id)
    );
    assert_eq!(
        presenter.model().conversation.selected_task,
        Some(request.task_id)
    );
    let config = presenter
        .storage
        .conversation_config(request.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(config.executable, request.executable);
    assert_eq!(config.model, "opus");
    assert_eq!(config.effort, request.effort);
    assert_eq!(
        presenter.model().conversation.messages[0].content,
        request.prompt
    );
    assert_eq!(presenter.model().conversation.tasks.len(), 1);
    assert_eq!(
        presenter.model().conversation.tasks[0].title,
        "explain this project"
    );
}

#[test]
fn follow_up_resumes_the_saved_session_after_reopening_and_new_task_starts_fresh() {
    for harness in HarnessKind::ALL {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("nexus.db");
        let runner = FakeRunner::default();
        let mut presenter = Presenter::new(
            Storage::open(&database).unwrap(),
            Ok(Box::new(runner.clone())),
            None,
        );
        presenter.open_project(directory.path());
        presenter.select_harness(harness, "claude");
        presenter
            .model
            .harnesses
            .insert(harness, ready_probe(harness));
        assert!(presenter.submit("first question", harness.default_executable()));
        let cwd = directory.path().canonicalize().unwrap();
        assert_eq!(Path::new(&last_start(&runner).cwd), cwd);
        assert_eq!(presenter.model().working_directory(), cwd.to_str());
        let task_id = presenter.model().conversation.active_task.unwrap();
        let first_run = presenter.model().conversation.active_run.unwrap();
        let first_message = presenter.model().conversation.messages[0].id;
        runner.emit(Event::RunSessionStarted {
            run_id: first_run,
            session_id: "saved-session".into(),
        });
        runner.emit(Event::RunMessageCompleted {
            run_id: first_run,
            text: "first answer".into(),
        });
        runner.emit(Event::RunExited {
            run_id: first_run,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        drop(presenter);

        let mut presenter = Presenter::new(
            Storage::open(&database).unwrap(),
            Ok(Box::new(runner.clone())),
            None,
        );
        presenter.open_project(directory.path());
        let saved_probe = ready_probe(harness);
        let other_probe = HarnessProbe {
            executable: format!("/other/{harness}"),
            ..saved_probe.clone()
        };
        presenter
            .model
            .harnesses
            .insert(harness, other_probe.clone());
        runner.0.borrow_mut().commands.clear();
        presenter.select_task(task_id);
        assert_eq!(presenter.model().working_directory(), cwd.to_str());
        assert!(!presenter.submit("follow-up before probe", &saved_probe.executable));
        assert!(!presenter.model().can_submit());
        assert_eq!(
            presenter.model().conversation.executable,
            saved_probe.executable
        );
        assert!(runner.0.borrow().commands.iter().any(|command| matches!(
            &command.command, Command::HarnessProbe { harness: probed_harness, executable, .. }
                if *probed_harness == harness && executable == &saved_probe.executable
        )));
        if harness == HarnessKind::Codex {
            assert!(runner.0.borrow().commands.iter().any(|command| matches!(
                &command.command, Command::ModelCatalogRefresh { executable, .. }
                    if executable == &saved_probe.executable
            )));
        }
        // A late result for another executable must not authorize this session.
        runner.emit(Event::HarnessDetected(other_probe));
        presenter.drain_events();
        assert!(!presenter.submit("follow-up with stale probe", &saved_probe.executable));
        assert!(
            presenter
                .model()
                .latest_log_text(presenter.model().language)
                .contains("可执行文件不一致")
        );
        assert!(
            runner
                .0
                .borrow()
                .commands
                .iter()
                .all(|command| !matches!(command.command, Command::RunStart(_)))
        );
        assert_eq!(presenter.model().conversation.messages.len(), 2);

        runner.emit(Event::HarnessDetected(saved_probe.clone()));
        presenter.drain_events();
        assert!(presenter.model().can_submit());
        assert!(presenter.submit("  follow-up  ", harness.default_executable()));
        let second_run = presenter.model().conversation.active_run.unwrap();
        assert_ne!(first_run, second_run);
        assert_eq!(presenter.model().conversation.selected_task, Some(task_id));
        assert_eq!(presenter.model().conversation.tasks.len(), 1);
        assert_eq!(
            presenter.model().conversation.tasks[0].title,
            "first question"
        );
        assert_eq!(presenter.model().conversation.messages[0].id, first_message);
        assert_eq!(
            presenter
                .model()
                .conversation
                .messages
                .iter()
                .map(|message| (message.sequence, message.content.as_str(), message.run_id))
                .collect::<Vec<_>>(),
            vec![
                (1, "first question", first_run),
                (2, "first answer", first_run),
                (3, "follow-up", second_run)
            ]
        );
        {
            let state = runner.0.borrow();
            let Command::RunStart(request) = &state.commands.last().unwrap().command else {
                panic!("expected follow-up run");
            };
            assert_eq!(request.task_id, task_id);
            assert_eq!(request.session_id.as_deref(), Some("saved-session"));
            assert_eq!(request.prompt, "follow-up");
            assert_eq!(request.harness, harness);
            assert_eq!(request.executable, saved_probe.executable);
            assert_eq!(
                Some(request.cwd.as_str()),
                presenter.model().working_directory()
            );
        }
        // A failed continuation must not lose the saved session.
        runner.emit(Event::RunExited {
            run_id: second_run,
            status: RunStatus::Failed,
            exit_code: Some(1),
        });
        presenter.drain_events();
        assert!(presenter.submit("retry", harness.default_executable()));
        let retry_run = presenter.model().conversation.active_run.unwrap();
        {
            let state = runner.0.borrow();
            let Command::RunStart(request) = &state.commands.last().unwrap().command else {
                panic!("expected retry run");
            };
            assert_eq!(request.task_id, task_id);
            assert_eq!(request.session_id.as_deref(), Some("saved-session"));
        }
        runner.emit(Event::RunExited {
            run_id: retry_run,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        presenter.new_task();
        assert!(presenter.submit("new question", harness.default_executable()));
        assert_eq!(presenter.model().working_directory(), cwd.to_str());
        let state = runner.0.borrow();
        let Command::RunStart(request) = &state.commands.last().unwrap().command else {
            panic!("expected new task");
        };
        assert_ne!(request.task_id, task_id);
        assert!(request.session_id.is_none());
        assert_eq!(
            Some(request.cwd.as_str()),
            presenter.model().working_directory()
        );
        assert_eq!(presenter.model().conversation.tasks.len(), 2);
        assert_eq!(presenter.model().conversation.messages.len(), 1);
    }
}

#[test]
fn follow_up_does_not_silently_restart_when_the_session_is_missing_or_harness_changes() {
    for missing_session in [true, false] {
        let (mut presenter, runner, _directory) = fixture();
        assert!(presenter.submit("first question", "claude"));
        let run_id = presenter.model().conversation.active_run.unwrap();
        if !missing_session {
            runner.emit(Event::RunSessionStarted {
                run_id,
                session_id: "claude-session".into(),
            });
        }
        runner.emit(Event::RunExited {
            run_id,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        if !missing_session {
            presenter.select_harness(HarnessKind::Codex, "claude");
            presenter
                .model
                .harnesses
                .insert(HarnessKind::Codex, ready_probe(HarnessKind::Codex));
        }
        runner.0.borrow_mut().commands.clear();
        assert!(!presenter.submit("follow-up", "configured-cli"));
        assert!(runner.0.borrow().commands.is_empty());
        assert!(presenter.model().conversation.active_run.is_none());
        assert_eq!(presenter.model().conversation.messages.len(), 1);
        assert_eq!(presenter.model().conversation.tasks.len(), 1);
        assert!(
            presenter
                .model()
                .latest_log_text(presenter.model().language)
                .contains(if missing_session {
                    "未保存可恢复的会话"
                } else {
                    "切回该 Harness"
                })
        );
    }
}

#[test]
fn send_failure_rolls_back_a_new_task_without_entering_busy_state() {
    let (mut presenter, runner, _directory) = fixture();
    runner.0.borrow_mut().fail_send = true;
    assert!(!presenter.submit("hello", "claude"));
    assert!(presenter.model().conversation.active_run.is_none());
    assert!(presenter.model.conversation.active_run_started_at.is_none());
    assert!(
        presenter
            .model()
            .conversation
            .active_run_elapsed_seconds
            .is_none()
    );
    assert_eq!(
        presenter
            .model()
            .latest_log_text(presenter.model().language),
        "Runner 不可用，任务未启动。"
    );
    let project_id = presenter
        .model()
        .conversation
        .selected_project
        .as_ref()
        .unwrap()
        .id;
    assert!(presenter.storage.tasks(project_id).unwrap().is_empty());
    runner.0.borrow_mut().fail_send = false;
    assert!(presenter.submit("hello", "claude"));
    assert_eq!(presenter.storage.tasks(project_id).unwrap().len(), 1);
    assert_eq!(presenter.model().conversation.messages.len(), 1);
    assert_eq!(presenter.model().conversation.messages[0].content, "hello");
}

#[test]
fn runner_events_update_timeline_and_persist_terminal_statuses() {
    for status in [
        RunStatus::Completed,
        RunStatus::Failed,
        RunStatus::Cancelled,
        RunStatus::Interrupted,
    ] {
        let (mut presenter, runner, _directory) = fixture();
        assert!(presenter.submit("hello", "claude"));
        let started = presenter.model.conversation.active_run_started_at.unwrap();
        assert!(presenter.refresh_run_elapsed(started + Duration::from_secs(5)));
        let run_id = presenter.model().conversation.active_run.unwrap();
        let task_id = presenter.model().conversation.active_task.unwrap();
        runner.emit(Event::RunStarted { run_id, pid: 42 });
        runner.emit(Event::RunSessionStarted {
            run_id,
            session_id: "saved-session".into(),
        });
        runner.emit(Event::RunOutputDelta {
            run_id,
            text: "partial".into(),
        });
        assert!(presenter.drain_events());
        assert_eq!(presenter.model().conversation.streaming_text, "partial");
        assert!(!presenter.drain_events());
        runner.emit(Event::RunMessageCompleted {
            run_id,
            text: "answer".into(),
        });
        presenter.drain_events();
        assert!(
            !presenter
                .model()
                .conversation
                .completed_runs
                .contains(&run_id)
        );
        if status == RunStatus::Failed {
            runner.emit(Event::RunFailed {
                run_id,
                code: ErrorCode::UnexpectedExit,
                message: "failed".into(),
            });
        }
        runner.emit(Event::RunExited {
            run_id,
            status,
            exit_code: Some(0),
        });
        presenter.drain_events();
        assert!(presenter.model().conversation.streaming_text.is_empty());
        assert!(presenter.model().conversation.active_run.is_none());
        assert!(presenter.model().conversation.active_task.is_none());
        assert!(presenter.model().conversation.active_harness.is_none());
        assert!(presenter.model.conversation.active_run_started_at.is_none());
        assert!(
            presenter
                .model()
                .conversation
                .active_run_elapsed_seconds
                .is_none()
        );
        assert!(!presenter.refresh_run_elapsed(started + Duration::from_secs(10)));
        assert_eq!(presenter.model().conversation.tasks[0].status, status);
        assert_eq!(
            presenter
                .model()
                .conversation
                .completed_runs
                .contains(&run_id),
            status == RunStatus::Completed
        );
        assert_eq!(
            presenter
                .storage
                .completed_runs(task_id)
                .unwrap()
                .contains(&run_id),
            status == RunStatus::Completed
        );
        let messages = presenter.storage.messages(task_id).unwrap();
        assert_eq!(messages[1].content, "answer");
        assert_eq!(messages[1].sequence, 2);
        if status == RunStatus::Failed {
            assert_eq!(messages[2].kind, MessageKind::Error);
        }
        presenter.new_task();
        presenter.select_task(task_id);
        assert_eq!(
            presenter
                .model()
                .conversation
                .completed_runs
                .contains(&run_id),
            status == RunStatus::Completed
        );
        assert!(presenter.submit("next task", "claude"));
        assert_eq!(presenter.model().conversation.selected_task, Some(task_id));
        assert_eq!(presenter.model().conversation.tasks.len(), 1);
        assert_eq!(
            presenter.model().conversation.active_run_elapsed_seconds,
            Some(0)
        );
        assert!(
            !presenter
                .model()
                .conversation
                .completed_runs
                .contains(&presenter.model().conversation.active_run.unwrap())
        );
        assert!(presenter.model.conversation.active_run_started_at.unwrap() >= started);
    }
}

#[test]
fn run_elapsed_advances_without_output_and_only_changes_each_second() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(!presenter.refresh_run_elapsed(Instant::now()));
    assert!(presenter.submit("hello", "claude"));
    let started = presenter.model.conversation.active_run_started_at.unwrap();
    let run_id = presenter.model().conversation.active_run.unwrap();
    assert_eq!(
        presenter.model().conversation.active_run_elapsed_seconds,
        Some(0)
    );
    assert!(!presenter.refresh_run_elapsed(started + Duration::from_millis(999)));
    assert!(!presenter.drain_events());
    assert!(presenter.refresh_run_elapsed(started + Duration::from_secs(1)));
    assert_eq!(
        presenter.model().conversation.active_run_elapsed_seconds,
        Some(1)
    );
    assert!(!presenter.refresh_run_elapsed(started + Duration::from_millis(1999)));

    runner.emit(Event::RunStarted { run_id, pid: 42 });
    runner.emit(Event::RunOutputDelta {
        run_id,
        text: "partial".into(),
    });
    assert!(presenter.drain_events());
    assert_eq!(
        presenter.model.conversation.active_run_started_at,
        Some(started)
    );
    assert!(presenter.refresh_run_elapsed(started + Duration::from_secs(65)));
    assert_eq!(
        presenter.model().conversation.active_run_elapsed_seconds,
        Some(65)
    );

    presenter.cancel();
    assert!(presenter.refresh_run_elapsed(started + Duration::from_secs(66)));
    assert_eq!(
        presenter.model().conversation.active_run_elapsed_seconds,
        Some(66)
    );
    assert_eq!(
        presenter.model.conversation.active_run_started_at,
        Some(started)
    );
    assert!(presenter.model().conversation.active_run.is_some());
}

#[test]
fn unrelated_run_events_cannot_replace_the_active_run() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("hello", "claude"));
    let active_run = presenter.model().conversation.active_run;
    let started = presenter.model.conversation.active_run_started_at;
    let other_run = Uuid::new_v4();
    runner.emit(Event::RunStarted {
        run_id: other_run,
        pid: 42,
    });
    runner.emit(Event::RunSessionStarted {
        run_id: other_run,
        session_id: "unrelated".into(),
    });
    runner.emit(Event::RunOutputDelta {
        run_id: other_run,
        text: "unrelated".into(),
    });
    runner.emit(Event::RunMessageCompleted {
        run_id: other_run,
        text: "unrelated".into(),
    });
    runner.emit(Event::RunExited {
        run_id: other_run,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    assert_eq!(presenter.model().conversation.active_run, active_run);
    assert_eq!(presenter.model.conversation.active_run_started_at, started);
    assert_eq!(
        presenter.model().conversation.active_run_elapsed_seconds,
        Some(0)
    );
    assert_eq!(presenter.model().conversation.messages.len(), 1);
    assert!(presenter.model().conversation.streaming_text.is_empty());
    assert!(
        presenter
            .storage
            .conversation_config(presenter.model().conversation.active_task.unwrap())
            .unwrap()
            .unwrap()
            .session_id
            .is_none()
    );
}

#[test]
fn active_run_locks_configuration_and_cancels_the_matching_run() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("hello", "claude"));
    let task_id = presenter.model().conversation.active_task;
    let run_id = presenter.model().conversation.active_run.unwrap();
    presenter.select_catalog_model(Some("opus".into()));
    presenter.select_effort(ThinkingEffort::Max);
    assert!(!presenter.select_harness(HarnessKind::Codex, "claude"));
    assert!(!presenter.select_model_configuration(HarnessKind::Codex, None, "claude"));
    presenter.new_task();
    assert!(presenter.model().conversation.model_override.is_none());
    assert_eq!(
        presenter.model().conversation.effort,
        ThinkingEffort::Default
    );
    assert!(presenter.model().conversation.selected_task.is_none());
    assert_eq!(presenter.model().active_run_count(), 1);
    presenter.select_task(task_id.unwrap());
    presenter.cancel();
    assert!(matches!(runner.0.borrow().commands.last().unwrap().command,
        Command::RunCancel { run_id: id } if id == run_id));
    let project_id = presenter
        .model()
        .conversation
        .selected_project
        .as_ref()
        .unwrap()
        .id;
    assert_eq!(
        presenter.storage.tasks(project_id).unwrap()[0].status,
        RunStatus::Cancelling
    );
}
