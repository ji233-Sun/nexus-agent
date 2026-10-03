use super::*;

#[test]
fn language_preferences_restore_and_fall_back_without_changing_appearance() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("language.sqlite");
    let mut presenter = Presenter::new(
        Storage::open(&path).unwrap(),
        Err(anyhow::anyhow!("test")),
        None,
    );
    assert_eq!(presenter.model().language, Language::Chinese);
    let appearance = AppearanceSettings {
        glass: false,
        ..Default::default()
    };
    assert!(presenter.set_appearance(appearance));
    for language in [Language::English, Language::Chinese] {
        assert!(presenter.set_language(language));
        assert_eq!(
            presenter.storage.setting("language").unwrap().as_deref(),
            Some(language.as_str())
        );
        drop(presenter);
        presenter = Presenter::new(
            Storage::open(&path).unwrap(),
            Err(anyhow::anyhow!("test")),
            None,
        );
        assert_eq!(presenter.model().language, language);
        assert_eq!(presenter.model().appearance, appearance);
    }
    for invalid in ["", "fr", "invalid json"] {
        presenter.storage.set_setting("language", invalid).unwrap();
        drop(presenter);
        presenter = Presenter::new(
            Storage::open(&path).unwrap(),
            Err(anyhow::anyhow!("test")),
            None,
        );
        assert_eq!(presenter.model().language, Language::Chinese);
    }
}

#[test]
fn runtime_log_is_ordered_localized_and_limited_to_the_current_launch() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("logs-test.sqlite3");
    let runner = FakeRunner::default();
    let mut presenter = Presenter::new_with_credentials(
        Storage::open(&path).unwrap(),
        Ok(Box::new(runner.clone())),
        None,
        Box::new(FakeCredentialStore::default()),
    );
    assert_eq!(presenter.model.runtime_log.len(), 1);
    assert_eq!(
        presenter.model.latest_log_text(Language::Chinese),
        "正在连接本地 Runner…"
    );
    presenter.issues_client = crate::infrastructure::issues::Client::fake();
    presenter.open_project(directory.path());
    let before = chrono::Local::now();
    presenter.new_task();
    assert!(presenter.delete_archived_tasks());
    let after = chrono::Local::now();
    assert_eq!(presenter.model.runtime_log.len(), 3);
    for entry in &presenter.model.runtime_log[1..] {
        assert!(entry.timestamp >= before && entry.timestamp <= after);
    }
    assert_eq!(
        presenter.model.runtime_log[1]
            .message
            .render(Language::Chinese),
        "已准备好新任务。"
    );
    assert_eq!(
        presenter.model.latest_log_text(Language::Chinese),
        "已删除 0 个归档对话。"
    );
    assert!(presenter.set_language(Language::English));
    assert_eq!(presenter.model.runtime_log.len(), 3);
    assert_eq!(
        presenter.model.latest_log_text(Language::English),
        "Deleted 0 archived conversations."
    );
    drop(presenter);
    let presenter = Presenter::new_with_credentials(
        Storage::open(&path).unwrap(),
        Ok(Box::new(runner)),
        Some("storage startup diagnostic".into()),
        Box::new(FakeCredentialStore::default()),
    );
    assert_eq!(presenter.model.runtime_log.len(), 1);
    assert_eq!(
        presenter.model.latest_log_text(Language::English),
        "storage startup diagnostic"
    );
}

#[test]
fn runtime_log_records_operations_and_background_events_without_replacing_run_progress() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("first task", "claude"));
    let first = last_start(&runner);
    runner.emit(Event::RunStatusChanged {
        run_id: first.run_id,
        status: RunStatus::Running,
        message: Some("thinking".into()),
    });
    presenter.drain_events();
    assert!(presenter.submit("queued message", "claude"));
    assert!(
        presenter
            .model
            .latest_log_text(Language::Chinese)
            .contains("消息已排队")
    );
    assert_eq!(
        presenter
            .model
            .conversation
            .run_status
            .render(Language::Chinese),
        "thinking"
    );
    runner.emit(Event::HarnessDetected(ready_probe(HarnessKind::Claude)));
    presenter.drain_events();
    assert_eq!(
        presenter
            .model
            .conversation
            .run_status
            .render(Language::Chinese),
        "thinking"
    );
    assert_eq!(remote_status(&mut presenter), "thinking");

    presenter.new_task();
    assert_eq!(
        presenter.model.conversation.run_status,
        LocalizedText::default()
    );
    let other_directory = tempfile::tempdir().unwrap();
    presenter.open_project(other_directory.path());
    assert!(presenter.submit("second task", "claude"));
    let second = last_start(&runner);
    runner.emit(Event::RunStatusChanged {
        run_id: second.run_id,
        status: RunStatus::Running,
        message: Some("reading files".into()),
    });
    runner.emit(Event::RunStatusChanged {
        run_id: first.run_id,
        status: RunStatus::Running,
        message: Some("testing".into()),
    });
    presenter.drain_events();
    assert_eq!(
        presenter
            .model
            .conversation
            .run_status
            .render(Language::Chinese),
        "reading files"
    );
    assert_eq!(
        presenter.model.latest_log_text(Language::Chinese),
        "testing"
    );
    assert_eq!(remote_status(&mut presenter), "reading files");
    presenter.select_task(first.task_id);
    assert_eq!(
        presenter
            .model
            .conversation
            .run_status
            .render(Language::Chinese),
        "testing"
    );
    runner.emit(Event::RunExited {
        run_id: second.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    assert_eq!(
        presenter.model.latest_log_text(Language::Chinese),
        "任务已完成"
    );
    assert_eq!(
        presenter
            .model
            .conversation
            .run_status
            .render(Language::Chinese),
        "testing"
    );
    assert_eq!(remote_status(&mut presenter), "testing");
    let count = presenter.model.runtime_log.len();
    runner.emit(Event::RunStatusChanged {
        run_id: second.run_id,
        status: RunStatus::Running,
        message: Some("stale progress".into()),
    });
    presenter.drain_events();
    assert_eq!(presenter.model.runtime_log.len(), count);
    presenter.select_task(second.task_id);
    assert_eq!(
        presenter
            .model
            .conversation
            .run_status
            .render(Language::Chinese),
        "任务已完成"
    );
}

#[test]
fn language_switch_updates_status_and_preserves_active_runs_and_remote_content() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(!presenter.submit(" ", "claude"));
    assert!(presenter.set_language(Language::English));
    assert_eq!(
        presenter
            .model()
            .latest_log_text(presenter.model().language),
        "Prompt cannot be empty."
    );
    assert_eq!(
        presenter.model.latest_log_text(Language::Chinese),
        "Prompt 不能为空。"
    );
    assert_eq!(remote_status(&mut presenter), "");
    assert!(presenter.submit("设置 {count} café", "claude"));
    let task_id = presenter.model().conversation.selected_task;
    let run_id = presenter.model().conversation.active_run;
    assert!(presenter.submit("待发送 {error}", "claude"));
    let command_count = runner.0.borrow().commands.len();
    let status_before = remote_status(&mut presenter);
    for language in [Language::Chinese, Language::English] {
        assert!(presenter.set_language(language));
        assert_eq!(presenter.model().conversation.selected_task, task_id);
        assert_eq!(presenter.model().conversation.active_run, run_id);
        assert_eq!(
            presenter.model().conversation.messages[0].content,
            "设置 {count} café"
        );
        assert_eq!(
            presenter.model().conversation.queued_messages[0].prompt,
            "待发送 {error}"
        );
        assert_eq!(
            presenter.model().conversation.tasks[0].title,
            "设置 {count} café"
        );
        assert_eq!(remote_status(&mut presenter), status_before);
        assert_eq!(runner.0.borrow().commands.len(), command_count);
    }
    runner.emit(Event::RunFailed {
        run_id: run_id.unwrap(),
        code: ErrorCode::UnexpectedExit,
        message: "原始诊断 {message}".into(),
    });
    presenter.drain_events();
    assert_eq!(
        presenter
            .model()
            .latest_log_text(presenter.model().language),
        "原始诊断 {message}"
    );
}

#[test]
fn language_translates_probe_summaries_and_default_effort_without_changing_cli_values() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.set_language(Language::English));
    for (available, authenticated, expected) in [
        (true, true, "Claude Code is ready."),
        (
            true,
            false,
            "Claude Code is not signed in. Sign in to the CLI or configure a provider profile.",
        ),
        (
            false,
            false,
            "Claude Code unavailable. Check its executable in settings.",
        ),
    ] {
        let mut probe = ready_probe(HarnessKind::Claude);
        probe.available = available;
        probe.authenticated = authenticated;
        probe.message = "原始探测诊断".into();
        runner.emit(Event::HarnessDetected(probe));
        presenter.drain_events();
        assert_eq!(
            presenter
                .model()
                .latest_log_text(presenter.model().language),
            expected
        );
        assert_eq!(
            presenter.model.latest_log_text(Language::Chinese),
            "原始探测诊断"
        );
        assert_eq!(remote_status(&mut presenter), "");
    }
    runner.emit(Event::HarnessDetected(ready_probe(HarnessKind::Claude)));
    presenter.drain_events();
    presenter.select_effort(ThinkingEffort::Default);
    assert!(presenter.submit("hello", "claude"));
    assert_eq!(
        presenter
            .model()
            .latest_log_text(presenter.model().language),
        "Starting Claude Code · Model default"
    );
    assert_eq!(
        remote_status(&mut presenter),
        "正在启动 Claude Code · 模型默认"
    );
    assert!(runner.0.borrow().commands.iter().any(|command| matches!(
        &command.command,
        Command::RunStart(request) if request.effort == ThinkingEffort::Default && request.model.is_none()
    )));
}

#[test]
fn appearance_preferences_restore_and_accept_missing_or_invalid_settings() {
    use crate::model::{AppearanceSettings, FontSettings, ThemePreference};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("appearance.sqlite");
    let mut presenter = Presenter::new(
        Storage::open(&path).unwrap(),
        Err(anyhow::anyhow!("test")),
        None,
    );
    assert_eq!(presenter.model().appearance, AppearanceSettings::default());
    assert_eq!(presenter.model().fonts, FontSettings::default());
    for theme in [
        ThemePreference::Dark,
        ThemePreference::Light,
        ThemePreference::System,
    ] {
        let appearance = AppearanceSettings {
            theme,
            glass: false,
            reduced_motion: true,
        };
        assert!(presenter.set_appearance(appearance));
        drop(presenter);
        presenter = Presenter::new(
            Storage::open(&path).unwrap(),
            Err(anyhow::anyhow!("test")),
            None,
        );
        assert_eq!(presenter.model().appearance, appearance);
    }
    for (raw, expected) in [
        (
            r#"{"theme":"dark"}"#,
            AppearanceSettings {
                theme: ThemePreference::Dark,
                ..Default::default()
            },
        ),
        ("invalid json", AppearanceSettings::default()),
        (r#"{"theme":"unknown"}"#, AppearanceSettings::default()),
    ] {
        presenter.storage.set_setting("appearance", raw).unwrap();
        drop(presenter);
        presenter = Presenter::new(
            Storage::open(&path).unwrap(),
            Err(anyhow::anyhow!("test")),
            None,
        );
        assert_eq!(presenter.model().appearance, expected);
    }
    for fonts in [
        FontSettings {
            reading: Some("SimSun".into()),
            code: Some("Consolas".into()),
        },
        FontSettings::default(),
    ] {
        let appearance = presenter.model().appearance;
        presenter.set_fonts(fonts.clone()).unwrap();
        drop(presenter);
        presenter = Presenter::new(
            Storage::open(&path).unwrap(),
            Err(anyhow::anyhow!("test")),
            None,
        );
        assert_eq!(presenter.model().fonts, fonts);
        assert_eq!(presenter.model().appearance, appearance);
    }
    for (raw, expected) in [
        (
            r#"{"reading":"Songti SC"}"#,
            FontSettings {
                reading: Some("Songti SC".into()),
                ..Default::default()
            },
        ),
        ("invalid json", FontSettings::default()),
        (r#"{"code":42}"#, FontSettings::default()),
    ] {
        presenter.storage.set_setting("fonts", raw).unwrap();
        drop(presenter);
        presenter = Presenter::new(
            Storage::open(&path).unwrap(),
            Err(anyhow::anyhow!("test")),
            None,
        );
        assert_eq!(presenter.model().fonts, expected);
    }
}

#[test]
fn sound_settings_default_on_and_restore_across_restart() {
    use crate::model::SoundSettings;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sound.sqlite");
    let mut presenter = Presenter::new(
        Storage::open(&path).unwrap(),
        Err(anyhow::anyhow!("test")),
        None,
    );
    assert!(presenter.model().sound.task_complete);
    assert!(presenter.set_sound(SoundSettings {
        task_complete: false
    }));
    drop(presenter);
    presenter = Presenter::new(
        Storage::open(&path).unwrap(),
        Err(anyhow::anyhow!("test")),
        None,
    );
    assert!(!presenter.model().sound.task_complete);
    for (raw, expected) in [
        (r#"{"task_complete":true}"#, true),
        ("invalid json", true),
        (r#"{"task_complete":"yes"}"#, true),
    ] {
        presenter.storage.set_setting("sound", raw).unwrap();
        drop(presenter);
        presenter = Presenter::new(
            Storage::open(&path).unwrap(),
            Err(anyhow::anyhow!("test")),
            None,
        );
        assert_eq!(presenter.model().sound.task_complete, expected);
    }
}
