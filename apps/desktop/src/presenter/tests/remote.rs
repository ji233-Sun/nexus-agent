use super::*;

#[test]
fn expired_remote_start_does_not_change_selection_or_start_a_run() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.select_harness(HarnessKind::Codex, "claude"));
    presenter
        .model
        .harnesses
        .insert(HarnessKind::Codex, ready_probe(HarnessKind::Codex));
    emit_current_catalog(
        &presenter,
        &runner,
        vec![catalog_model(
            "explicit-remote-model",
            true,
            &[ThinkingEffort::Low, ThinkingEffort::High],
            ThinkingEffort::Low,
        )],
    );
    presenter.drain_events();
    presenter.select_catalog_model(Some("explicit-remote-model".into()));
    presenter.select_effort(ThinkingEffort::High);
    let project_id = presenter
        .model
        .conversation
        .selected_project
        .as_ref()
        .unwrap()
        .id;
    let other_directory = tempfile::tempdir().unwrap();
    presenter.open_project(other_directory.path());
    assert!(
        !presenter
            .model()
            .selected_catalog_model()
            .unwrap()
            .is_default
    );
    runner.0.borrow_mut().commands.clear();
    let selected_project_id = presenter
        .model
        .conversation
        .selected_project
        .as_ref()
        .unwrap()
        .id;
    let status = presenter
        .model
        .latest_log_text(presenter.model.language)
        .to_owned();
    let (reply, response) = tokio::sync::oneshot::channel();
    drop(response);

    assert!(!presenter.handle_remote_command(RemoteCommand::StartRun {
        project_id,
        prompt: "expired request".into(),
        reply,
    }));

    assert_eq!(
        presenter
            .model
            .conversation
            .selected_project
            .as_ref()
            .unwrap()
            .id,
        selected_project_id
    );
    assert_eq!(
        presenter.model.latest_log_text(presenter.model.language),
        status
    );
    assert!(presenter.model.conversation.active_run.is_none());
    assert!(presenter.storage.tasks(project_id).unwrap().is_empty());
    assert!(runner.0.borrow().commands.is_empty());

    let (reply, mut response) = tokio::sync::oneshot::channel();
    assert!(presenter.handle_remote_command(RemoteCommand::StartRun {
        project_id,
        prompt: "retry request".into(),
        reply,
    }));
    assert_eq!(response.try_recv().unwrap(), Ok(()));
    let request = last_start(&runner);
    assert_eq!(request.model.as_deref(), Some("explicit-remote-model"));
    assert_eq!(request.effort, ThinkingEffort::High);
    assert_eq!(presenter.remote_state().model, request.model);
    assert_eq!(presenter.remote_state().effort, request.effort);
    let config = presenter
        .storage
        .conversation_config(request.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(config.model, "explicit-remote-model");
    assert_eq!(config.effort, ThinkingEffort::High);
    assert_eq!(presenter.storage.tasks(project_id).unwrap().len(), 1);
    assert_eq!(
        runner
            .0
            .borrow()
            .commands
            .iter()
            .filter(|envelope| matches!(envelope.command, Command::RunStart(_)))
            .count(),
        1
    );
    let task_id = presenter.model().conversation.selected_task.unwrap();
    let run_id = presenter.model().conversation.active_run.unwrap();
    runner.emit(Event::RunSessionStarted {
        run_id,
        session_id: "desktop-session".into(),
    });
    runner.emit(Event::RunExited {
        run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();

    // Remote StartRun remains an explicit new-task request, even with a task selected.
    let (reply, mut response) = tokio::sync::oneshot::channel();
    presenter.handle_remote_command(RemoteCommand::StartRun {
        project_id,
        prompt: " ".into(),
        reply,
    });
    assert!(response.try_recv().unwrap().is_err());
    assert_eq!(presenter.model().conversation.selected_task, Some(task_id));
    let (reply, mut response) = tokio::sync::oneshot::channel();
    presenter.handle_remote_command(RemoteCommand::StartRun {
        project_id,
        prompt: "new remote task".into(),
        reply,
    });
    assert_eq!(response.try_recv().unwrap(), Ok(()));
    assert_ne!(presenter.model().conversation.selected_task, Some(task_id));
    assert_eq!(presenter.storage.tasks(project_id).unwrap().len(), 2);
    let state = runner.0.borrow();
    let Command::RunStart(request) = &state.commands.last().unwrap().command else {
        panic!("expected new remote task");
    };
    assert!(request.session_id.is_none());
}

#[test]
fn remote_state_uses_the_selected_provider_profile() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    let credentials = FakeCredentialStore::default();
    let mut presenter = Presenter::new_with_credentials(
        storage,
        Err(anyhow::anyhow!("runner unavailable")),
        None,
        Box::new(credentials),
    );
    presenter.open_project(directory.path());
    assert!(presenter.select_harness(HarnessKind::Omp, "claude"));
    presenter.model.harnesses.insert(
        HarnessKind::Omp,
        HarnessProbe {
            authenticated: false,
            ..ready_probe(HarnessKind::Omp)
        },
    );
    presenter
        .save_provider_profile(profile_draft(None, "DeepSeek", "super-secret"))
        .unwrap();
    let (reply, mut response) = tokio::sync::oneshot::channel();

    assert!(!presenter.handle_remote_command(RemoteCommand::GetState { reply }));
    let state = response.try_recv().unwrap();
    assert_eq!(state.harness, HarnessKind::Omp);
    assert_eq!(state.model.as_deref(), Some("deepseek/deepseek-v4-pro"));
    assert!(state.harness_ready);
}

#[test]
fn constructing_presenter_does_not_start_remote_service() {
    let (mut presenter, _, _directory) = fixture();
    assert!(presenter.remote_endpoint().is_none());
    assert!(
        presenter
            .storage
            .setting(crate::remote_control::TOKEN_SETTING_KEY)
            .unwrap()
            .is_none()
    );
    presenter.attach_remote_control(Err(anyhow::anyhow!("remote port unavailable")));
    assert_eq!(
        presenter.remote_control_error(),
        Some("remote port unavailable")
    );
}
