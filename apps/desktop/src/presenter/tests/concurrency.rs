use super::*;

#[test]
fn concurrent_worktree_tasks_isolate_output_approvals_queue_and_cancellation() {
    let (mut presenter, runner, _directory, first) = worktree_fixture("first");
    assert!(presenter.submit("first follow-up", "claude"));
    presenter.new_task();
    let second = start_test_worktree(&mut presenter, &runner, "second");
    assert_eq!(presenter.model.active_run_count(), 2);
    assert_ne!(first.cwd, second.cwd);
    let approval_id = Uuid::new_v4();
    runner.emit(Event::RunSessionStarted {
        run_id: first.run_id,
        session_id: "first-session".into(),
    });
    runner.emit(Event::RunOutputDelta {
        run_id: first.run_id,
        text: "first output".into(),
    });
    runner.emit(Event::RunOutputDelta {
        run_id: second.run_id,
        text: "second output".into(),
    });
    runner.emit(Event::RunApprovalRequested {
        run_id: first.run_id,
        request: nexus_protocol::ApprovalRequest {
            request_id: approval_id,
            title: "first approval".into(),
            details: "first only".into(),
            options: vec!["Approve".into()],
        },
    });
    presenter.drain_events();
    assert_eq!(
        presenter.model.conversation.selected_task,
        Some(second.task_id)
    );
    assert_eq!(presenter.model.conversation.streaming_text, "second output");
    assert!(presenter.model.conversation.pending_approvals.is_empty());
    presenter.new_task();
    let third = start_test_worktree(&mut presenter, &runner, "third");
    assert_eq!(presenter.model.active_run_count(), 3);
    assert_ne!(third.cwd, first.cwd);
    assert_ne!(third.cwd, second.cwd);
    assert!(presenter.submit("third follow-up", "claude"));
    runner.emit(Event::RunOutputDelta {
        run_id: third.run_id,
        text: "third output".into(),
    });
    presenter.drain_events();
    assert_eq!(presenter.model.conversation.streaming_text, "third output");
    assert!(presenter.model.conversation.pending_approvals.is_empty());
    presenter.select_task(first.task_id);
    assert_eq!(presenter.model.conversation.streaming_text, "first output");
    assert_eq!(presenter.model.conversation.queued_messages.len(), 1);
    assert!(presenter.respond_approval(first.run_id, approval_id, Some(0)));
    assert!(
        matches!(runner.0.borrow().commands.last().unwrap().command, Command::RunApprovalRespond { run_id, .. } if run_id == first.run_id)
    );
    presenter.select_task(second.task_id);
    presenter.cancel();
    assert!(
        matches!(runner.0.borrow().commands.last().unwrap().command, Command::RunCancel { run_id } if run_id == second.run_id)
    );
    runner.emit(Event::RunExited {
        run_id: first.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    let follow_up = last_start(&runner);
    assert_eq!(follow_up.task_id, first.task_id);
    assert_eq!(follow_up.cwd, first.cwd);
    assert_eq!(follow_up.session_id.as_deref(), Some("first-session"));
    assert_eq!(follow_up.prompt, "first follow-up");
    assert_eq!(
        presenter.model.conversation.selected_task,
        Some(second.task_id)
    );
    assert!(presenter.model.conversation.run_cancelling);
    runner.emit(Event::RunExited {
        run_id: second.run_id,
        status: RunStatus::Cancelled,
        exit_code: None,
    });
    presenter.drain_events();
    assert_eq!(presenter.model.active_run_count(), 2);
    assert!(presenter.model.task_running(first.task_id));
    assert!(!presenter.model.task_running(second.task_id));
    presenter.select_task(third.task_id);
    assert_eq!(presenter.model.conversation.active_run, Some(third.run_id));
    assert_eq!(presenter.model.conversation.streaming_text, "third output");
    assert_eq!(presenter.model.conversation.queued_messages.len(), 1);
    assert_eq!(
        presenter.model.conversation.queued_messages[0].prompt,
        "third follow-up"
    );
    assert!(presenter.model.conversation.pending_approvals.is_empty());
    assert!(!presenter.model.conversation.run_cancelling);
}

#[test]
fn a_second_local_task_cannot_write_to_an_active_checkout() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("first", "claude"));
    let first = last_start(&runner);
    presenter.new_task();
    assert!(!presenter.submit("second", "claude"));
    assert!(
        presenter
            .model
            .latest_log_text(presenter.model.language)
            .contains("checkout")
    );
    assert_eq!(last_start(&runner).run_id, first.run_id);
    assert_eq!(presenter.model.active_run_count(), 1);
}

#[test]
fn deleting_external_worktree_tasks_preserves_the_directory_and_association() {
    use crate::infrastructure::git;
    let (directory, project) = git::tests::repository_fixture();
    let external = directory.path().join("external");
    git::git(
        Path::new(&project.canonical_path),
        &[
            "worktree",
            "add",
            "-b",
            "external",
            external.to_str().unwrap(),
        ],
    )
    .unwrap();
    let (mut presenter, runner, _fixture) = fixture();
    presenter.open_project(&external);
    assert!(presenter.submit("external task", "claude"));
    let start = last_start(&runner);
    assert_eq!(start.prompt, "external task");
    runner.emit(Event::RunExited {
        run_id: start.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    presenter.reload_workspaces();
    let workspace = presenter
        .storage
        .task_workspace(start.task_id)
        .unwrap()
        .unwrap();
    assert!(workspace.external);
    assert!(!workspace.managed);
    assert!(presenter.delete_task(start.task_id));
    assert!(presenter.storage.workspace(workspace.id).unwrap().is_some());
    assert!(external.join("tracked.txt").exists());
    assert!(external.join(".git").exists());
}
