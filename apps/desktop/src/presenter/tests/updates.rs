use super::*;

#[test]
fn harness_updates_require_a_verified_newer_version() {
    let (mut presenter, _, _directory) = fixture();
    seed_harness_installations(&mut presenter);
    for (current, latest, available) in [
        (Some("1.0.0"), Some("2.0.0"), true),
        (Some("1.0.0"), Some("1.0.0"), false),
        (Some("2.0.0"), Some("1.0.0"), false),
        (Some("1.9.0"), Some("1.10.0"), true),
        (Some("1.0.0-alpha.2"), Some("1.0.0-alpha.10"), true),
        (Some("1.0.0-alpha.2"), Some("1.0.0"), true),
        (Some("1.0.0"), Some("1.0.0-alpha.2"), false),
        (Some("1.0.0+local"), Some("1.0.0+release"), false),
        (Some("1.0.0"), None, false),
        (None, Some("2.0.0"), false),
        (Some("unknown"), Some("2.0.0"), false),
    ] {
        let installation = presenter
            .model
            .harness_manager
            .installations
            .get_mut(&HarnessKind::Claude)
            .unwrap();
        installation.version = current.map(str::to_owned);
        installation.latest_version = latest
            .map(str::to_owned)
            .ok_or_else(|| "最新版本检测失败：网络不可用".to_owned().into());
        assert_eq!(
            presenter
                .harness_maintenance_request(HarnessKind::Claude, None)
                .is_some(),
            available,
            "current: {current:?}, latest: {latest:?}"
        );
    }
}

#[test]
fn harness_management_guards_running_tasks_stale_settings_and_unowned_installs() {
    use crate::model::harness_installation::InstallMethod;
    let (mut presenter, _, _directory) = fixture();
    seed_harness_installations(&mut presenter);
    assert!(
        presenter
            .harness_maintenance_request(HarnessKind::Claude, None)
            .is_some()
    );
    assert!(
        presenter
            .harness_maintenance_request(HarnessKind::Codex, Some(InstallMethod::VitePlus))
            .is_some()
    );
    assert!(
        presenter
            .harness_maintenance_request(HarnessKind::Omp, None)
            .is_none()
    );
    presenter.model.harness_manager.busy = true;
    assert!(
        presenter
            .harness_maintenance_request(HarnessKind::Claude, None)
            .is_none()
    );
    presenter.model.harness_manager.operating = Some(HarnessKind::Claude);
    assert!(!presenter.model.can_submit());
    assert!(!presenter.submit("must wait for installer", "claude"));
    presenter.model.harness_manager.busy = false;
    presenter.model.harness_manager.operating = None;
    presenter.model.conversation.executable = "/changed/claude".into();
    assert!(
        presenter
            .harness_maintenance_request(HarnessKind::Claude, None)
            .is_none()
    );
    presenter.model.conversation.executable = "claude".into();
    assert!(presenter.submit("running task", "claude"));
    assert!(
        presenter
            .harness_maintenance_request(HarnessKind::Codex, Some(InstallMethod::VitePlus))
            .is_none()
    );
}

#[test]
fn harness_installation_events_refresh_versions_and_probes_without_switching_harnesses() {
    use crate::infrastructure::harness_installation::{Event as InstallationEvent, Worker};
    let (mut presenter, runner, _directory) = fixture();
    seed_harness_installations(&mut presenter);
    let request = presenter
        .harness_maintenance_request(HarnessKind::Claude, None)
        .unwrap();
    presenter.select_harness(HarnessKind::Omp, "claude");
    let selected = presenter.model.conversation.selected_harness;
    let executable = presenter.model.conversation.executable.clone();
    let project = presenter
        .model
        .conversation
        .selected_project
        .as_ref()
        .unwrap()
        .id;
    for failed in [false, true] {
        let (send, worker) = Worker::test_channel();
        presenter.installation_worker = Some(worker);
        presenter.model.harness_manager.busy = true;
        presenter.model.harness_manager.operating = Some(HarnessKind::Claude);
        let mut installation =
            presenter.model.harness_manager.installations[&HarnessKind::Claude].clone();
        installation.version = Some("2.0.0".into());
        installation.latest_version = Ok("2.0.0".into());
        send.send(InstallationEvent::Scanned(
            HarnessKind::Claude,
            installation,
        ))
        .unwrap();
        send.send(InstallationEvent::Finished {
            request: Some(request.clone()),
            result: if failed { Err("失败".into()) } else { Ok(()) },
        })
        .unwrap();
        assert!(presenter.drain_installation_events());
        assert!(!presenter.model.harness_manager.busy);
        assert!(presenter.model.harness_manager.operating.is_none());
        assert_eq!(
            presenter.model.harness_manager.installations[&HarnessKind::Claude]
                .version
                .as_deref(),
            Some("2.0.0")
        );
        assert_eq!(
            presenter.model.harness_manager.installations[&HarnessKind::Claude]
                .latest_version
                .as_deref()
                .unwrap(),
            "2.0.0"
        );
        assert!(
            presenter
                .harness_maintenance_request(HarnessKind::Claude, None)
                .is_none()
        );
        assert_eq!(presenter.model.conversation.selected_harness, selected);
        assert_eq!(presenter.model.conversation.executable, executable);
        assert_eq!(
            presenter
                .model
                .conversation
                .selected_project
                .as_ref()
                .unwrap()
                .id,
            project
        );
        assert!(runner.0.borrow().commands.iter().any(|command| matches!(&command.command, Command::HarnessProbe { harness: HarnessKind::Claude, executable, .. } if executable == "claude")));
        assert!(!presenter.drain_installation_events());
    }
}

#[test]
fn harness_scan_ignores_stale_paths_and_recovers_from_disconnected_workers() {
    use crate::infrastructure::harness_installation::{Event as InstallationEvent, Worker};
    let (mut presenter, _, _directory) = fixture();
    seed_harness_installations(&mut presenter);
    let mut stale = presenter.model.harness_manager.installations[&HarnessKind::Claude].clone();
    stale.configured = "old-path".into();
    stale.version = Some("obsolete".into());
    let (send, worker) = Worker::test_channel();
    presenter.installation_worker = Some(worker);
    presenter.model.harness_manager.busy = true;
    send.send(InstallationEvent::Scanned(HarnessKind::Claude, stale))
        .unwrap();
    drop(send);
    assert!(presenter.drain_installation_events());
    assert!(!presenter.model.harness_manager.busy);
    assert!(presenter.installation_worker.is_none());
    assert_eq!(
        presenter.model.harness_manager.installations[&HarnessKind::Claude]
            .version
            .as_deref(),
        Some("1.0.0")
    );
    assert!(
        presenter
            .model
            .harness_manager
            .message
            .as_ref()
            .unwrap()
            .render(Language::Chinese)
            .contains("中断")
    );
}

#[test]
fn harness_scan_adopts_a_verified_manager_path_and_reprobes_it() {
    use crate::infrastructure::harness_installation::{Event as InstallationEvent, Worker};
    let (mut presenter, runner, directory) = fixture();
    seed_harness_installations(&mut presenter);
    let mut installation =
        presenter.model.harness_manager.installations[&HarnessKind::Claude].clone();
    let discovered = directory.path().join("custom global/bin/claude");
    installation.executable = Some(discovered.clone());
    installation.discovered_from_manager = true;
    let (send, worker) = Worker::test_channel();
    presenter.installation_worker = Some(worker);
    send.send(InstallationEvent::Scanned(
        HarnessKind::Claude,
        installation,
    ))
    .unwrap();
    send.send(InstallationEvent::Finished {
        request: None,
        result: Ok(()),
    })
    .unwrap();
    assert!(presenter.drain_installation_events());
    assert_eq!(
        presenter.model.conversation.executable,
        discovered.display().to_string()
    );
    assert_eq!(
        presenter
            .storage
            .setting("claude_executable")
            .unwrap()
            .as_deref(),
        Some(discovered.to_str().unwrap())
    );
    assert!(runner.0.borrow().commands.iter().any(|command| matches!(&command.command, Command::HarnessProbe { harness: HarnessKind::Claude, executable, .. } if executable == discovered.to_str().unwrap())));
}

#[test]
fn update_preferences_restore_and_invalid_values_follow_the_installed_channel() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let mut presenter = Presenter::new(
        Storage::open(&path).unwrap(),
        Err(anyhow::anyhow!("test")),
        None,
    );
    assert_eq!(presenter.model().updates.channel, UpdateChannel::default());
    assert!(presenter.model().updates.check_on_startup);
    assert!(presenter.set_update_check_on_startup(false));
    for channel in [UpdateChannel::Nightly, UpdateChannel::Release] {
        assert!(presenter.set_update_channel(channel));
        drop(presenter);
        presenter = Presenter::new(
            Storage::open(&path).unwrap(),
            Err(anyhow::anyhow!("test")),
            None,
        );
        assert_eq!(presenter.model().updates.channel, channel);
        assert!(!presenter.model().updates.check_on_startup);
    }
    presenter
        .storage
        .set_setting("update_channel", "unknown")
        .unwrap();
    drop(presenter);
    let presenter = Presenter::new(
        Storage::open(&path).unwrap(),
        Err(anyhow::anyhow!("test")),
        None,
    );
    assert_eq!(presenter.model().updates.channel, UpdateChannel::default());
    assert!(!presenter.model().updates.check_on_startup);
}

#[test]
fn cli_installation_reports_completion_and_failure_without_changing_the_conversation() {
    let (mut presenter, _, _directory) = fixture();
    let original_status = presenter
        .model()
        .latest_log_text(presenter.model().language)
        .to_owned();
    let original_project = presenter
        .model()
        .conversation
        .selected_project
        .as_ref()
        .map(|project| project.id);
    for result in [Ok(()), Err(anyhow::anyhow!("permission denied"))] {
        let success = result.is_ok();
        let sender = pending_cli_installation(&mut presenter);
        presenter.install_cli();
        assert!(presenter.model().cli_installation_busy);
        assert!(!presenter.drain_cli_installation_result());
        sender.send(result).unwrap();
        assert!(presenter.drain_events());
        assert!(!presenter.model().cli_installation_busy);
        assert!(presenter.cli_installation_result.is_none());
        let message = presenter.model().cli_installation_message.as_ref().unwrap();
        assert!(message.render(Language::English).contains(if success {
            "CLI installed"
        } else {
            "permission denied"
        }));
        assert_eq!(
            presenter
                .model()
                .latest_log_text(presenter.model().language),
            original_status
        );
        assert_eq!(
            presenter
                .model()
                .conversation
                .selected_project
                .as_ref()
                .map(|project| project.id),
            original_project
        );
    }
    drop(pending_cli_installation(&mut presenter));
    assert!(presenter.drain_cli_installation_result());
    assert!(!presenter.model().cli_installation_busy);
    assert!(
        presenter
            .model()
            .cli_installation_message
            .as_ref()
            .unwrap()
            .render(Language::Chinese)
            .contains("中断")
    );
}

#[test]
fn update_events_guard_concurrency_and_preserve_conversations_through_completion_and_failure() {
    let (mut presenter, _, directory) = fixture();
    assert!(presenter.submit("Keep this conversation running", "claude"));
    let task = presenter.model().conversation.selected_task;
    let run = presenter.model().conversation.active_run;
    let status = presenter
        .model()
        .latest_log_text(presenter.model().language)
        .to_owned();
    let sender = pending_update(&mut presenter);
    let channel = presenter.model().updates.channel;
    assert!(!presenter.set_update_channel(UpdateChannel::Nightly));
    presenter.check_for_updates();
    sender
        .send(UpdateState::Downloading {
            package: update_package(),
            received: 12,
        })
        .unwrap();
    assert!(presenter.drain_update_events());
    assert_eq!(presenter.model().updates.channel, channel);
    assert!(
        presenter
            .model()
            .updates
            .state
            .message(Language::English)
            .contains("50%")
    );
    assert!(!presenter.drain_update_events());
    sender
        .send(UpdateState::Ready {
            package: update_package(),
            path: directory.path().join("update.zip"),
        })
        .unwrap();
    assert!(presenter.drain_update_events());
    assert!(presenter.update_events.is_none());
    assert!(!presenter.set_update_channel(channel));
    assert!(!presenter.install_update_when_idle());
    assert!(matches!(
        presenter.model().updates.state,
        UpdateState::Ready { .. }
    ));
    let sender = pending_update(&mut presenter);
    drop(sender);
    assert!(presenter.drain_update_events());
    assert!(matches!(
        presenter.model().updates.state,
        UpdateState::Failed(_)
    ));
    assert_eq!(presenter.model().conversation.selected_task, task);
    assert_eq!(presenter.model().conversation.active_run, run);
    assert_eq!(
        presenter
            .model()
            .latest_log_text(presenter.model().language),
        status
    );
    assert!(!presenter.drain_update_events());
}

#[test]
fn update_installation_waits_for_all_tasks_and_workspace_work_and_blocks_new_operations() {
    let (mut presenter, runner, directory, run) = worktree_fixture("finish before updating");
    seed_harness_installations(&mut presenter);
    presenter.model.updates.state = UpdateState::Ready {
        package: update_package(),
        path: directory.path().join("missing-update.zip"),
    };
    presenter.new_task();
    assert!(presenter.model.conversation.active_run.is_none());
    assert!(!presenter.install_update_when_idle());
    runner.emit(Event::RunExited {
        run_id: run.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    assert_eq!(presenter.model.active_run_count(), 0);
    assert!(presenter.review_workspace(run.task_id));
    assert!(!presenter.install_update_when_idle());
    finish_workspace_operation(&mut presenter);
    presenter.model.harness_manager.busy = true;
    assert!(!presenter.install_update_when_idle());
    presenter.model.harness_manager.busy = false;
    assert!(presenter.install_update_when_idle());
    assert!(matches!(
        presenter.model.updates.state,
        UpdateState::Installing(_)
    ));
    assert!(!presenter.model().can_submit());
    assert!(!presenter.submit("Do not interrupt installation", "claude"));
    assert!(!presenter.preview_workspace_merge(run.task_id, "main".into()));
    assert!(
        presenter
            .harness_maintenance_request(HarnessKind::Claude, None)
            .is_none()
    );
    presenter.scan_harness_installations();
    assert!(!presenter.model.harness_manager.busy);
    // The install thread only reads a missing archive, but Windows CI can be
    // slow to schedule it while the rest of the suite runs in parallel.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while presenter.update_events.is_some() {
        assert!(std::time::Instant::now() < deadline);
        presenter.drain_update_events();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(matches!(
        presenter.model.updates.state,
        UpdateState::Failed(_)
    ));
    assert!(presenter.model.conversation.active_run.is_none());
}
