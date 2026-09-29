use super::*;

#[test]
fn catalog_lifecycle_ignores_stale_responses_and_supports_retry() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.select_harness(HarnessKind::Codex, "claude"));
    let stale_request_id = current_catalog_request_id(&presenter);

    presenter.model.log_status("当前任务状态".to_owned().into());
    assert!(presenter.refresh_model_catalog());
    assert_eq!(
        presenter
            .model()
            .latest_log_text(presenter.model().language),
        "当前任务状态"
    );
    let failed_request_id = current_catalog_request_id(&presenter);
    assert_ne!(stale_request_id, failed_request_id);
    runner.0.borrow_mut().events.push(EventEnvelope {
        protocol_version: PROTOCOL_VERSION - 1,
        id: Uuid::new_v4(),
        sequence: 1,
        event: Event::ModelCatalogLoaded {
            request_id: failed_request_id,
            harness: HarnessKind::Codex,
            models: vec![],
        },
    });
    presenter.drain_events();
    assert_eq!(current_catalog_request_id(&presenter), failed_request_id);
    assert!(
        presenter
            .model()
            .latest_log_text(presenter.model().language)
            .contains("协议版本不匹配")
    );
    let task_status = presenter
        .model()
        .latest_log_text(presenter.model().language)
        .to_owned();
    runner.emit(Event::ModelCatalogLoaded {
        request_id: stale_request_id,
        harness: HarnessKind::Codex,
        models: vec![catalog_model(
            "stale-model",
            true,
            &[ThinkingEffort::Low],
            ThinkingEffort::Low,
        )],
    });
    assert!(presenter.drain_events());
    assert!(matches!(
        presenter.model().conversation.model_catalog,
        ModelCatalogState::Loading { request_id, .. } if request_id == failed_request_id
    ));

    runner.emit(Event::ModelCatalogFailed {
        request_id: failed_request_id,
        harness: HarnessKind::Codex,
        message: "model/list unavailable".into(),
    });
    presenter.drain_events();
    assert!(matches!(
        &presenter.model().conversation.model_catalog,
        ModelCatalogState::Failed { message, .. } if message.render(Language::Chinese) == "model/list unavailable"
    ));
    assert_eq!(
        presenter
            .model()
            .latest_log_text(presenter.model().language),
        task_status
    );

    assert!(presenter.refresh_model_catalog());
    assert_eq!(
        presenter
            .model()
            .latest_log_text(presenter.model().language),
        task_status
    );
    emit_current_catalog(&presenter, &runner, Vec::new());
    presenter.drain_events();
    assert!(matches!(
        presenter.model().conversation.model_catalog,
        ModelCatalogState::Empty
    ));
    assert_eq!(
        presenter
            .model()
            .latest_log_text(presenter.model().language),
        task_status
    );

    assert!(presenter.refresh_model_catalog());
    emit_current_catalog(
        &presenter,
        &runner,
        vec![catalog_model(
            "codex-current",
            true,
            &[ThinkingEffort::Low, ThinkingEffort::High],
            ThinkingEffort::High,
        )],
    );
    presenter.drain_events();
    let ModelCatalogState::Ready(models) = &presenter.model().conversation.model_catalog else {
        panic!("expected a ready model catalog")
    };
    assert_eq!(models[0].id, "codex-current");
    assert_eq!(
        presenter
            .model()
            .latest_log_text(presenter.model().language),
        task_status
    );

    assert!(presenter.refresh_model_catalog());
    let request_id = current_catalog_request_id(&presenter);
    runner.emit(Event::HarnessDetected(HarnessProbe {
        available: false,
        ..ready_probe(HarnessKind::Codex)
    }));
    runner.emit(Event::ModelCatalogFailed {
        request_id,
        harness: HarnessKind::Codex,
        message: "late catalog failure".into(),
    });
    presenter.drain_events();
    assert!(matches!(
        presenter.model().conversation.model_catalog,
        ModelCatalogState::NotReady(_)
    ));
    runner.emit(Event::HarnessDetected(ready_probe(HarnessKind::Codex)));
    presenter.drain_events();
    assert_ne!(current_catalog_request_id(&presenter), request_id);
}

#[test]
fn catalog_preferences_are_isolated_by_harness_and_profile() {
    let (mut presenter, runner, _credentials, _directory) = provider_fixture();
    assert!(presenter.select_harness(HarnessKind::Codex, "claude"));
    let mut codex_draft = profile_draft(None, "Codex Profile", "codex-secret");
    codex_draft.model = String::new();
    let codex_profile_id = presenter.save_provider_profile(codex_draft).unwrap();
    emit_current_catalog(
        &presenter,
        &runner,
        vec![catalog_model(
            "codex-model",
            true,
            &[ThinkingEffort::High],
            ThinkingEffort::High,
        )],
    );
    presenter.drain_events();
    presenter.select_catalog_model(Some("codex-model".into()));
    presenter.select_effort(ThinkingEffort::High);
    presenter
        .save_provider_profile(profile_draft(
            None,
            "Other Codex Profile",
            "other-codex-secret",
        ))
        .unwrap();

    assert!(presenter.select_harness(HarnessKind::Omp, "codex"));
    let mut omp_draft = profile_draft(None, "OMP Profile", "omp-secret");
    omp_draft.model = String::new();
    let omp_profile_id = presenter.save_provider_profile(omp_draft).unwrap();
    emit_current_catalog(
        &presenter,
        &runner,
        vec![catalog_model_with_provider(
            Some("bigmodel"),
            "bigmodel/omp-model",
            "OMP Model",
            false,
            &[ThinkingEffort::XHigh],
            None,
        )],
    );
    presenter.drain_events();
    assert!(presenter.model().conversation.model_override.is_none());
    presenter.select_catalog_model(Some("bigmodel/omp-model".into()));
    presenter.select_effort(ThinkingEffort::XHigh);

    runner.0.borrow_mut().commands.clear();
    assert!(presenter.select_model_configuration(
        HarnessKind::Codex,
        Some(codex_profile_id),
        "omp"
    ));
    {
        let state = runner.0.borrow();
        let catalogs = state
            .commands
            .iter()
            .filter_map(|envelope| match &envelope.command {
                Command::ModelCatalogRefresh {
                    harness,
                    executable,
                    environment,
                    ..
                } => Some((harness, executable, environment)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(catalogs.len(), 1);
        let (harness, executable, environment) = catalogs[0];
        assert_eq!(*harness, HarnessKind::Codex);
        assert_eq!(executable, "codex");
        assert_eq!(environment.len(), 1);
        assert_eq!(environment[0].value, "codex-secret");
    }
    assert_eq!(
        presenter
            .model()
            .selected_provider_profile()
            .map(|profile| profile.id),
        Some(codex_profile_id)
    );
    assert_eq!(
        presenter.model().conversation.model_override.as_deref(),
        Some("codex-model")
    );
    assert_eq!(presenter.model().conversation.effort, ThinkingEffort::High);

    assert!(presenter.select_harness(HarnessKind::Omp, "codex"));
    assert_eq!(
        presenter
            .model()
            .selected_provider_profile()
            .map(|profile| profile.id),
        Some(omp_profile_id)
    );
    assert_eq!(
        presenter.model().conversation.model_override.as_deref(),
        Some("bigmodel/omp-model")
    );
    assert_eq!(presenter.model().conversation.effort, ThinkingEffort::XHigh);
}

#[test]
fn catalog_context_changes_ignore_late_responses_and_keep_missing_model_names() {
    for harness in HarnessKind::ALL {
        let (mut presenter, runner, _credentials, directory) = provider_fixture();
        presenter.select_harness(harness, "claude");
        presenter.refresh_model_catalog();
        let mut model = catalog_model(
            "chosen-id",
            false,
            &[ThinkingEffort::High],
            ThinkingEffort::High,
        );
        model.display_name = "Recognizable name".into();
        emit_current_catalog(&presenter, &runner, vec![model]);
        presenter.drain_events();
        presenter.select_catalog_model(Some("chosen-id".into()));
        presenter.probe("/new/executable");
        assert!(presenter.model().selected_catalog_model().is_none());
        let stale = current_catalog_request_id(&presenter);
        let other = directory.path().join("other-project");
        fs::create_dir(&other).unwrap();
        presenter.open_project(&other);
        let current = current_catalog_request_id(&presenter);
        assert_ne!(stale, current);
        for event in [
            Event::ModelCatalogLoaded {
                request_id: stale,
                harness,
                models: vec![],
            },
            Event::ModelCatalogFailed {
                request_id: stale,
                harness,
                message: "stale failure".into(),
            },
        ] {
            runner.emit(event);
        }
        presenter.drain_events();
        assert_eq!(current_catalog_request_id(&presenter), current);
        assert!(runner.0.borrow().commands.iter().any(|command| matches!(&command.command,
            Command::ModelCatalogRefresh { request_id, executable, cwd, .. }
            if *request_id == current && executable == "/new/executable" && Path::new(cwd) == other.canonicalize().unwrap())));
        emit_current_catalog(&presenter, &runner, vec![]);
        presenter.drain_events();
        assert_eq!(
            presenter.model().catalog_selection_is_valid(),
            harness == HarnessKind::Claude
        );
        assert_eq!(
            presenter
                .model()
                .conversation
                .model_override_name
                .as_deref(),
            Some("Recognizable name")
        );
        presenter.select_catalog_model(None);
        assert!(presenter.model().catalog_selection_is_valid());
    }

    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("first context", "claude"));
    let first = last_start(&runner);
    runner.emit(Event::RunExited {
        run_id: first.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    let previous_context = presenter.model.conversation.id;
    assert!(presenter.refresh_model_catalog());
    let previous_request = current_catalog_request_id(&presenter);
    presenter.new_task();
    let current_request = current_catalog_request_id(&presenter);
    assert_ne!(previous_request, current_request);
    runner.emit(Event::ModelCatalogLoaded {
        request_id: previous_request,
        harness: HarnessKind::Claude,
        models: vec![],
    });
    presenter.drain_events();
    assert_eq!(current_catalog_request_id(&presenter), current_request);
    assert!(
        !presenter.model.conversations[&previous_context]
            .model_catalog
            .accepts(previous_request)
    );
    emit_current_catalog(&presenter, &runner, claude_aliases());
    presenter.drain_events();
    assert_eq!(
        presenter
            .model
            .conversation
            .model_catalog
            .models()
            .unwrap()
            .len(),
        claude_aliases().len()
    );
}

#[test]
fn unavailable_models_and_unknown_efforts_cannot_change_the_requested_configuration() {
    let (mut presenter, runner, _directory) = fixture();
    presenter.select_catalog_model(Some("opus".into()));
    presenter.select_effort(ThinkingEffort::Max);
    assert_eq!(
        presenter.model().conversation.effort,
        ThinkingEffort::Default
    );
    assert!(
        presenter
            .model()
            .latest_log_text(presenter.model().language)
            .contains("不支持")
    );
    assert!(presenter.refresh_model_catalog());
    let mut unavailable = claude_aliases().remove(1);
    unavailable.availability = nexus_domain::ModelAvailability::Unavailable {
        reason: "disabled by provider".into(),
    };
    emit_current_catalog(&presenter, &runner, vec![unavailable]);
    presenter.drain_events();
    assert!(!presenter.model().can_submit());
    assert!(!presenter.submit("must not start", "claude"));
    assert_eq!(
        presenter.model().conversation.model_override.as_deref(),
        Some("opus")
    );
    presenter.select_catalog_model(None);
    assert!(presenter.model().can_submit());
    assert!(presenter.refresh_model_catalog());
    let request_id = current_catalog_request_id(&presenter);
    assert!(presenter.submit("use defaults", "claude"));
    let request = last_start(&runner);
    presenter.probe("other");
    presenter.select_effort(ThinkingEffort::High);
    runner.emit(Event::ModelCatalogLoaded {
        request_id,
        harness: HarnessKind::Claude,
        models: claude_aliases(),
    });
    presenter.drain_events();
    assert!(matches!(
        presenter.model().conversation.model_catalog,
        ModelCatalogState::Loading { .. }
    ));
    let remote = presenter.remote_state();
    assert_eq!(remote.model, request.model);
    assert_eq!(remote.effort, request.effort);
    assert_eq!(presenter.model().conversation.executable, "claude");
    let recorded = presenter
        .storage
        .conversation_config(request.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(recorded.effort, request.effort);

    runner.0.borrow_mut().commands.clear();
    runner.emit(Event::RunExited {
        run_id: request.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    assert_ne!(current_catalog_request_id(&presenter), request_id);
    assert_eq!(runner.0.borrow().commands.len(), 1);
    emit_current_catalog(&presenter, &runner, claude_aliases());
    presenter.drain_events();
    assert!(matches!(
        presenter.model().conversation.model_catalog,
        ModelCatalogState::Ready(_)
    ));
}

#[test]
fn omp_catalog_ignores_a_response_from_the_previous_profile() {
    let (mut presenter, runner, _credentials, _directory) = provider_fixture();
    assert!(presenter.select_harness(HarnessKind::Omp, "claude"));
    let first_profile = presenter
        .save_provider_profile(profile_draft(None, "First OMP", "first-secret"))
        .unwrap();
    let stale_request_id = current_catalog_request_id(&presenter);

    let mut second_draft = profile_draft(None, "Second OMP", "second-secret");
    second_draft.model = "second/profile-model".into();
    let second_profile = presenter.save_provider_profile(second_draft).unwrap();
    let current_request_id = current_catalog_request_id(&presenter);
    assert_ne!(first_profile, second_profile);
    assert_ne!(stale_request_id, current_request_id);

    runner.emit(Event::ModelCatalogLoaded {
        request_id: stale_request_id,
        harness: HarnessKind::Omp,
        models: vec![catalog_model_with_provider(
            Some("stale"),
            "stale/model",
            "Stale Model",
            false,
            &[ThinkingEffort::Low],
            None,
        )],
    });
    presenter.drain_events();
    assert!(matches!(
        presenter.model().conversation.model_catalog,
        ModelCatalogState::Loading { request_id, .. } if request_id == current_request_id
    ));

    emit_current_catalog(
        &presenter,
        &runner,
        vec![catalog_model_with_provider(
            Some("second"),
            "second/model",
            "Current Model",
            false,
            &[ThinkingEffort::Auto],
            None,
        )],
    );
    presenter.drain_events();
    let ModelCatalogState::Ready(models) = &presenter.model().conversation.model_catalog else {
        panic!("expected current catalog")
    };
    assert_eq!(models[0].id, "second/model");
}

#[test]
fn invalid_codex_catalog_selection_cannot_be_submitted() {
    let (mut presenter, runner, _directory) = fixture();
    presenter
        .storage
        .set_setting("codex_model_override_cli", "removed-model")
        .unwrap();
    assert!(presenter.select_harness(HarnessKind::Codex, "claude"));
    presenter
        .model
        .harnesses
        .insert(HarnessKind::Codex, ready_probe(HarnessKind::Codex));
    emit_current_catalog(
        &presenter,
        &runner,
        vec![catalog_model(
            "current-model",
            true,
            &[ThinkingEffort::Medium],
            ThinkingEffort::Medium,
        )],
    );
    presenter.drain_events();
    runner.0.borrow_mut().commands.clear();

    assert!(!presenter.model().can_submit());
    assert!(!presenter.submit("must not start", "codex"));
    assert!(presenter.model().conversation.active_run.is_none());
    assert!(
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
            .unwrap()
            .is_empty()
    );
    assert!(runner.0.borrow().commands.is_empty());
    assert!(
        presenter
            .model()
            .latest_log_text(presenter.model().language)
            .contains("未通过目录验证")
    );
}
