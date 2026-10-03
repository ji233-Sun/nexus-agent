use super::*;

#[test]
fn generated_title_replaces_fallback_and_is_persisted() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("  **请修复登录流程。**\n并补充回归测试  ", "claude"));
    let task_id = presenter.model().conversation.selected_task.unwrap();
    assert_eq!(
        presenter.model().conversation.tasks[0].title,
        "请修复登录流程。 并补充回归测试"
    );

    runner.emit(Event::TaskTitleGenerated {
        task_id,
        title: "```".into(),
    });
    assert!(presenter.drain_events());
    assert_eq!(
        presenter.model().conversation.tasks[0].title,
        "请修复登录流程。 并补充回归测试"
    );

    runner.emit(Event::TaskTitleGenerated {
        task_id,
        title: "**修复登录流程。**".into(),
    });
    assert!(presenter.drain_events());

    assert_eq!(
        presenter.model().conversation.tasks[0].title,
        "修复登录流程"
    );
    let project_id = presenter
        .model()
        .conversation
        .selected_project
        .as_ref()
        .unwrap()
        .id;
    assert_eq!(
        presenter.storage.tasks(project_id).unwrap()[0].title,
        "修复登录流程"
    );
}

#[test]
fn title_generation_settings_persist_independently_of_conversation_selection() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("title-settings.sqlite");
    let storage = Storage::open(&path).unwrap();
    storage
        .set_setting(
            "title_generation",
            r#"{"harness":"claude","model":"haiku"}"#,
        )
        .unwrap();
    storage.set_setting("thinking_effort", "high").unwrap();
    let mut presenter = Presenter::new(storage, Err(anyhow::anyhow!("test")), None);
    assert_eq!(
        presenter.model.title_generation.harness,
        HarnessKind::Claude
    );
    assert_eq!(
        presenter.model.title_generation.model.as_deref(),
        Some("haiku")
    );
    assert_eq!(
        presenter.model.title_generation.effort,
        ThinkingEffort::Default
    );
    assert_eq!(presenter.model.conversation.effort, ThinkingEffort::High);
    assert!(presenter.select_generation_harness(GenerationKind::Title, HarnessKind::Omp));
    assert!(!presenter.select_generation_model(GenerationKind::Title, Some("missing".into())));
    presenter.model.conversation.title_model_catalog =
        ModelCatalogState::Ready(vec![catalog_model(
            "provider/title-model",
            false,
            &[ThinkingEffort::Low],
            ThinkingEffort::Default,
        )]);
    assert!(
        presenter
            .select_generation_model(GenerationKind::Title, Some("provider/title-model".into()))
    );
    assert!(presenter.select_generation_effort(GenerationKind::Title, ThinkingEffort::Low));
    assert!(!presenter.select_generation_effort(GenerationKind::Title, ThinkingEffort::High));
    assert_eq!(presenter.model.conversation.effort, ThinkingEffort::High);
    assert!(presenter.select_harness(HarnessKind::Codex, "claude"));
    let expected = presenter.model.title_generation.clone();
    assert!(presenter.select_generation_harness(GenerationKind::Commit, HarnessKind::Codex));
    presenter.model.conversation.commit_model_catalog =
        ModelCatalogState::Ready(vec![catalog_model(
            "commit-model",
            false,
            &[ThinkingEffort::Medium],
            ThinkingEffort::Default,
        )]);
    assert!(presenter.select_generation_model(GenerationKind::Commit, Some("commit-model".into())));
    assert!(presenter.select_generation_effort(GenerationKind::Commit, ThinkingEffort::Medium));
    assert!(!presenter.select_generation_effort(GenerationKind::Commit, ThinkingEffort::High));
    let expected_commit = presenter.model.commit_message_generation.clone();
    assert_eq!(presenter.model.title_generation, expected);
    drop(presenter);
    let mut presenter = Presenter::new(
        Storage::open(&path).unwrap(),
        Err(anyhow::anyhow!("test")),
        None,
    );
    assert_eq!(presenter.model.title_generation, expected);
    assert_eq!(presenter.model.commit_message_generation, expected_commit);
    assert_eq!(
        presenter
            .generation_configuration(GenerationKind::Title)
            .unwrap()
            .effort,
        ThinkingEffort::Low
    );
    assert_eq!(
        presenter.model.conversation.selected_harness,
        HarnessKind::Codex
    );
    assert!(presenter.select_generation_harness(GenerationKind::Title, HarnessKind::Claude));
    assert!(presenter.model.title_generation.model.is_none());
    assert_eq!(
        presenter.model.title_generation.effort,
        ThinkingEffort::Default
    );
    assert_eq!(
        presenter.model.conversation.selected_harness,
        HarnessKind::Codex
    );
}

#[test]
fn commit_message_generation_uses_selected_changes_and_routes_results_to_the_owner() {
    let (mut presenter, runner, _directory, start) = worktree_fixture("commit message task");
    let cwd = Path::new(&start.cwd);
    fs::write(cwd.join("tracked.txt"), "selected content\n").unwrap();
    fs::write(cwd.join("unselected.txt"), "private unselected content\n").unwrap();
    let conversation_status = presenter.model.conversation.run_status.clone();
    presenter.toggle_changes_sidebar();
    finish_workspace_operation(&mut presenter);
    assert_eq!(presenter.model.conversation.run_status, conversation_status);
    assert!(presenter.model.conversation.changes_status.is_none());
    presenter.select_changed_file("tracked.txt".into(), true);
    assert!(
        !presenter.generate_workspace_commit_message(),
        "running checkout must be blocked"
    );
    runner.emit(Event::RunExited {
        run_id: start.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    let conversation_status = presenter.model.conversation.run_status.clone();
    presenter.select_changed_file("tracked.txt".into(), false);
    presenter.toggle_commit_editor();
    assert_eq!(presenter.model.conversation.selected_changes.len(), 2);
    presenter.select_changed_file("unselected.txt".into(), false);
    assert!(presenter.review_conversation_changes());
    finish_workspace_operation(&mut presenter);
    assert!(presenter.model.conversation.commit_editor_open);
    assert_eq!(
        presenter
            .model
            .conversation
            .selected_changes
            .iter()
            .cloned()
            .collect::<Vec<_>>(),
        ["tracked.txt"]
    );
    presenter.model.commit_message_generation.model = Some("commit-model".into());
    assert!(presenter.generate_workspace_commit_message());
    finish_workspace_operation(&mut presenter);
    let owner = presenter.model.conversation.id;
    let request_id = presenter.model.conversation.commit_message_request.unwrap();
    let commands = runner.0.borrow();
    let command = commands
        .commands
        .iter()
        .rev()
        .find_map(|command| match &command.command {
            Command::GenerateCommitMessage {
                request_id,
                cwd,
                diff,
                language,
                configuration,
            } => Some((*request_id, cwd, diff, language, configuration)),
            _ => None,
        })
        .unwrap();
    assert_eq!(command.0, request_id);
    assert_eq!(command.1, &start.cwd);
    assert!(command.2.contains("+selected content"));
    assert!(!command.2.contains("unselected"));
    assert_eq!(command.3, "zh-CN");
    assert_eq!(command.4.model.as_deref(), Some("commit-model"));
    drop(commands);
    presenter.new_task();
    runner.emit(Event::CommitMessageGenerated {
        request_id,
        message: "fix: Describe selected changes\n\nBody".into(),
    });
    presenter.drain_events();
    assert!(presenter.model.conversation.commit_message.is_empty());
    presenter.model.activate_conversation(owner);
    assert_eq!(
        presenter.model.conversation.commit_message,
        "fix: Describe selected changes\n\nBody"
    );
    assert!(
        presenter
            .model
            .conversation
            .commit_message_request
            .is_none()
    );

    for edit_draft in [true, false] {
        assert!(presenter.generate_workspace_commit_message());
        finish_workspace_operation(&mut presenter);
        let request_id = presenter.model.conversation.commit_message_request.unwrap();
        if edit_draft {
            presenter.set_commit_message("manual edit".into());
        } else {
            presenter.select_changed_file("tracked.txt".into(), false);
        }
        runner.emit(Event::CommitMessageGenerated {
            request_id,
            message: "late result".into(),
        });
        presenter.drain_events();
        assert_eq!(presenter.model.conversation.commit_message, "manual edit");
        assert!(
            presenter
                .model
                .conversation
                .commit_message_request
                .is_none()
        );
    }
    presenter.select_changed_file("tracked.txt".into(), true);
    assert!(presenter.generate_workspace_commit_message());
    finish_workspace_operation(&mut presenter);
    let request_id = presenter.model.conversation.commit_message_request.unwrap();
    runner.emit(Event::CommitMessageFailed {
        request_id,
        message: "offline".into(),
    });
    presenter.drain_events();
    assert_eq!(presenter.model.conversation.commit_message, "manual edit");
    assert!(
        presenter
            .model
            .conversation
            .commit_message_request
            .is_none()
    );
    assert_eq!(presenter.model.conversation.run_status, conversation_status);
    assert_eq!(
        presenter
            .model
            .conversation
            .changes_status
            .as_ref()
            .unwrap()
            .render(presenter.model.language),
        "offline"
    );
    fs::write(cwd.join("tracked.txt"), "newer contents\n").unwrap();
    let before = runner.0.borrow().commands.len();
    assert!(presenter.generate_workspace_commit_message());
    finish_workspace_operation(&mut presenter);
    assert!(
        presenter
            .model
            .conversation
            .commit_message_request
            .is_none()
    );
    assert_eq!(
        runner.0.borrow().commands.len(),
        before,
        "stale diffs must not be sent to the model"
    );
}

#[test]
fn commit_model_catalog_does_not_replace_conversation_or_title_catalogs() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.refresh_generation_model_catalog(GenerationKind::Title));
    let title_request = match presenter.model.conversation.title_model_catalog {
        ModelCatalogState::Loading { request_id, .. } => request_id,
        _ => panic!("title catalog loading"),
    };
    assert!(presenter.refresh_generation_model_catalog(GenerationKind::Commit));
    let request_id = match presenter.model.conversation.commit_model_catalog {
        ModelCatalogState::Loading { request_id, .. } => request_id,
        _ => panic!("commit catalog loading"),
    };
    assert!(matches!(
        &runner.0.borrow().commands.last().unwrap().command,
        Command::ModelCatalogRefresh {
            purpose: ModelCatalogPurpose::CommitMessageGeneration,
            ..
        }
    ));
    assert!(presenter.select_generation_harness(GenerationKind::Commit, HarnessKind::Omp));
    runner.emit(Event::ModelCatalogLoaded {
        request_id,
        harness: HarnessKind::Claude,
        models: claude_aliases(),
    });
    presenter.drain_events();
    assert!(matches!(
        presenter.model.conversation.commit_model_catalog,
        ModelCatalogState::Idle
    ));
    assert!(
        presenter
            .model
            .conversation
            .title_model_catalog
            .accepts(title_request)
    );
    assert_eq!(
        presenter.model.conversation.model_catalog.models().unwrap(),
        claude_aliases()
    );
}

#[test]
fn title_generation_uses_separate_configuration_and_skips_resumed_runs() {
    let (mut presenter, runner, credentials, _directory) = provider_fixture();
    presenter.select_harness(HarnessKind::Omp, "claude");
    let mut draft = profile_draft(None, "Title", "title-secret");
    draft.model = "provider/title-model".into();
    let title_profile = presenter.save_provider_profile(draft).unwrap();
    presenter.select_generation_harness(GenerationKind::Title, HarnessKind::Omp);
    presenter.model.conversation.title_model_catalog =
        ModelCatalogState::Ready(vec![catalog_model(
            "provider/title-model",
            false,
            &[ThinkingEffort::Low],
            ThinkingEffort::Default,
        )]);
    assert!(presenter.select_generation_effort(GenerationKind::Title, ThinkingEffort::Low));
    let profile_default = presenter
        .generation_configuration(GenerationKind::Title)
        .unwrap();
    assert_eq!(
        profile_default.model.as_deref(),
        Some("provider/title-model")
    );
    assert_eq!(profile_default.effort, ThinkingEffort::Low);
    assert!(
        presenter
            .select_generation_model(GenerationKind::Title, Some("provider/title-model".into()))
    );
    presenter.select_harness(HarnessKind::Claude, "/custom/omp");
    presenter
        .save_provider_profile(profile_draft(None, "Conversation", "conversation-secret"))
        .unwrap();
    presenter
        .model
        .harnesses
        .insert(HarnessKind::Claude, ready_probe(HarnessKind::Claude));
    emit_current_catalog(&presenter, &runner, claude_aliases());
    presenter.drain_events();
    presenter.select_catalog_model(Some("opus".into()));

    assert!(presenter.submit("first", "claude"));
    let first = last_start(&runner);
    assert_eq!(first.harness, HarnessKind::Claude);
    assert_eq!(first.model.as_deref(), Some("opus"));
    assert_eq!(first.effort, ThinkingEffort::Default);
    assert_eq!(first.environment[0].value, "conversation-secret");
    let title = first.title_generation.as_ref().unwrap();
    assert_eq!(title.harness, HarnessKind::Omp);
    assert_eq!(title.executable, "/custom/omp");
    assert_eq!(title.model.as_deref(), Some("provider/title-model"));
    assert_eq!(title.effort, ThinkingEffort::Low);
    assert_eq!(title.environment[0].value, "title-secret");
    assert!(!format!("{first:?}").contains("title-secret"));

    runner.emit(Event::RunSessionStarted {
        run_id: first.run_id,
        session_id: "session".into(),
    });
    runner.emit(Event::RunExited {
        run_id: first.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    assert!(presenter.submit("continue", "claude"));
    let resumed = last_start(&runner);
    assert!(resumed.title_generation.is_none());
    runner.emit(Event::RunExited {
        run_id: resumed.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    presenter.new_task();
    credentials.delete_api_key(title_profile).unwrap();
    assert!(presenter.submit("missing title credentials", "claude"));
    assert!(last_start(&runner).title_generation.is_none());
    assert_eq!(
        presenter.model.conversation.tasks[0].title,
        "missing title credentials"
    );
}

#[test]
fn title_effort_tracks_model_support_and_resets_after_catalog_changes() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.select_generation_harness(GenerationKind::Title, HarnessKind::Codex));
    assert!(!presenter.select_generation_effort(GenerationKind::Title, ThinkingEffort::Low));
    presenter.model.conversation.title_model_catalog = ModelCatalogState::Ready(vec![
        catalog_model(
            "default-model",
            true,
            &[ThinkingEffort::Low],
            ThinkingEffort::Low,
        ),
        catalog_model(
            "other-model",
            false,
            &[ThinkingEffort::Low],
            ThinkingEffort::Low,
        ),
        catalog_model("no-effort", false, &[], ThinkingEffort::Default),
    ]);
    assert!(presenter.select_generation_effort(GenerationKind::Title, ThinkingEffort::Low));
    assert!(presenter.select_generation_model(GenerationKind::Title, Some("other-model".into())));
    assert_eq!(presenter.model.title_generation.effort, ThinkingEffort::Low);
    assert!(presenter.select_generation_model(GenerationKind::Title, Some("no-effort".into())));
    assert_eq!(
        presenter.model.title_generation.effort,
        ThinkingEffort::Default
    );
    assert!(!presenter.select_generation_effort(GenerationKind::Title, ThinkingEffort::Low));
    assert!(presenter.select_generation_model(GenerationKind::Title, None));
    assert!(presenter.select_generation_effort(GenerationKind::Title, ThinkingEffort::Low));

    assert!(presenter.refresh_generation_model_catalog(GenerationKind::Title));
    let ModelCatalogState::Loading { request_id, .. } =
        presenter.model.conversation.title_model_catalog
    else {
        panic!("loading")
    };
    runner.emit(Event::ModelCatalogLoaded {
        request_id: Uuid::new_v4(),
        harness: HarnessKind::Codex,
        models: Vec::new(),
    });
    presenter.drain_events();
    assert_eq!(presenter.model.title_generation.effort, ThinkingEffort::Low);
    runner.emit(Event::ModelCatalogLoaded {
        request_id,
        harness: HarnessKind::Codex,
        models: vec![catalog_model(
            "default-model",
            true,
            &[],
            ThinkingEffort::Default,
        )],
    });
    presenter.drain_events();
    assert_eq!(
        presenter.model.title_generation.effort,
        ThinkingEffort::Default
    );
    let saved: GenerationSettings = serde_json::from_str(
        &presenter
            .storage
            .setting("title_generation")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(saved.effort, ThinkingEffort::Default);
    assert_eq!(
        presenter.model.conversation.selected_harness,
        HarnessKind::Claude
    );
    assert_eq!(presenter.model.conversation.effort, ThinkingEffort::Default);
}

#[test]
fn title_model_catalog_is_independent_and_ignores_stale_results() {
    let (mut presenter, runner, _directory) = fixture();
    presenter.refresh_model_catalog();
    let conversation_request = current_catalog_request_id(&presenter);
    assert!(presenter.refresh_generation_model_catalog(GenerationKind::Title));
    let ModelCatalogState::Loading {
        request_id: stale_request,
        ..
    } = presenter.model.conversation.title_model_catalog
    else {
        panic!("loading")
    };
    assert!(presenter.select_generation_harness(GenerationKind::Title, HarnessKind::Omp));
    assert!(presenter.refresh_generation_model_catalog(GenerationKind::Title));
    let ModelCatalogState::Loading { request_id, .. } =
        presenter.model.conversation.title_model_catalog
    else {
        panic!("loading")
    };
    let status = presenter
        .model
        .latest_log_text(presenter.model.language)
        .to_owned();
    for (id, harness) in [
        (stale_request, HarnessKind::Claude),
        (request_id, HarnessKind::Claude),
    ] {
        runner.emit(Event::ModelCatalogLoaded {
            request_id: id,
            harness,
            models: claude_aliases(),
        });
    }
    presenter.drain_events();
    assert!(
        presenter
            .model
            .conversation
            .title_model_catalog
            .accepts(request_id)
    );
    assert_eq!(current_catalog_request_id(&presenter), conversation_request);
    runner.emit(Event::ModelCatalogLoaded {
        request_id,
        harness: HarnessKind::Omp,
        models: vec![catalog_model(
            "provider/title-model",
            false,
            &[],
            ThinkingEffort::Default,
        )],
    });
    presenter.drain_events();
    assert_eq!(
        presenter.model.latest_log_text(presenter.model.language),
        status
    );
    assert!(
        presenter
            .select_generation_model(GenerationKind::Title, Some("provider/title-model".into()))
    );
    assert!(
        !presenter.select_generation_model(GenerationKind::Title, Some("not-in-catalog".into()))
    );
    emit_current_catalog(&presenter, &runner, claude_aliases());
    presenter.drain_events();
    assert_eq!(
        presenter.model.conversation.selected_harness,
        HarnessKind::Claude
    );
    assert!(presenter.model.conversation.model_override.is_none());
    assert_eq!(
        presenter.model.title_generation.model.as_deref(),
        Some("provider/title-model")
    );
    assert_eq!(
        presenter
            .model
            .conversation
            .title_model_catalog
            .models()
            .unwrap()[0]
            .id,
        "provider/title-model"
    );
}
