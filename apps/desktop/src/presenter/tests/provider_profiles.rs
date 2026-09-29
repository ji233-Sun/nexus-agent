use super::*;

#[test]
fn startup_reads_profile_credential_state_from_the_system_store() {
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    let configured_id = Uuid::new_v4();
    let missing_id = Uuid::new_v4();
    storage
        .set_provider_profiles(&[
            ProviderProfile {
                id: configured_id,
                name: "Configured".into(),
                harness: HarnessKind::Codex,
                api_key_env: "CODEX_API_KEY".into(),
                base_url_env: None,
                base_url: None,
                model: None,
                credential_configured: false,
            },
            ProviderProfile {
                id: missing_id,
                name: "Missing".into(),
                harness: HarnessKind::Codex,
                api_key_env: "CODEX_API_KEY".into(),
                base_url_env: None,
                base_url: None,
                model: None,
                credential_configured: true,
            },
        ])
        .unwrap();
    let credentials = FakeCredentialStore::default();
    credentials
        .0
        .borrow_mut()
        .insert(configured_id, "stored-key".into());

    let presenter = Presenter::new_with_credentials(
        storage,
        Err(anyhow::anyhow!("runner unavailable")),
        None,
        Box::new(credentials),
    );

    assert!(
        presenter
            .model()
            .provider_profiles
            .iter()
            .find(|profile| profile.id == configured_id)
            .unwrap()
            .credential_configured
    );
    assert!(
        !presenter
            .model()
            .provider_profiles
            .iter()
            .find(|profile| profile.id == missing_id)
            .unwrap()
            .credential_configured
    );
}

#[test]
fn provider_profile_keeps_secret_out_of_storage_and_injects_only_the_selected_run() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    let runner = FakeRunner::default();
    let credentials = FakeCredentialStore::default();
    let mut presenter = Presenter::new_with_credentials(
        storage,
        Ok(Box::new(runner.clone())),
        None,
        Box::new(credentials.clone()),
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
    runner.0.borrow_mut().commands.clear();

    let profile_id = presenter
        .save_provider_profile(profile_draft(None, "DeepSeek", "super-secret"))
        .unwrap();

    assert!(presenter.model().can_submit());
    assert_eq!(
        credentials.0.borrow().get(&profile_id).map(String::as_str),
        Some("super-secret")
    );
    assert!(
        !presenter
            .storage
            .setting("provider_profiles")
            .unwrap()
            .unwrap()
            .contains("super-secret")
    );
    assert!(presenter.submit("use the selected provider", "omp"));
    let state = runner.0.borrow();
    assert!(matches!(
        &state.commands[0].command,
        Command::ModelCatalogRefresh { .. }
    ));
    let request = state
        .commands
        .iter()
        .find_map(|envelope| match &envelope.command {
            Command::RunStart(request) => Some(request),
            _ => None,
        })
        .expect("expected start");
    assert_eq!(request.harness, HarnessKind::Omp);
    assert_eq!(request.model.as_deref(), Some("deepseek/deepseek-v4-pro"));
    assert_eq!(request.environment.len(), 1);
    assert_eq!(request.environment[0].name, "DEEPSEEK_API_KEY");
    assert_eq!(request.environment[0].value, "super-secret");
    assert_eq!(
        format!("{:?}", request.environment[0]),
        r#"EnvironmentVariable { name: "DEEPSEEK_API_KEY", value: "[REDACTED]" }"#
    );
}

#[test]
fn provider_profiles_can_be_updated_switched_and_deleted() {
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    let runner = FakeRunner::default();
    let credentials = FakeCredentialStore::default();
    let mut presenter = Presenter::new_with_credentials(
        storage,
        Ok(Box::new(runner)),
        None,
        Box::new(credentials.clone()),
    );
    assert!(presenter.select_harness(HarnessKind::Omp, "claude"));
    let profile_id = presenter
        .save_provider_profile(profile_draft(None, "DeepSeek", "first-key"))
        .unwrap();
    assert_eq!(
        presenter.model().selected_provider_profile().unwrap().id,
        profile_id
    );

    assert_eq!(
        presenter.save_provider_profile(profile_draft(Some(profile_id), "DeepSeek Production", "")),
        Some(profile_id)
    );
    assert_eq!(
        credentials.0.borrow().get(&profile_id).map(String::as_str),
        Some("first-key")
    );
    assert!(presenter.select_provider_profile(None));
    assert!(presenter.model().selected_provider_profile().is_none());
    credentials.0.borrow_mut().remove(&profile_id);
    assert!(presenter.select_provider_profile(Some(profile_id)));
    assert!(
        !presenter
            .model()
            .selected_provider_profile()
            .unwrap()
            .credential_configured
    );
    assert!(
        presenter
            .model()
            .latest_log_text(presenter.model().language)
            .contains("没有 API Key")
    );
    assert!(presenter.delete_provider_profile(profile_id));
    assert!(presenter.model().provider_profiles.is_empty());
    assert!(!credentials.0.borrow().contains_key(&profile_id));
}

#[test]
fn provider_profiles_reject_process_control_environment_variables() {
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    let mut presenter = Presenter::new_with_credentials(
        storage,
        Err(anyhow::anyhow!("runner unavailable")),
        None,
        Box::new(FakeCredentialStore::default()),
    );
    let mut draft = profile_draft(None, "Unsafe", "secret");
    draft.api_key_env = "LD_PRELOAD".into();

    assert!(presenter.save_provider_profile(draft).is_none());
    assert!(presenter.model().provider_profiles.is_empty());
    assert!(
        presenter
            .model()
            .latest_log_text(presenter.model().language)
            .contains("*_API_KEY")
    );
}

#[test]
fn provider_profiles_bound_visible_name_and_model_lengths() {
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    let mut presenter = Presenter::new_with_credentials(
        storage,
        Err(anyhow::anyhow!("runner unavailable")),
        None,
        Box::new(FakeCredentialStore::default()),
    );
    let mut draft = profile_draft(None, &"n".repeat(49), "secret");

    assert!(presenter.save_provider_profile(draft).is_none());
    assert!(
        presenter
            .model()
            .latest_log_text(presenter.model().language)
            .contains("48")
    );

    draft = profile_draft(None, "Valid name", "secret");
    draft.model = "m".repeat(129);
    assert!(presenter.save_provider_profile(draft).is_none());
    assert!(
        presenter
            .model()
            .latest_log_text(presenter.model().language)
            .contains("128")
    );
    assert!(presenter.model().provider_profiles.is_empty());
}
