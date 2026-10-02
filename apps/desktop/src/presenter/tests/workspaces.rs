use super::*;

#[test]
fn failed_worktree_creation_retries_with_a_new_branch_and_keeps_the_selected_base() {
    use crate::{
        infrastructure::git,
        model::workspace::{WorkspaceKind, WorkspaceStatus},
    };
    let (directory, project) = git::tests::repository_fixture();
    let project_path = Path::new(&project.canonical_path);
    git::git(project_path, &["branch", "release"]).unwrap();
    std::fs::write(
        project_path.join("tracked.txt"),
        "original checkout changes\n",
    )
    .unwrap();
    let (mut presenter, runner, _fixture) = fixture();
    presenter.worktree_root = Ok(directory.path().canonicalize().unwrap().join("worktrees"));
    presenter.open_project(project_path);
    presenter.new_task();
    presenter.select_workspace_kind(WorkspaceKind::Worktree);
    let planned = git::planned_workspace(
        presenter.worktree_root.as_ref().unwrap(),
        &project,
        &presenter.model.conversation.workspace_draft,
    );
    git::git(
        project_path,
        &["branch", planned.branch.as_deref().unwrap()],
    )
    .unwrap();
    assert!(presenter.submit("retry task", "claude"));
    finish_workspace_operation(&mut presenter);
    assert!(presenter.model.conversation.workspace_retry);
    assert_eq!(presenter.model.occupied_run_slots(), 0);
    let failed = presenter
        .model
        .conversation
        .selected_workspace
        .clone()
        .unwrap();
    assert_eq!(failed.status, WorkspaceStatus::Missing);
    presenter.select_workspace_base("release".into());
    assert!(presenter.retry_workspace_start());
    finish_workspace_operation(&mut presenter);
    assert_eq!(presenter.model.occupied_run_slots(), 1);
    emit_current_catalog(&presenter, &runner, claude_aliases());
    presenter.drain_events();
    let started = last_start(&runner);
    let branch = git::current_branch(Path::new(&started.cwd)).unwrap();
    assert!(branch.starts_with("feat/nx-"));
    assert_ne!(Some(branch), planned.branch);
    assert_eq!(presenter.model.conversation.workspace_draft.base, "release");
    assert_ne!(failed.task_id, Some(started.task_id));
    assert_eq!(
        presenter
            .storage
            .workspace(failed.id)
            .unwrap()
            .unwrap()
            .status,
        WorkspaceStatus::Missing
    );
    assert_eq!(
        presenter.model.conversation.active_run,
        Some(started.run_id)
    );
}

#[test]
fn worktree_retry_uses_its_own_context_while_another_task_is_selected() {
    use crate::{infrastructure::git, model::workspace::WorkspaceKind};
    let (directory, project) = git::tests::repository_fixture();
    let path = Path::new(&project.canonical_path);
    let (mut presenter, runner, _fixture) = fixture();
    presenter.worktree_root = Ok(directory.path().canonicalize().unwrap().join("worktrees"));
    presenter.open_project(path);
    presenter.select_workspace_kind(WorkspaceKind::Worktree);
    let context = presenter.model.conversation.id;
    let planned = git::planned_workspace(
        presenter.worktree_root.as_ref().unwrap(),
        &project,
        &presenter.model.conversation.workspace_draft,
    );
    git::git(path, &["branch", planned.branch.as_deref().unwrap()]).unwrap();
    assert!(presenter.submit("background worktree", "claude"));
    finish_workspace_operation(&mut presenter);
    assert!(presenter.model[context].workspace_retry);
    presenter.new_task();
    presenter.select_workspace_kind(WorkspaceKind::Local);
    assert!(presenter.submit("foreground local task", "claude"));
    let foreground = last_start(&runner);
    assert!(presenter.retry_workspace_start_in(context));
    finish_workspace_operation(&mut presenter);
    let ModelCatalogState::Loading { request_id, .. } = presenter.model[context].model_catalog
    else {
        panic!("background catalog")
    };
    runner.emit(Event::ModelCatalogLoaded {
        request_id,
        harness: HarnessKind::Claude,
        models: claude_aliases(),
    });
    presenter.drain_events();
    assert_eq!(
        presenter.model.conversation.active_run,
        Some(foreground.run_id)
    );
    assert!(presenter.model[context].active_run.is_some());
    assert_eq!(presenter.model.active_run_count(), 2);
    assert_ne!(last_start(&runner).cwd, foreground.cwd);
}

#[test]
fn worktree_uses_the_selected_local_branch_and_keeps_the_user_prompt_in_history() {
    use crate::{infrastructure::git, model::workspace::WorkspaceKind};
    let (directory, project) = git::tests::repository_fixture();
    let path = Path::new(&project.canonical_path);
    git::git(path, &["branch", "release"]).unwrap();
    std::fs::write(path.join("tracked.txt"), "new main\n").unwrap();
    git::git(path, &["commit", "-am", "advance main"]).unwrap();
    // A tag with the same name must not change which source branch is checked out.
    git::git(
        path,
        &["tag", "--no-sign", "-m", "shadow branch name", "release"],
    )
    .unwrap();
    let (mut presenter, runner, _fixture) = fixture();
    presenter.worktree_root = Ok(directory.path().canonicalize().unwrap().join("worktrees"));
    presenter.open_project(path);
    assert_eq!(
        presenter.workspace_base_branches().unwrap(),
        ["main", "release"]
    );
    assert_eq!(presenter.model.conversation.workspace_draft.base, "main");
    presenter.select_workspace_kind(WorkspaceKind::Worktree);
    presenter.select_workspace_base("HEAD".into());
    assert_eq!(presenter.model.conversation.workspace_draft.base, "main");
    presenter.select_workspace_base("release".into());
    assert!(presenter.submit("fix release", "claude"));
    presenter.select_workspace_base("main".into());
    assert_eq!(presenter.model.conversation.workspace_draft.base, "release");
    finish_workspace_operation(&mut presenter);
    emit_current_catalog(&presenter, &runner, claude_aliases());
    presenter.drain_events();
    let start = last_start(&runner);
    assert_eq!(
        std::fs::read_to_string(Path::new(&start.cwd).join("tracked.txt")).unwrap(),
        "base\n"
    );
    let workspace = presenter
        .model
        .conversation
        .selected_workspace
        .as_ref()
        .unwrap();
    assert_eq!(
        workspace.base_sha.as_deref(),
        Some(
            git::git(path, &["rev-parse", "refs/heads/release"])
                .unwrap()
                .trim()
        )
    );
    assert!(
        start
            .prompt
            .starts_with("fix release\n\n<nexus_worktree_context>")
    );
    assert!(start.prompt.contains(workspace.branch.as_deref().unwrap()));
    assert!(start.prompt.contains("git branch -m <name>"));
    assert_eq!(
        presenter.storage.messages(start.task_id).unwrap()[0].content,
        "fix release"
    );
    assert_eq!(
        presenter
            .model
            .conversation
            .tasks
            .iter()
            .find(|task| task.id == start.task_id)
            .unwrap()
            .title,
        "fix release"
    );
}

#[test]
fn worktree_defaults_to_remote_main_and_resolves_its_exact_commit() {
    use crate::{infrastructure::git, model::workspace::WorkspaceKind};
    let (directory, project) = git::tests::repository_fixture();
    let path = Path::new(&project.canonical_path);
    let remote_sha = git::git(path, &["rev-parse", "HEAD"]).unwrap();
    git::git(
        path,
        &["update-ref", "refs/remotes/origin/main", remote_sha.trim()],
    )
    .unwrap();
    git::git(
        path,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    )
    .unwrap();
    std::fs::write(path.join("tracked.txt"), "local main\n").unwrap();
    git::git(path, &["commit", "-am", "advance local main"]).unwrap();
    // A local branch with the same display name must not shadow the remote baseline.
    git::git(path, &["branch", "origin/main"]).unwrap();
    let (mut presenter, runner, _fixture) = fixture();
    presenter.worktree_root = Ok(directory.path().canonicalize().unwrap().join("worktrees"));
    presenter.open_project(path);
    assert_eq!(
        presenter.workspace_base_branches().unwrap(),
        ["main", "origin/main"]
    );
    assert_eq!(
        presenter.model.conversation.workspace_draft.base,
        "origin/main"
    );
    presenter.select_workspace_kind(WorkspaceKind::Worktree);
    presenter.select_workspace_base("main".into());
    presenter.refresh_workspace_branches();
    assert_eq!(presenter.model.conversation.workspace_draft.base, "main");
    presenter.select_workspace_base("origin/main".into());
    assert!(presenter.submit("start from remote main", "claude"));
    finish_workspace_operation(&mut presenter);
    emit_current_catalog(&presenter, &runner, claude_aliases());
    presenter.drain_events();
    let start = last_start(&runner);
    assert_eq!(
        std::fs::read_to_string(Path::new(&start.cwd).join("tracked.txt")).unwrap(),
        "base\n"
    );
    assert_eq!(
        presenter
            .model
            .conversation
            .selected_workspace
            .as_ref()
            .unwrap()
            .base_sha
            .as_deref(),
        Some(remote_sha.trim())
    );
    assert_eq!(
        std::fs::read_to_string(path.join("tracked.txt")).unwrap(),
        "local main\n"
    );
}

#[test]
fn worktree_source_selection_handles_non_git_empty_and_detached_projects() {
    use crate::{infrastructure::git, model::workspace::WorkspaceKind};
    let (mut presenter, runner, directory) = fixture();
    presenter.select_workspace_kind(WorkspaceKind::Worktree);
    assert_eq!(
        presenter.model.conversation.workspace_draft.kind,
        WorkspaceKind::Local
    );
    git::git(directory.path(), &["init", "-b", "main"]).unwrap();
    presenter.open_project(directory.path());
    presenter.select_workspace_kind(WorkspaceKind::Worktree);
    assert!(presenter.workspace_base_branches().unwrap().is_empty());
    assert!(presenter.model.conversation.workspace_draft.base.is_empty());
    emit_current_catalog(&presenter, &runner, claude_aliases());
    presenter.drain_events();
    assert!(!presenter.submit("needs a source commit", "claude"));
    assert_eq!(
        presenter.model.latest_log_text(presenter.model.language),
        "请选择来源分支。"
    );
    let (_repository, project) = git::tests::repository_fixture();
    let path = Path::new(&project.canonical_path);
    git::git(path, &["checkout", "--detach", "HEAD"]).unwrap();
    presenter.open_project(path);
    assert_eq!(presenter.model.conversation.workspace_draft.base, "main");
}

#[test]
fn workspace_branch_refresh_syncs_external_switches_while_a_task_runs() {
    use crate::infrastructure::git;
    let (_directory, project) = git::tests::repository_fixture();
    let path = Path::new(&project.canonical_path);
    let (mut presenter, _runner, _fixture) = fixture();
    presenter.open_project(path);
    assert!(presenter.submit("local task", "claude"));
    let run_id = presenter.model.conversation.active_run.unwrap();
    presenter.refresh_workspace_branches();
    assert_eq!(
        presenter.model.conversation.workspace_branch.as_deref(),
        Some("main")
    );
    // Switch branches outside the app (e.g. from VSCode), then refresh.
    git::git(path, &["switch", "-c", "feature"]).unwrap();
    presenter.refresh_workspace_branches();
    assert_eq!(
        presenter.model.conversation.workspace_branch.as_deref(),
        Some("feature")
    );
    git::git(path, &["switch", "main"]).unwrap();
    presenter.refresh_workspace_branches();
    assert_eq!(
        presenter.model.conversation.workspace_branch.as_deref(),
        Some("main")
    );
    // Refresh is read-only: the running task is untouched.
    assert_eq!(presenter.model.conversation.active_run, Some(run_id));
    assert!(!presenter.model.workspace_busy);
}

#[test]
fn workspace_branch_refresh_revalidates_the_worktree_base_branch() {
    use crate::{infrastructure::git, model::workspace::WorkspaceKind};
    let (_directory, project) = git::tests::repository_fixture();
    let path = Path::new(&project.canonical_path);
    git::git(path, &["branch", "release"]).unwrap();
    git::git(path, &["update-ref", "refs/remotes/origin/main", "HEAD"]).unwrap();
    let (mut presenter, _runner, _fixture) = fixture();
    presenter.open_project(path);
    presenter.select_workspace_kind(WorkspaceKind::Worktree);
    presenter.select_workspace_base("release".into());
    presenter.refresh_workspace_branches();
    assert_eq!(presenter.model.conversation.workspace_draft.base, "release");
    assert!(
        presenter
            .workspace_base_branches()
            .unwrap()
            .contains(&"release".to_owned())
    );
    // A branch deleted externally falls back to the preferred remote baseline.
    git::git(path, &["branch", "-D", "release"]).unwrap();
    presenter.refresh_workspace_branches();
    assert_eq!(
        presenter.model.conversation.workspace_draft.base,
        "origin/main"
    );
}

#[test]
fn workspace_branch_refresh_degrades_without_git() {
    let (mut presenter, _runner, _directory) = fixture();
    presenter.refresh_workspace_branches();
    assert!(!presenter.model.conversation.project_is_git);
    assert_eq!(presenter.model.conversation.workspace_branch, None);
}

#[test]
fn worktree_task_binds_cwd_session_and_preserves_history_after_external_removal() {
    use crate::{
        infrastructure::git,
        model::workspace::{WorkspaceKind, WorkspaceStatus},
    };
    let (directory, project) = git::tests::repository_fixture();
    let (mut presenter, runner, _fixture) = fixture();
    presenter.worktree_root = Ok(directory.path().canonicalize().unwrap().join("worktrees"));
    presenter.open_project(Path::new(&project.canonical_path));
    presenter.select_workspace_kind(WorkspaceKind::Worktree);
    assert!(!presenter.worktree_root.as_ref().unwrap().exists());
    assert!(presenter.submit("isolated task", "claude"));
    finish_workspace_operation(&mut presenter);
    let workspace = presenter
        .model
        .conversation
        .selected_workspace
        .clone()
        .unwrap();
    assert_eq!(workspace.status, WorkspaceStatus::Ready);
    assert!(workspace.branch.as_ref().unwrap().starts_with("feat/nx-"));
    let ModelCatalogState::Loading { request_id, .. } = presenter.model.conversation.model_catalog
    else {
        panic!("catalog must use new cwd")
    };
    runner.emit(Event::ModelCatalogLoaded {
        request_id,
        harness: HarnessKind::Claude,
        models: claude_aliases(),
    });
    presenter.drain_events();
    let start = last_start(&runner);
    assert_eq!(start.cwd, workspace.path);
    assert_eq!(start.task_id, workspace.task_id.unwrap());
    assert_ne!(start.cwd, project.canonical_path);
    runner.emit(Event::RunSessionStarted {
        run_id: start.run_id,
        session_id: "isolated-session".into(),
    });
    git::git(
        Path::new(&start.cwd),
        &["branch", "-m", "fix/meaningful-task"],
    )
    .unwrap();
    runner.emit(Event::RunExited {
        run_id: start.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    assert_eq!(
        presenter
            .storage
            .workspace(workspace.id)
            .unwrap()
            .unwrap()
            .branch
            .as_deref(),
        Some("fix/meaningful-task")
    );
    assert_eq!(
        presenter.model.conversation.workspace_branch.as_deref(),
        Some("fix/meaningful-task")
    );
    presenter.select_task(start.task_id);
    assert!(presenter.submit("continue", "claude"));
    let resumed = last_start(&runner);
    assert_eq!(resumed.cwd, start.cwd);
    assert_eq!(resumed.session_id.as_deref(), Some("isolated-session"));
    assert_eq!(resumed.prompt, "continue");
    git::git(
        Path::new(&project.canonical_path),
        &[
            "worktree",
            "remove",
            &git::git_path_argument(&workspace.path),
        ],
    )
    .unwrap();
    presenter.reload_workspaces();
    assert_eq!(
        presenter
            .model
            .conversation
            .selected_workspace
            .as_ref()
            .unwrap()
            .status,
        WorkspaceStatus::Missing
    );
    runner.emit(Event::RunExited {
        run_id: resumed.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    assert!(!presenter.submit("must not run in project", "claude"));
    assert_eq!(presenter.storage.messages(start.task_id).unwrap().len(), 2);
    assert!(presenter.delete_task(start.task_id));
    assert!(presenter.storage.workspace(workspace.id).unwrap().is_some());
    assert!(!Path::new(&workspace.path).exists());
}
