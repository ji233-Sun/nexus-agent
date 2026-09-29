use super::*;

#[test]
fn project_deletion_clears_selected_and_cached_state_and_preserves_worktree_files() {
    use crate::infrastructure::git;
    let (mut presenter, runner, _directory, start) = worktree_fixture("Keep my worktree");
    let project = presenter
        .model
        .conversation
        .selected_project
        .clone()
        .unwrap();
    let workspace = presenter
        .model
        .conversation
        .selected_workspace
        .clone()
        .unwrap();
    let repository = Path::new(&project.canonical_path);
    let worktree = Path::new(&workspace.path);
    fs::write(repository.join("tracked.txt"), "project edits\n").unwrap();
    fs::write(worktree.join("tracked.txt"), "worktree edits\n").unwrap();
    fs::write(worktree.join("untracked.txt"), "untracked work\n").unwrap();
    let worktrees = git::git(repository, &["worktree", "list", "--porcelain"]).unwrap();
    let branches = git::git(repository, &["show-ref", "--heads"]).unwrap();
    runner.emit(Event::RunExited {
        run_id: start.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    presenter
        .model
        .conversation
        .queued_messages
        .push_back(crate::model::QueuedMessage {
            attachments: Vec::new(),
            id: Uuid::new_v4(),
            task_id: start.task_id,
            prompt: "unsent follow-up".into(),
            permission_mode: PermissionMode::AutoEdit,
        });
    assert!(presenter.archive_task(start.task_id));
    seed_issues(&mut presenter, IssueProvider::Cnb);
    presenter.open_issues(IssueProvider::Cnb);
    assert!(presenter.model.cnb.opened);
    assert!(presenter.model.all_conversations().any(|conversation| {
        conversation.selected_task == Some(start.task_id)
            && !conversation.queued_messages.is_empty()
    }));

    assert!(presenter.delete_project(project.id));
    assert!(presenter.model.conversation.selected_project.is_none());
    assert!(presenter.model.conversation.selected_task.is_none());
    assert!(
        presenter
            .model
            .conversation
            .selected_workspace
            .as_ref()
            .unwrap()
            .project_id
            .is_none()
    );
    assert!(presenter.model.conversation.workspaces.is_empty());
    assert!(presenter.model.conversation.messages.is_empty());
    assert!(presenter.model.conversation.tasks.is_empty());
    assert!(presenter.model.archived_tasks.is_empty());
    assert!(presenter.model.conversation.queued_messages.is_empty());
    assert!(!presenter.model.conversation.project_is_git);
    assert!(matches!(
        presenter.model.conversation.model_catalog,
        ModelCatalogState::Loading { .. }
    ));
    assert!(!presenter.model.cnb.opened);
    assert!(presenter.model.cnb.repository.is_none());
    assert!(presenter.model.all_conversations().all(|conversation| {
        conversation
            .selected_project
            .as_ref()
            .is_none_or(|item| item.id != project.id)
    }));
    assert!(presenter.storage.workspace(workspace.id).unwrap().is_none());
    let remote = presenter.remote_state();
    assert!(remote.projects.iter().all(|item| item.id != project.id));
    assert!(
        remote
            .tasks
            .iter()
            .all(|task| task.project_id != Some(project.id))
    );
    assert!(remote.selected_project_id.is_none());
    assert_eq!(
        fs::read_to_string(repository.join("tracked.txt")).unwrap(),
        "project edits\n"
    );
    assert_eq!(
        fs::read_to_string(worktree.join("tracked.txt")).unwrap(),
        "worktree edits\n"
    );
    assert_eq!(
        fs::read_to_string(worktree.join("untracked.txt")).unwrap(),
        "untracked work\n"
    );
    assert_eq!(
        git::git(repository, &["worktree", "list", "--porcelain"]).unwrap(),
        worktrees
    );
    assert_eq!(
        git::git(repository, &["show-ref", "--heads"]).unwrap(),
        branches
    );
}

#[test]
fn project_deletion_guards_background_runs_and_preserves_other_conversations() {
    let (mut presenter, runner, _directory) = fixture();
    let project = presenter
        .model
        .conversation
        .selected_project
        .clone()
        .unwrap();
    assert!(presenter.submit("background task", "claude"));
    let start = last_start(&runner);
    assert!(!presenter.can_delete_project(project.id));
    let other_dir = tempfile::tempdir().unwrap();
    presenter.open_project(other_dir.path());
    let other = presenter
        .model
        .conversation
        .selected_project
        .clone()
        .unwrap();
    assert!(presenter.submit("keep running", "claude"));
    let other_start = last_start(&runner);
    let selected_conversation = presenter.model.conversation.id;
    assert!(!presenter.delete_project(project.id));
    assert!(presenter.storage.project(project.id).unwrap().is_some());
    runner.emit(Event::RunExited {
        run_id: start.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    presenter.model.workspace_busy = true;
    assert!(!presenter.delete_project(project.id));
    presenter.model.workspace_busy = false;
    assert!(presenter.can_delete_project(project.id));
    assert!(presenter.delete_project(project.id));
    assert_eq!(presenter.model.conversation.id, selected_conversation);
    assert_eq!(
        presenter
            .model
            .conversation
            .selected_project
            .as_ref()
            .unwrap()
            .id,
        other.id
    );
    assert_eq!(
        presenter.model.conversation.active_run,
        Some(other_start.run_id)
    );
    assert_eq!(
        presenter.model.conversation.selected_task,
        Some(other_start.task_id)
    );
    assert_eq!(
        presenter.model.conversation.messages[0].content,
        "keep running"
    );
    assert_eq!(presenter.model.active_run_count(), 1);
    assert!(presenter.model.conversations.values().all(|conversation| {
        conversation
            .selected_project
            .as_ref()
            .is_none_or(|item| item.id != project.id)
    }));
}

#[test]
fn project_deletion_waits_for_pending_workspace_start_but_allows_failed_retry() {
    use crate::model::workspace::PendingWorkspaceStart;
    let (mut presenter, _runner, _directory) = fixture();
    let project = presenter
        .model
        .conversation
        .selected_project
        .clone()
        .unwrap();
    presenter.model.conversation.pending_workspace_start = Some(PendingWorkspaceStart {
        attachments: Vec::new(),
        context_id: presenter.model.conversation.id,
        prompt: "pending worktree".into(),
        executable: "claude".into(),
        permission: PermissionMode::AutoEdit,
    });
    let pending = presenter.model.conversation.id;
    presenter.new_task();
    assert!(!presenter.delete_project(project.id));
    presenter
        .model
        .conversations
        .get_mut(&pending)
        .unwrap()
        .workspace_retry = true;
    assert!(presenter.delete_project(project.id));
    assert!(
        presenter
            .model
            .conversation
            .pending_workspace_start
            .is_none()
    );
    assert!(presenter.model.conversations.is_empty());
}

#[test]
fn conversation_actions_keep_active_and_archived_models_in_sync() {
    let (mut presenter, _runner, _directory) = fixture();
    let project = presenter
        .model()
        .conversation
        .selected_project
        .clone()
        .unwrap();
    let create_task = |presenter: &mut Presenter, title: &str| {
        presenter
            .storage
            .create_task_run(NewTaskRun {
                attachments: &[],
                workspace_id: None,
                permission_mode: nexus_domain::PermissionMode::AutoEdit,
                task_id: None,
                project_id: Some(project.id),
                title,
                prompt: title,
                harness: HarnessKind::Claude,
                executable: "claude",
                model: None,
                effort: ThinkingEffort::Medium,
                harness_version: None,
            })
            .unwrap()
            .0
    };
    let first_task = create_task(&mut presenter, "First conversation");
    let second_task = create_task(&mut presenter, "Second conversation");
    for task_id in [first_task, second_task] {
        presenter.select_task(task_id);
        presenter
            .model
            .conversation
            .queued_messages
            .push_back(crate::model::QueuedMessage {
                attachments: Vec::new(),
                permission_mode: nexus_domain::PermissionMode::AutoEdit,
                id: Uuid::new_v4(),
                task_id,
                prompt: "unsent follow-up".into(),
            });
    }
    presenter.select_project(project);
    presenter.select_task(first_task);

    assert!(presenter.archive_task(first_task));
    assert!(presenter.model().conversation.selected_task.is_none());
    assert!(presenter.model().conversation.messages.is_empty());
    assert_eq!(presenter.model().conversation.tasks[0].id, second_task);
    assert_eq!(presenter.model().archived_tasks[0].id, first_task);
    assert_eq!(
        presenter
            .model()
            .all_conversations()
            .map(|conversation| conversation.queued_messages.len())
            .sum::<usize>(),
        2
    );

    assert!(presenter.restore_task(first_task));
    assert_eq!(presenter.model().conversation.tasks.len(), 2);
    assert!(presenter.model().archived_tasks.is_empty());

    presenter.model.conversation.active_run = Some(Uuid::new_v4());
    presenter.model.conversation.active_task = Some(first_task);
    assert!(!presenter.delete_task(first_task));
    assert_eq!(presenter.model().conversation.tasks.len(), 2);
    assert_eq!(
        presenter
            .model()
            .all_conversations()
            .map(|conversation| conversation.queued_messages.len())
            .sum::<usize>(),
        2
    );
    presenter.model.conversation.active_run = None;
    presenter.model.conversation.active_task = None;

    assert!(presenter.delete_task(first_task));
    assert_eq!(presenter.model().conversation.tasks.len(), 1);
    assert_eq!(
        presenter
            .model()
            .all_conversations()
            .map(|conversation| conversation.queued_messages.len())
            .sum::<usize>(),
        1
    );
    assert!(
        presenter
            .model()
            .all_conversations()
            .flat_map(|conversation| &conversation.queued_messages)
            .all(|message| message.task_id == second_task)
    );
    assert!(presenter.archive_task(second_task));
    assert_eq!(presenter.model().archived_tasks.len(), 1);
    assert!(presenter.delete_archived_tasks());
    assert!(presenter.model().archived_tasks.is_empty());
    assert!(presenter.model().conversation.tasks.is_empty());
    assert!(presenter.model().conversation.queued_messages.is_empty());
}

#[test]
fn reordering_projects_preserves_the_active_conversation_and_survives_reload() {
    let (mut presenter, runner, directory) = fixture();
    for name in ["second", "third"] {
        let path = directory.path().join(name);
        fs::create_dir(&path).unwrap();
        presenter.storage.open_project(&path).unwrap();
    }
    presenter.reload_projects();
    assert!(presenter.submit("Keep this conversation running", "claude"));
    let selected = presenter
        .model()
        .conversation
        .selected_project
        .as_ref()
        .unwrap()
        .id;
    let conversation = presenter.model().conversation.id;
    let task = presenter.model().conversation.selected_task;
    let run = presenter.model().conversation.active_run;
    let messages = serde_json::to_value(&presenter.model().conversation.messages).unwrap();
    let commands = runner.0.borrow().commands.len();
    let original: Vec<_> = presenter
        .model()
        .projects
        .iter()
        .map(|project| project.id)
        .collect();

    assert!(presenter.reorder_project(original[0], original[2]));
    let reordered = vec![original[1], original[2], original[0]];
    assert_eq!(
        presenter
            .model()
            .projects
            .iter()
            .map(|project| project.id)
            .collect::<Vec<_>>(),
        reordered,
    );
    presenter.reload_projects();
    assert_eq!(
        presenter
            .model()
            .projects
            .iter()
            .map(|project| project.id)
            .collect::<Vec<_>>(),
        reordered,
    );
    assert!(presenter.reorder_project(original[0], original[1]));
    assert!(!presenter.reorder_project(original[0], original[0]));
    assert!(!presenter.reorder_project(Uuid::new_v4(), original[0]));
    assert!(!presenter.reorder_project(original[0], Uuid::new_v4()));
    assert_eq!(
        presenter
            .storage
            .projects()
            .unwrap()
            .iter()
            .map(|project| project.id)
            .collect::<Vec<_>>(),
        original,
    );
    assert_eq!(
        presenter
            .model()
            .conversation
            .selected_project
            .as_ref()
            .unwrap()
            .id,
        selected
    );
    assert_eq!(presenter.model().conversation.id, conversation);
    assert_eq!(presenter.model().conversation.selected_task, task);
    assert_eq!(presenter.model().conversation.active_run, run);
    assert_eq!(
        serde_json::to_value(&presenter.model().conversation.messages).unwrap(),
        messages
    );
    assert_eq!(runner.0.borrow().commands.len(), commands);
}

#[test]
fn reordering_projects_keeps_the_original_order_when_saving_fails() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("nexus.db");
    let storage = Storage::open(&database).unwrap();
    for name in ["first", "second"] {
        let path = directory.path().join(name);
        fs::create_dir(&path).unwrap();
        storage.open_project(&path).unwrap();
    }
    rusqlite::Connection::open(&database)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_project_order BEFORE INSERT ON settings
         WHEN NEW.key = 'project_order'
         BEGIN SELECT RAISE(FAIL, 'cannot save order'); END;",
        )
        .unwrap();
    let mut presenter = Presenter::new(storage, Err(anyhow::anyhow!("test")), None);
    let original: Vec<_> = presenter
        .model()
        .projects
        .iter()
        .map(|project| project.id)
        .collect();
    assert!(!presenter.reorder_project(original[0], original[1]));
    assert_eq!(
        presenter
            .model()
            .projects
            .iter()
            .map(|project| project.id)
            .collect::<Vec<_>>(),
        original,
    );
    assert!(
        presenter
            .storage
            .setting("project_order")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        presenter.model().latest_log_text(Language::English),
        "Cannot save project order: cannot save order",
    );
}

#[test]
fn restoring_archived_project_preserves_the_selected_project_and_tasks() {
    let ArchivedProjectFixture {
        mut presenter,
        _directory,
        archived_project,
        archived_task,
        oldest_recent_project,
        oldest_recent_task,
    } = archived_project_fixture();
    assert_eq!(presenter.model().archived_tasks[0].id, archived_task);
    let archived_project_metadata = presenter
        .model()
        .projects
        .iter()
        .find(|project| project.id == archived_project.id)
        .unwrap();
    assert_eq!(archived_project_metadata.display_name, "archived-project");
    presenter.select_project(oldest_recent_project.clone());
    presenter.select_task(oldest_recent_task);

    assert!(presenter.restore_task(archived_task));
    assert!(presenter.model().archived_tasks.is_empty());
    assert_eq!(
        presenter
            .model()
            .conversation
            .selected_project
            .as_ref()
            .unwrap()
            .id,
        oldest_recent_project.id
    );
    assert_eq!(
        presenter.model().conversation.selected_task,
        Some(oldest_recent_task)
    );
    assert!(
        presenter
            .model()
            .conversation
            .tasks
            .iter()
            .any(|task| task.id == oldest_recent_task)
    );
    assert!(
        presenter
            .model()
            .projects
            .iter()
            .any(|project| project.id == oldest_recent_project.id)
    );
    let restored_project = presenter
        .model()
        .projects
        .iter()
        .find(|project| project.id == archived_project.id)
        .unwrap()
        .clone();
    presenter.select_project(restored_project);
    assert!(
        presenter
            .model()
            .conversation
            .tasks
            .iter()
            .any(|task| task.id == archived_task)
    );
}

#[test]
fn deleting_archived_tasks_refreshes_projects_for_individual_and_bulk_actions() {
    for delete_all in [false, true] {
        let ArchivedProjectFixture {
            mut presenter,
            _directory,
            archived_project,
            archived_task,
            ..
        } = archived_project_fixture();
        assert!(
            presenter
                .model()
                .projects
                .iter()
                .any(|project| project.id == archived_project.id)
        );

        if delete_all {
            assert!(presenter.delete_archived_tasks());
        } else {
            assert!(presenter.delete_task(archived_task));
        }

        assert!(presenter.model().archived_tasks.is_empty());
        assert_eq!(presenter.model().projects.len(), 20);
        assert!(
            presenter
                .model()
                .projects
                .iter()
                .all(|project| project.id != archived_project.id)
        );
    }
}

#[test]
fn projectless_chat_discovers_models_saves_history_and_resumes_after_restart() {
    for harness in [HarnessKind::Claude, HarnessKind::Codex] {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("nexus.db");
        let storage = Storage::open(&database).unwrap();
        storage
            .set_setting("default_harness", harness.as_str())
            .unwrap();
        let runner = FakeRunner::default();
        let mut presenter = Presenter::new(storage, Ok(Box::new(runner.clone())), None);
        assert!(presenter.model.projects.is_empty());
        assert!(presenter.model.conversation.selected_project.is_none());
        assert!(!presenter.model.can_submit());
        let cwd = presenter.model.working_directory().unwrap().to_owned();
        assert!(Path::new(&cwd).is_dir());
        assert!(
            Path::new(&cwd).starts_with(directory.path().canonicalize().unwrap().join("sessions"))
        );
        assert!(runner.0.borrow().commands.iter().any(|command| matches!(
            &command.command, Command::ModelCatalogRefresh { cwd: catalog_cwd, .. } if catalog_cwd == &cwd
        )));
        runner.emit(Event::HarnessDetected(ready_probe(harness)));
        emit_current_catalog(&presenter, &runner, claude_aliases());
        presenter.drain_events();
        assert!(presenter.model.can_submit());
        runner.0.borrow_mut().fail_send = true;
        assert!(!presenter.submit("你好", harness.default_executable()));
        assert!(presenter.storage.tasks(None).unwrap().is_empty());
        assert!(presenter.model.conversation.messages.is_empty());
        runner.0.borrow_mut().fail_send = false;
        assert!(presenter.submit("你好", harness.default_executable()));
        let start = last_start(&runner);
        assert_eq!(start.cwd, cwd);
        assert_eq!(start.prompt, "你好");
        assert!(start.session_id.is_none());
        assert_eq!(presenter.model.projectless_tasks[0].id, start.task_id);
        assert!(presenter.model.conversation.tasks[0].project_id.is_none());
        assert!(presenter.storage.projects().unwrap().is_empty());
        fs::write(Path::new(&cwd).join("conversation.txt"), "saved context").unwrap();
        runner.emit(Event::RunSessionStarted {
            run_id: start.run_id,
            session_id: "projectless-session".into(),
        });
        runner.emit(Event::RunOutputDelta {
            run_id: start.run_id,
            text: "你好！".into(),
        });
        runner.emit(Event::RunMessageCompleted {
            run_id: start.run_id,
            text: "你好！".into(),
        });
        runner.emit(Event::RunExited {
            run_id: start.run_id,
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
        assert!(presenter.model.projects.is_empty());
        assert_eq!(presenter.model.projectless_tasks[0].id, start.task_id);
        assert!(presenter.archive_task(start.task_id));
        assert!(presenter.model.projectless_tasks.is_empty());
        assert!(presenter.model.archived_tasks[0].project_id.is_none());
        assert!(presenter.restore_task(start.task_id));
        presenter.select_task(start.task_id);
        assert!(presenter.model.conversation.selected_project.is_none());
        assert_eq!(presenter.model.working_directory(), Some(cwd.as_str()));
        assert_eq!(
            presenter
                .model
                .conversation
                .messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            ["你好", "你好！"]
        );
        assert_eq!(
            fs::read_to_string(Path::new(&cwd).join("conversation.txt")).unwrap(),
            "saved context"
        );
        runner.emit(Event::HarnessDetected(ready_probe(harness)));
        emit_current_catalog(&presenter, &runner, claude_aliases());
        presenter.drain_events();
        assert!(presenter.model.can_submit());
        assert!(presenter.submit("继续", harness.default_executable()));
        let resumed = last_start(&runner);
        assert_eq!(resumed.task_id, start.task_id);
        assert_eq!(resumed.cwd, cwd);
        assert_eq!(resumed.session_id.as_deref(), Some("projectless-session"));
    }
}

#[test]
fn projectless_tasks_isolate_directories_and_preserve_project_context_when_switching() {
    let (mut presenter, runner, directory) = fixture();
    let project = presenter
        .model
        .conversation
        .selected_project
        .clone()
        .unwrap();
    assert!(presenter.submit("project task", "claude"));
    let project_start = last_start(&runner);
    presenter.new_projectless_task();
    emit_current_catalog(&presenter, &runner, claude_aliases());
    presenter.drain_events();
    assert!(presenter.model.can_submit());
    assert!(presenter.model.conversation.selected_project.is_none());
    assert!(presenter.submit("independent chat", "claude"));
    let first = last_start(&runner);
    assert_ne!(first.cwd, project.canonical_path);
    assert!(!Path::new(&first.cwd).starts_with(directory.path()));
    assert!(presenter.model.projectless_tasks[0].project_id.is_none());
    runner.emit(Event::RunSessionStarted {
        run_id: first.run_id,
        session_id: "independent-session".into(),
    });
    runner.emit(Event::RunExited {
        run_id: first.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    presenter.new_task();
    let second_cwd = presenter.model.working_directory().unwrap().to_owned();
    assert_ne!(second_cwd, first.cwd);
    assert_ne!(second_cwd, project.canonical_path);
    assert!(presenter.submit("another chat", "claude"));
    let second = last_start(&runner);
    assert_eq!(second.cwd, second_cwd);
    assert_ne!(second.task_id, first.task_id);
    assert!(second.session_id.is_none());
    presenter.select_task(project_start.task_id);
    assert_eq!(
        presenter
            .model
            .conversation
            .selected_project
            .as_ref()
            .unwrap()
            .id,
        project.id
    );
    assert_eq!(
        presenter.model.working_directory(),
        Some(project.canonical_path.as_str())
    );
    assert_eq!(
        presenter.model.conversation.active_run,
        Some(project_start.run_id)
    );
    presenter.select_task(first.task_id);
    assert!(presenter.model.conversation.selected_project.is_none());
    assert_eq!(
        presenter.model.working_directory(),
        Some(first.cwd.as_str())
    );
    assert_eq!(
        presenter.model.conversation.messages[0].content,
        "independent chat"
    );
    runner.emit(Event::RunExited {
        run_id: second.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    fs::rename(&first.cwd, format!("{}-missing", first.cwd)).unwrap();
    assert!(!presenter.submit("must not fall back", "claude"));
    assert_eq!(last_start(&runner).run_id, second.run_id);
    assert!(!Path::new(&first.cwd).exists());
    assert_eq!(presenter.storage.tasks(project.id).unwrap().len(), 1);
    assert_eq!(presenter.storage.tasks(None).unwrap().len(), 2);
}

#[test]
fn selecting_a_saved_task_restores_its_configuration_and_messages() {
    let (mut presenter, runner, _directory) = fixture();
    presenter.select_catalog_model(Some("sonnet".into()));
    presenter.select_effort(ThinkingEffort::High);
    assert!(presenter.submit("hello", "claude"));
    let run_id = presenter.model().conversation.active_run.unwrap();
    let task_id = presenter.model().conversation.selected_task.unwrap();
    runner.emit(Event::RunExited {
        run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    presenter.new_task();
    assert!(presenter.model().conversation.messages.is_empty());
    assert!(presenter.select_harness(HarnessKind::Codex, "claude"));
    presenter.select_effort(ThinkingEffort::Low);
    presenter.select_task(task_id);
    assert_eq!(
        presenter.model().conversation.selected_harness,
        HarnessKind::Claude
    );
    assert_eq!(
        presenter.model().conversation.model_override.as_deref(),
        Some("sonnet")
    );
    assert_eq!(
        presenter.model().conversation.effort,
        ThinkingEffort::Default
    );
    assert_eq!(
        presenter.model().conversation.executable,
        ready_probe(HarnessKind::Claude).executable
    );
    assert_eq!(presenter.model().conversation.messages[0].content, "hello");

    for catalog_pending in [false, true] {
        presenter.new_task();
        let executable = presenter.model().conversation.executable.clone();
        assert!(presenter.select_harness(HarnessKind::Codex, &executable));
        presenter
            .model
            .harnesses
            .insert(HarnessKind::Codex, ready_probe(HarnessKind::Codex));
        if !catalog_pending {
            emit_current_catalog(&presenter, &runner, vec![]);
            presenter.drain_events();
        }
        assert!(presenter.submit("another task", "codex"));
        let run_id = presenter.model().conversation.active_run.unwrap();
        runner.0.borrow_mut().commands.clear();
        presenter.select_task(task_id);
        assert_eq!(
            presenter.model().conversation.selected_harness,
            HarnessKind::Claude
        );

        runner.emit(Event::RunExited {
            run_id,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        let state = runner.0.borrow();
        let refreshes = state
            .commands
            .iter()
            .map(|envelope| &envelope.command)
            .filter(|command| matches!(command, Command::ModelCatalogRefresh { context_id, .. } if *context_id == Some(presenter.model().conversation.id)))
            .collect::<Vec<_>>();
        assert_eq!(
            refreshes.len(),
            1,
            "task restoration must refresh only once"
        );
        assert!(matches!(refreshes[0], Command::ModelCatalogRefresh {
            request_id, harness: HarnessKind::Claude, executable, cwd, ..
        } if *request_id == current_catalog_request_id(&presenter)
            && *executable == ready_probe(HarnessKind::Claude).executable
            && *cwd == presenter.model().conversation.selected_project.as_ref().unwrap().canonical_path));
        drop(state);

        emit_current_catalog(&presenter, &runner, claude_aliases());
        presenter.drain_events();
        assert_eq!(
            presenter.model().conversation.model_override.as_deref(),
            Some("sonnet")
        );
        assert_eq!(presenter.model().conversation.messages[0].content, "hello");
        assert!(presenter.model().catalog_selection_is_valid());
    }
}

#[test]
fn working_directory_follows_project_and_task_selection_during_background_runs() {
    let (mut presenter, runner, directory) = fixture();
    let mut conversations = Vec::new();
    for parent in ["first", "第二个 项目"] {
        let path = directory.path().join(parent).join("同名目录 nexus");
        fs::create_dir_all(&path).unwrap();
        presenter.open_project(&path);
        assert_eq!(
            presenter.model().working_directory(),
            path.canonicalize().unwrap().to_str()
        );
        assert!(presenter.submit("first question", "claude"));
        let start = last_start(&runner);
        assert_eq!(
            Some(start.cwd.as_str()),
            presenter.model().working_directory()
        );
        conversations.push((
            presenter
                .model()
                .conversation
                .selected_project
                .clone()
                .unwrap(),
            start.task_id,
        ));
        runner.emit(Event::RunSessionStarted {
            run_id: start.run_id,
            session_id: start.task_id.to_string(),
        });
        runner.emit(Event::RunExited {
            run_id: start.run_id,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        assert_eq!(
            Some(start.cwd.as_str()),
            presenter.model().working_directory()
        );
    }
    let (first_project, first_task) = &conversations[0];
    let (second_project, second_task) = &conversations[1];
    assert_eq!(first_project.display_name, second_project.display_name);
    assert_ne!(first_project.canonical_path, second_project.canonical_path);
    assert!(presenter.submit("background follow-up", "claude"));
    let background = last_start(&runner);

    presenter.select_project(first_project.clone());
    presenter.select_task(*first_task);
    runner.emit(Event::RunOutputDelta {
        run_id: background.run_id,
        text: "background output".into(),
    });
    presenter.drain_events();
    assert_eq!(
        presenter.model().working_directory(),
        Some(first_project.canonical_path.as_str())
    );
    runner.emit(Event::RunExited {
        run_id: background.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    assert_eq!(
        presenter.model().conversation.selected_task,
        Some(*first_task)
    );
    assert_eq!(
        presenter.model().working_directory(),
        Some(first_project.canonical_path.as_str())
    );
    presenter.select_project(second_project.clone());
    presenter.select_task(*second_task);
    assert!(presenter.submit("resume second task", "claude"));
    assert_eq!(last_start(&runner).cwd, second_project.canonical_path);
    assert_eq!(
        presenter.model().working_directory(),
        Some(second_project.canonical_path.as_str())
    );
}
