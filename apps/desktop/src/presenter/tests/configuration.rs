use super::*;

#[test]
fn startup_restores_preferences_and_probes_all_harnesses() {
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    for (key, value) in [
        ("default_harness", "codex"),
        ("claude_model", "opus"),
        ("thinking_effort", "high"),
        ("codex_effort_cli", "high"),
        ("codex_executable", "/custom/codex"),
        ("permission_mode.codex", "yolo"),
    ] {
        storage.set_setting(key, value).unwrap();
    }
    let runner = FakeRunner::default();
    let presenter = Presenter::new(storage, Ok(Box::new(runner.clone())), None);

    assert_eq!(
        presenter.model().conversation.selected_harness,
        HarnessKind::Codex
    );
    assert!(presenter.model().conversation.model_override.is_none());
    assert_eq!(presenter.model().conversation.effort, ThinkingEffort::High);
    assert_eq!(
        presenter.model().conversation.permission_mode,
        PermissionMode::Yolo
    );
    assert_eq!(presenter.model().conversation.executable, "/custom/codex");
    let state = runner.0.borrow();
    assert_eq!(state.commands.len(), HarnessKind::ALL.len() + 2);
    assert!(matches!(state.commands[0].command, Command::RunnerHello));
    assert!(state.commands.iter().any(|command| matches!(&command.command,
        Command::HarnessProbe { harness: HarnessKind::Codex, executable, .. } if executable == "/custom/codex")));
    assert!(!presenter.model().can_submit());
}

#[test]
fn switching_harnesses_restores_each_executable_and_codex_uses_default_model() {
    let (mut presenter, runner, _directory) = fixture();
    presenter.select_catalog_model(Some("opus".into()));
    presenter
        .storage
        .set_setting("codex_executable", "/custom/codex")
        .unwrap();
    assert!(presenter.select_harness(HarnessKind::Codex, "/custom/claude"));
    assert_eq!(presenter.model().conversation.executable, "/custom/codex");
    assert!(!presenter.select_harness(HarnessKind::Codex, "edited"));
    assert_eq!(
        presenter
            .storage
            .setting("claude_executable")
            .unwrap()
            .as_deref(),
        Some("/custom/claude")
    );
    presenter
        .model
        .harnesses
        .insert(HarnessKind::Codex, ready_probe(HarnessKind::Codex));
    assert!(presenter.submit("hello", "/custom/codex"));
    let state = runner.0.borrow();
    let Command::RunStart(request) = &state.commands.last().unwrap().command else {
        panic!("expected start");
    };
    assert_eq!(request.harness, HarnessKind::Codex);
    assert!(request.model.is_none());
    assert_eq!(request.effort, ThinkingEffort::Default);
    let config = presenter
        .storage
        .conversation_config(request.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(config.model, "default");
    assert_eq!(config.effort, ThinkingEffort::Default);
}

#[test]
fn all_harnesses_share_selection_priority_and_run_configuration() {
    for harness in HarnessKind::ALL {
        let (mut presenter, runner, _credentials, _directory) = provider_fixture();
        presenter.select_harness(harness, "claude");
        presenter
            .model
            .harnesses
            .insert(harness, ready_probe(harness));
        let mut draft = profile_draft(None, "Provider Profile", "profile-secret");
        draft.model = "profile-model".into();
        let profile_id = presenter.save_provider_profile(draft).unwrap();
        let models = vec![
            catalog_model(
                "cli-model",
                true,
                &[ThinkingEffort::Low],
                ThinkingEffort::Low,
            ),
            catalog_model(
                "profile-model",
                false,
                &[ThinkingEffort::Medium],
                ThinkingEffort::Medium,
            ),
            catalog_model(
                "explicit-model",
                false,
                &[ThinkingEffort::Low, ThinkingEffort::High],
                ThinkingEffort::High,
            ),
        ];
        emit_current_catalog(&presenter, &runner, models.clone());
        presenter.drain_events();
        assert_eq!(
            presenter
                .model()
                .configured_catalog_model_in(presenter.model().conversation.id),
            Some("profile-model")
        );

        presenter.select_catalog_model(Some("explicit-model".into()));
        assert_eq!(
            presenter
                .model()
                .configured_catalog_model_in(presenter.model().conversation.id),
            Some("explicit-model")
        );
        assert_eq!(
            presenter
                .storage
                .setting(&catalog_model_setting_key(harness, Some(profile_id),))
                .unwrap()
                .as_deref(),
            Some("explicit-model")
        );

        presenter.select_catalog_model(None);
        assert_eq!(
            presenter
                .model()
                .configured_catalog_model_in(presenter.model().conversation.id),
            Some("profile-model")
        );
        assert_eq!(
            presenter
                .storage
                .setting(&catalog_model_setting_key(harness, Some(profile_id),))
                .unwrap()
                .as_deref(),
            Some("")
        );

        presenter.select_catalog_model(Some("explicit-model".into()));
        assert!(presenter.model().can_submit());
        assert!(presenter.submit("use catalog selection", harness.default_executable()));
        let resolved = presenter.model().resolved_model_selection();
        let remote = presenter.remote_state();
        assert_eq!(remote.model, resolved.model);
        assert_eq!(remote.effort, resolved.effort);
        assert!(
            !serde_json::to_string(&remote)
                .unwrap()
                .contains("profile-secret")
        );
        assert_eq!(
            presenter
                .model()
                .selected_provider_profile()
                .unwrap()
                .model
                .as_deref(),
            Some("profile-model")
        );
        let mut request = last_start(&runner);
        assert_eq!(request.harness, harness);
        assert_eq!(request.model.as_deref(), Some("explicit-model"));
        assert_eq!(request.effort, ThinkingEffort::High);
        let config = presenter
            .storage
            .conversation_config(request.task_id)
            .unwrap()
            .unwrap();
        assert_eq!(config.model, "explicit-model");
        assert_eq!(config.effort, ThinkingEffort::High);

        for (profile, expected_model, expected_effort) in [
            (
                Some(profile_id),
                Some("profile-model"),
                ThinkingEffort::Medium,
            ),
            (None, None, ThinkingEffort::Default),
        ] {
            runner.emit(Event::RunExited {
                run_id: request.run_id,
                status: RunStatus::Completed,
                exit_code: Some(0),
            });
            presenter.drain_events();
            presenter.new_task();
            presenter.select_catalog_model(None);
            if profile.is_none() {
                presenter.select_provider_profile(None);
                emit_current_catalog(&presenter, &runner, models.clone());
                presenter.drain_events();
            }
            assert_eq!(
                presenter
                    .model()
                    .configured_catalog_model_in(presenter.model().conversation.id),
                expected_model
            );
            assert!(presenter.submit("follow default", harness.default_executable()));
            request = last_start(&runner);
            assert_eq!(request.model.as_deref(), expected_model);
            assert_eq!(request.effort, expected_effort);
            let remote = presenter.remote_state();
            assert_eq!(remote.model, request.model);
            assert_eq!(remote.effort, request.effort);
            let config = presenter
                .storage
                .conversation_config(request.task_id)
                .unwrap()
                .unwrap();
            assert_eq!(config.model, expected_model.unwrap_or("default"));
            assert_eq!(config.effort, request.effort);
        }
    }
}

#[test]
fn codex_preferences_are_isolated_per_profile_and_invalid_effort_resets() {
    let (mut presenter, runner, _credentials, _directory) = provider_fixture();
    assert!(presenter.select_harness(HarnessKind::Codex, "claude"));
    let models = vec![
        catalog_model(
            "model-alpha",
            true,
            &[ThinkingEffort::High],
            ThinkingEffort::High,
        ),
        catalog_model(
            "model-beta",
            false,
            &[ThinkingEffort::Low],
            ThinkingEffort::Low,
        ),
    ];

    let mut first_draft = profile_draft(None, "First Codex", "first-secret");
    first_draft.model = "profile-first".into();
    let first_profile_id = presenter.save_provider_profile(first_draft).unwrap();
    emit_current_catalog(&presenter, &runner, models.clone());
    presenter.drain_events();
    presenter.select_catalog_model(Some("model-alpha".into()));
    presenter.select_effort(ThinkingEffort::High);

    assert!(presenter.refresh_model_catalog());
    runner.emit(Event::ModelCatalogFailed {
        request_id: current_catalog_request_id(&presenter),
        harness: HarnessKind::Codex,
        message: "temporary catalog failure".into(),
    });
    presenter.drain_events();
    assert_eq!(presenter.model().conversation.effort, ThinkingEffort::High);
    assert_eq!(
        presenter.model().resolved_model_selection().effort,
        ThinkingEffort::High
    );
    assert_eq!(
        presenter
            .storage
            .setting(&catalog_effort_setting_key(
                HarnessKind::Codex,
                Some(first_profile_id),
            ))
            .unwrap()
            .as_deref(),
        Some("high")
    );
    assert!(presenter.refresh_model_catalog());
    emit_current_catalog(&presenter, &runner, models.clone());
    presenter.drain_events();
    assert_eq!(presenter.model().conversation.effort, ThinkingEffort::High);
    assert_eq!(
        presenter.model().resolved_model_selection().effort,
        ThinkingEffort::High
    );

    let mut second_draft = profile_draft(None, "Second Codex", "second-secret");
    second_draft.model = "profile-second".into();
    let second_profile_id = presenter.save_provider_profile(second_draft).unwrap();
    emit_current_catalog(&presenter, &runner, models.clone());
    presenter.drain_events();
    assert!(presenter.model().conversation.model_override.is_none());
    assert_eq!(
        presenter.model().conversation.effort,
        ThinkingEffort::Default
    );
    presenter.select_catalog_model(Some("model-beta".into()));
    presenter.select_effort(ThinkingEffort::Low);

    assert!(presenter.select_provider_profile(Some(first_profile_id)));
    assert!(presenter.model().selected_catalog_model().is_none());
    assert_eq!(
        presenter.model().conversation.model_override.as_deref(),
        Some("model-alpha")
    );
    assert_eq!(presenter.model().conversation.effort, ThinkingEffort::High);
    emit_current_catalog(&presenter, &runner, models);
    presenter.drain_events();
    assert_eq!(presenter.model().conversation.effort, ThinkingEffort::High);

    assert!(presenter.select_provider_profile(Some(second_profile_id)));
    assert_eq!(
        presenter.model().conversation.model_override.as_deref(),
        Some("model-beta")
    );
    assert_eq!(presenter.model().conversation.effort, ThinkingEffort::Low);
    emit_current_catalog(
        &presenter,
        &runner,
        vec![catalog_model(
            "model-beta",
            true,
            &[ThinkingEffort::High],
            ThinkingEffort::High,
        )],
    );
    presenter.drain_events();
    assert_eq!(
        presenter.model().conversation.effort,
        ThinkingEffort::Default
    );
    assert_eq!(
        presenter
            .storage
            .setting(&catalog_effort_setting_key(
                HarnessKind::Codex,
                Some(second_profile_id),
            ))
            .unwrap()
            .as_deref(),
        Some("default")
    );
    assert!(
        presenter
            .model()
            .latest_log_text(presenter.model().language)
            .contains("恢复为模型默认")
    );
}

#[test]
fn omp_selection_preserves_full_selector_and_default_priority_in_run_configuration() {
    let (mut presenter, runner, _credentials, _directory) = provider_fixture();
    assert!(presenter.select_harness(HarnessKind::Omp, "claude"));
    presenter
        .model
        .harnesses
        .insert(HarnessKind::Omp, ready_probe(HarnessKind::Omp));
    let mut draft = profile_draft(None, "OMP Profile", "profile-secret");
    draft.model = "openai/shared-model".into();
    presenter.save_provider_profile(draft).unwrap();
    let models = vec![
        catalog_model_with_provider(
            Some("openai"),
            "openai/shared-model",
            "Shared Model",
            false,
            &[ThinkingEffort::Low],
            None,
        ),
        catalog_model_with_provider(
            Some("bigmodel"),
            "bigmodel/shared-model",
            "Shared Model",
            false,
            &[
                ThinkingEffort::Off,
                ThinkingEffort::Minimal,
                ThinkingEffort::XHigh,
                ThinkingEffort::Auto,
            ],
            None,
        ),
    ];
    emit_current_catalog(&presenter, &runner, models.clone());
    presenter.drain_events();

    assert_eq!(
        presenter
            .model()
            .configured_catalog_model_in(presenter.model().conversation.id),
        Some("openai/shared-model")
    );
    presenter.select_catalog_model(Some("bigmodel/shared-model".into()));
    assert_eq!(
        presenter
            .model()
            .selected_catalog_model()
            .and_then(|model| model.provider.as_deref()),
        Some("bigmodel")
    );
    presenter.select_effort(ThinkingEffort::XHigh);
    assert!(presenter.submit("use explicit OMP model", "omp"));
    let explicit = last_start(&runner);
    assert_eq!(explicit.model.as_deref(), Some("bigmodel/shared-model"));
    assert_eq!(explicit.effort, ThinkingEffort::XHigh);
    let config = presenter
        .storage
        .conversation_config(explicit.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(config.model, "bigmodel/shared-model");
    assert_eq!(config.effort, ThinkingEffort::XHigh);

    runner.emit(Event::RunExited {
        run_id: explicit.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    presenter.new_task();
    presenter.select_catalog_model(None);
    assert!(presenter.submit("use Profile default", "omp"));
    let profile_default = last_start(&runner);
    assert_eq!(
        profile_default.model.as_deref(),
        Some("openai/shared-model")
    );
    assert_eq!(profile_default.effort, ThinkingEffort::Default);

    runner.emit(Event::RunExited {
        run_id: profile_default.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    presenter.new_task();
    assert!(presenter.select_provider_profile(None));
    emit_current_catalog(&presenter, &runner, models);
    presenter.drain_events();
    assert!(presenter.submit("use CLI default", "omp"));
    let cli_default = last_start(&runner);
    assert!(cli_default.model.is_none());
    let config = presenter
        .storage
        .conversation_config(cli_default.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(config.model, "default");
}

#[test]
fn all_harnesses_restore_each_profile_and_cli_selection_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("preferences.db");
    let credentials = FakeCredentialStore::default();
    let runner = FakeRunner::default();
    let mut presenter = Presenter::new_with_credentials(
        Storage::open(&database).unwrap(),
        Ok(Box::new(runner.clone())),
        None,
        Box::new(credentials.clone()),
    );
    presenter.open_project(directory.path());
    let mut expected = Vec::new();
    for harness in HarnessKind::ALL {
        let executable = presenter.model().conversation.executable.clone();
        presenter.select_harness(harness, &executable);
        for (index, name) in ["cli", "first", "second"].into_iter().enumerate() {
            let profile = if index == 0 {
                presenter.select_provider_profile(None);
                None
            } else {
                Some(
                    presenter
                        .save_provider_profile(profile_draft(None, name, "profile-secret"))
                        .unwrap(),
                )
            };
            let id = format!("{harness}/{name}/model");
            let mut model = catalog_model(
                &id,
                false,
                &[ThinkingEffort::Low, ThinkingEffort::High],
                ThinkingEffort::High,
            );
            model.display_name = format!("{name} model");
            emit_current_catalog(&presenter, &runner, vec![model.clone()]);
            presenter.drain_events();
            presenter.select_catalog_model(Some(id.clone()));
            let effort = if index == 1 {
                ThinkingEffort::High
            } else {
                ThinkingEffort::Low
            };
            presenter.select_effort(effort);
            expected.push((harness, profile, model, effort));
        }
    }
    drop(presenter);
    let mut presenter = Presenter::new_with_credentials(
        Storage::open(&database).unwrap(),
        Ok(Box::new(runner.clone())),
        None,
        Box::new(credentials),
    );
    presenter.open_project(directory.path());
    for (harness, profile, model, effort) in expected.into_iter().rev() {
        let executable = presenter.model().conversation.executable.clone();
        presenter.select_harness(harness, &executable);
        presenter.select_provider_profile(profile);
        assert_eq!(
            presenter.model().conversation.model_override.as_deref(),
            Some(model.id.as_str())
        );
        assert_eq!(
            presenter
                .model()
                .conversation
                .model_override_name
                .as_deref(),
            Some(model.display_name.as_str())
        );
        assert_eq!(presenter.model().conversation.effort, effort);
        emit_current_catalog(&presenter, &runner, vec![model.clone()]);
        presenter.drain_events();
        assert_eq!(
            presenter.model().resolved_model_selection().model,
            Some(model.id)
        );
        assert_eq!(presenter.model().resolved_model_selection().effort, effort);
    }
}

#[test]
fn claude_custom_models_are_restored_per_configuration_and_can_start_runs() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("preferences.db");
    let credentials = FakeCredentialStore::default();
    let runner = FakeRunner::default();
    let mut presenter = Presenter::new_with_credentials(
        Storage::open(&database).unwrap(),
        Ok(Box::new(runner.clone())),
        None,
        Box::new(credentials.clone()),
    );
    presenter.open_project(directory.path());
    let mut expected = Vec::new();
    for (name, model_id) in [
        ("cli", "GLM-5"),
        ("Kimi", "moonshotai/Kimi-K2.5"),
        ("GLM", "z-ai/glm-5"),
    ] {
        let profile = (name != "cli").then(|| {
            presenter
                .save_provider_profile(profile_draft(None, name, "profile-secret"))
                .unwrap()
        });
        presenter.select_catalog_model(Some(model_id.into()));
        assert_eq!(
            presenter.model().conversation.model_override.as_deref(),
            Some(model_id)
        );
        expected.push((profile, model_id));
    }
    drop(presenter);

    let mut presenter = Presenter::new_with_credentials(
        Storage::open(&database).unwrap(),
        Ok(Box::new(runner.clone())),
        None,
        Box::new(credentials),
    );
    presenter.open_project(directory.path());
    presenter
        .model
        .harnesses
        .insert(HarnessKind::Claude, ready_probe(HarnessKind::Claude));
    for (profile, model_id) in expected.into_iter().rev() {
        presenter.select_provider_profile(profile);
        assert_eq!(
            presenter.model().conversation.model_override.as_deref(),
            Some(model_id)
        );
        assert!(presenter.model().can_submit());
        emit_current_catalog(&presenter, &runner, claude_aliases());
        presenter.drain_events();
        assert!(presenter.model().selected_catalog_model().is_none());
        assert!(presenter.model().can_submit());
        assert_eq!(presenter.remote_state().model.as_deref(), Some(model_id));
    }

    presenter.select_effort(ThinkingEffort::High);
    assert_eq!(
        presenter.model().conversation.effort,
        ThinkingEffort::Default
    );
    for models in [vec![], claude_aliases()] {
        assert!(presenter.refresh_model_catalog());
        assert!(presenter.model().can_submit());
        emit_current_catalog(&presenter, &runner, models);
        presenter.drain_events();
        assert!(presenter.model().can_submit());
    }
    assert!(presenter.refresh_model_catalog());
    runner.emit(Event::ModelCatalogFailed {
        request_id: current_catalog_request_id(&presenter),
        harness: HarnessKind::Claude,
        message: "catalog unavailable".into(),
    });
    presenter.drain_events();
    assert!(presenter.model().can_submit());
    assert!(presenter.submit("use custom model", "claude"));
    let request = last_start(&runner);
    assert_eq!(request.model.as_deref(), Some("GLM-5"));
    assert_eq!(request.effort, ThinkingEffort::Default);
    let config = presenter
        .storage
        .conversation_config(request.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(config.model, "GLM-5");
    presenter.select_catalog_model(Some("another-model".into()));
    assert_eq!(
        presenter.model().conversation.model_override.as_deref(),
        Some("GLM-5")
    );
}

#[test]
fn custom_model_selection_is_claude_only_and_generation_uses_the_same_rules() {
    for harness in HarnessKind::ALL {
        let (mut presenter, _runner, _directory) = fixture();
        presenter.select_harness(harness, "claude");
        presenter.select_catalog_model(Some("moonshotai/Kimi-K2.5".into()));
        assert_eq!(
            presenter.model().conversation.model_override.is_some(),
            harness == HarnessKind::Claude
        );
        for invalid in ["", "  ", "model\0id", "model\nid"] {
            let previous = presenter.model().conversation.model_override.clone();
            presenter.select_catalog_model(Some(invalid.into()));
            assert_eq!(presenter.model().conversation.model_override, previous);
        }
        for kind in GenerationKind::ALL {
            presenter.select_generation_harness(kind, harness);
            assert_eq!(
                presenter.select_generation_model(kind, Some("GLM-5".into())),
                harness == HarnessKind::Claude
            );
            if harness == HarnessKind::Claude {
                assert!(!presenter.select_generation_effort(kind, ThinkingEffort::High));
                let config = presenter.generation_configuration(kind).unwrap();
                assert_eq!(config.model.as_deref(), Some("GLM-5"));
                assert_eq!(config.effort, ThinkingEffort::Default);
                let saved: GenerationSettings = serde_json::from_str(
                    &presenter
                        .storage
                        .setting(kind.setting_key())
                        .unwrap()
                        .unwrap(),
                )
                .unwrap();
                assert_eq!(saved.model, config.model);
            }
            for invalid in ["", "  ", "model\0id", "model\nid"] {
                assert!(!presenter.select_generation_model(kind, Some(invalid.into())));
            }
        }
    }
}

#[test]
fn claude_legacy_preferences_migrate_once_without_leaking_to_profiles() {
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    storage.set_setting("claude_model", "opus").unwrap();
    let (model, effort) =
        load_catalog_preferences(&storage, HarnessKind::Claude, None, ThinkingEffort::High);
    assert_eq!(model.as_deref(), Some("opus"));
    assert_eq!(effort, ThinkingEffort::High);
    let (model, effort) = load_catalog_preferences(
        &storage,
        HarnessKind::Claude,
        Some(Uuid::new_v4()),
        ThinkingEffort::High,
    );
    assert!(model.is_none());
    assert_eq!(effort, ThinkingEffort::Default);
    storage
        .set_setting("claude_model_override_cli", "")
        .unwrap();
    assert!(
        load_catalog_preferences(&storage, HarnessKind::Claude, None, ThinkingEffort::High)
            .0
            .is_none()
    );
}

#[test]
fn omp_legacy_efforts_migrate_to_their_actual_cli_values() {
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    storage.set_setting("omp_effort_cli", "max").unwrap();
    let (_, effort) =
        load_catalog_preferences(&storage, HarnessKind::Omp, None, ThinkingEffort::Default);
    assert_eq!(effort, ThinkingEffort::XHigh);
    assert_eq!(
        storage.setting("omp_effort_cli").unwrap().as_deref(),
        Some("xhigh")
    );

    let storage = Storage::open(Path::new(":memory:")).unwrap();
    let (_, effort) =
        load_catalog_preferences(&storage, HarnessKind::Omp, None, ThinkingEffort::None);
    assert_eq!(effort, ThinkingEffort::Off);
    assert_eq!(
        storage.setting("omp_effort_cli").unwrap().as_deref(),
        Some("off")
    );
}

#[test]
fn omp_profile_custom_model_is_preserved_when_catalog_availability_is_unknown() {
    let (mut presenter, runner, _credentials, _directory) = provider_fixture();
    assert!(presenter.select_harness(HarnessKind::Omp, "claude"));
    presenter.model.harnesses.insert(
        HarnessKind::Omp,
        HarnessProbe {
            authenticated: false,
            ..ready_probe(HarnessKind::Omp)
        },
    );
    let mut draft = profile_draft(None, "Custom OMP", "custom-secret");
    draft.model = "private-provider/custom-model".into();
    presenter.save_provider_profile(draft).unwrap();
    emit_current_catalog(
        &presenter,
        &runner,
        vec![catalog_model_with_provider(
            Some("public-provider"),
            "public-provider/custom-model",
            "Custom Model",
            false,
            &[ThinkingEffort::Medium],
            None,
        )],
    );
    presenter.drain_events();

    assert!(presenter.model().conversation.model_override.is_none());
    assert_eq!(
        presenter
            .model()
            .configured_catalog_model_in(presenter.model().conversation.id),
        Some("private-provider/custom-model")
    );
    assert!(presenter.model().selected_catalog_model().is_none());
    assert!(presenter.model().can_submit());
    assert!(
        presenter
            .model()
            .latest_log_text(presenter.model().language)
            .contains("尚未验证可用")
    );
}

#[test]
fn native_transport_and_codebuddy_region_persist_and_reach_launch_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("preferences.sqlite");
    let runner = FakeRunner::default();
    let mut presenter = Presenter::new(
        Storage::open(&database).unwrap(),
        Ok(Box::new(runner.clone())),
        None,
    );
    for harness in [
        HarnessKind::Kimi,
        HarnessKind::Qoder,
        HarnessKind::QoderCn,
        HarnessKind::Codebuddy,
    ] {
        presenter.model.conversation.selected_harness = harness;
        // Kimi Code is ACP-only, so its default never falls back to the CLI.
        assert_eq!(
            presenter.harness_transport(harness),
            if harness == HarnessKind::Kimi {
                nexus_domain::HarnessTransport::Acp
            } else {
                nexus_domain::HarnessTransport::Cli
            }
        );
        presenter.set_harness_transport(nexus_domain::HarnessTransport::Acp);
        assert_eq!(
            presenter.harness_transport(harness),
            nexus_domain::HarnessTransport::Acp
        );
    }
    presenter.set_codebuddy_region("external");
    let environment = presenter
        .provider_launch_configuration_in(presenter.model.conversation.id, HarnessKind::Codebuddy)
        .unwrap();
    assert_eq!(environment.len(), 1);
    assert_eq!(environment[0].name, "CODEBUDDY_INTERNET_ENVIRONMENT");
    assert_eq!(environment[0].value, "external");
    assert!(
        presenter
            .provider_launch_configuration_in(presenter.model.conversation.id, HarnessKind::QoderCn)
            .unwrap()
            .is_empty()
    );
    assert!(runner.0.borrow().commands.iter().any(|command| matches!(&command.command,
        Command::HarnessProbe {harness:HarnessKind::Codebuddy,environment,..} if environment.iter().any(|v|v.value=="external"))));
    drop(presenter);
    let mut presenter = Presenter::new(
        Storage::open(&database).unwrap(),
        Ok(Box::new(runner)),
        None,
    );
    assert_eq!(presenter.codebuddy_region(), "external");
    assert_eq!(
        presenter.harness_transport(HarnessKind::QoderCn),
        nexus_domain::HarnessTransport::Acp
    );
    presenter.set_codebuddy_region("invalid");
    assert_eq!(presenter.codebuddy_region(), "external");
    presenter.set_codebuddy_region("");
    assert!(
        presenter
            .provider_launch_configuration_in(
                presenter.model.conversation.id,
                HarnessKind::Codebuddy
            )
            .unwrap()
            .is_empty()
    );
}
