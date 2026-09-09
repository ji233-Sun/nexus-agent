use std::{
    borrow::Cow,
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command as SystemCommand,
    sync::{Arc, LazyLock, Mutex},
};

use crate::model::workspace::{Workspace, WorkspaceDraft, WorkspaceKind, WorkspaceStatus};
pub(crate) mod changes;
use anyhow::{Context as _, Result, ensure};

type RepositoryLocks = Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>>;
static REPOSITORY_LOCKS: LazyLock<RepositoryLocks> = LazyLock::new(Mutex::default);

#[derive(Debug, Clone)]
pub(crate) struct Checkout {
    pub(crate) path: PathBuf,
    pub(crate) branch: Option<String>,
    pub(crate) locked: bool,
}

pub(crate) fn git(path: &Path, args: &[&str]) -> Result<String> {
    let output = SystemCommand::new("git")
        .args(["--no-pager", "--literal-pathspecs"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .current_dir(path)
        .output()
        .context("执行 Git")?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8(output.stdout).context("Git 返回了无法显示的路径或内容")
}

pub(crate) fn git_path_argument(path: &str) -> Cow<'_, str> {
    // Git for Windows rewrites verbatim paths to //?/..., breaking worktree creation.
    // Keep canonical paths for identity checks and adapt only explicit CLI path arguments.
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};

        if let Some(Component::Prefix(prefix)) = Path::new(path).components().next() {
            match prefix.kind() {
                Prefix::VerbatimDisk(_) => return Cow::Borrowed(&path[4..]),
                Prefix::VerbatimUNC(..) => return Cow::Owned(format!(r"\\{}", &path[8..])),
                _ => {}
            }
        }
    }
    Cow::Borrowed(path)
}

pub(crate) fn repository(path: &Path) -> Result<PathBuf> {
    let common = git(
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    Path::new(common.trim_end_matches(['\r', '\n']))
        .canonicalize()
        .context("读取 Git common directory")
}

pub(crate) fn checkout_path(path: &Path) -> Result<PathBuf> {
    match git(path, &["rev-parse", "--show-toplevel"]) {
        Ok(root) => Path::new(root.trim_end_matches(['\r', '\n']))
            .canonicalize()
            .map_err(Into::into),
        Err(_) => path.canonicalize().context("读取执行目录"),
    }
}

pub(crate) fn current_branch(path: &Path) -> Option<String> {
    git(path, &["symbolic-ref", "--quiet", "HEAD"])
        .ok()
        .and_then(|value| {
            value
                .trim_end()
                .strip_prefix("refs/heads/")
                .map(str::to_owned)
        })
}

pub(crate) fn local_branches(path: &Path) -> Result<Vec<String>> {
    Ok(git(
        path,
        &[
            "for-each-ref",
            "--format=%(refname:lstrip=2)",
            "refs/heads/",
        ],
    )?
    .lines()
    .map(str::to_owned)
    .collect())
}

fn with_repository<T>(path: &Path, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    let key = repository(path)?;
    let lock = REPOSITORY_LOCKS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .entry(key)
        .or_default()
        .clone();
    let _guard = lock.lock().unwrap_or_else(|error| error.into_inner());
    operation()
}

pub(crate) fn checkouts(path: &Path) -> Result<Vec<Checkout>> {
    let output = git(path, &["worktree", "list", "--porcelain", "-z"])?;
    let mut result = Vec::new();
    let mut current: Option<Checkout> = None;
    for field in output.split('\0') {
        if let Some(path) = field.strip_prefix("worktree ") {
            if let Some(checkout) = current.take() {
                result.push(checkout);
            }
            current = Some(Checkout {
                path: Path::new(path)
                    .canonicalize()
                    .unwrap_or_else(|_| path.into()),
                branch: None,
                locked: false,
            });
        } else if let Some(checkout) = current.as_mut() {
            if let Some(branch) = field.strip_prefix("branch refs/heads/") {
                checkout.branch = Some(branch.to_owned());
            }
            if field == "locked" || field.starts_with("locked ") {
                checkout.locked = true;
            }
        }
    }
    if let Some(checkout) = current {
        result.push(checkout);
    }
    Ok(result)
}

pub(crate) fn planned_workspace(
    root: &Path,
    project: &nexus_domain::Project,
    draft: &WorkspaceDraft,
) -> Workspace {
    Workspace {
        id: draft.task_id,
        project_id: project.id,
        task_id: Some(draft.task_id),
        path: root
            .join(project.id.to_string())
            .join(draft.task_id.to_string())
            .to_string_lossy()
            .into_owned(),
        repository: None,
        kind: WorkspaceKind::Worktree,
        managed: true,
        external: false,
        base_sha: None,
        branch: Some(format!(
            "feat/nx-{}",
            &draft.task_id.simple().to_string()[..8]
        )),
        merge_target: None,
        status: WorkspaceStatus::Creating,
        merge: None,
        initialization: None,
    }
}

pub(crate) fn resolve_workspace(
    project: &Path,
    mut workspace: Workspace,
    base: &str,
) -> Result<Workspace> {
    with_repository(project, || {
        let branch = workspace.branch.as_deref().context("缺少任务分支")?;
        ensure!(!branch.starts_with('-'), "分支名不能以 - 开头");
        git(project, &["check-ref-format", "--branch", branch])?;
        let base_sha = git(
            project,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{base}^{{commit}}"),
            ],
        )?
        .trim()
        .to_owned();
        workspace.repository = Some(repository(project)?.to_string_lossy().into_owned());
        workspace.base_sha = Some(base_sha);
        Ok(workspace)
    })
}

pub(crate) fn create_worktree(project: &Path, mut workspace: Workspace) -> Result<Workspace> {
    with_repository(project, || {
        let common = repository(project)?;
        ensure!(
            workspace.repository.as_deref() == common.to_str(),
            "仓库身份已改变"
        );
        let target = Path::new(&workspace.path);
        ensure!(!target.exists(), "任务目录已存在，请先恢复或清理该目录");
        std::fs::create_dir_all(target.parent().context("任务目录无父目录")?)?;
        ensure!(
            target
                .parent()
                .unwrap()
                .canonicalize()?
                .join(target.file_name().unwrap())
                == target,
            "任务目录被重定向，拒绝创建"
        );
        git(
            project,
            &[
                "worktree",
                "add",
                "-b",
                workspace.branch.as_deref().context("缺少任务分支")?,
                &git_path_argument(&workspace.path),
                workspace.base_sha.as_deref().context("缺少创建基准 SHA")?,
            ],
        )?;
        workspace.status = WorkspaceStatus::Ready;
        Ok(workspace)
    })
}

pub(crate) fn validate_workspace(workspace: &Workspace) -> Result<()> {
    ensure!(
        workspace.status == WorkspaceStatus::Ready,
        "任务目录已清理、缺失或尚未创建，无法继续运行；请新建任务"
    );
    let path = Path::new(&workspace.path);
    ensure!(
        path.is_dir(),
        "任务目录不存在，无法继续运行；不会回退到项目目录"
    );
    if workspace.managed {
        let canonical = path.canonicalize()?;
        ensure!(canonical == path, "任务目录被重定向，拒绝使用");
        let common = repository(path)?;
        ensure!(
            workspace.repository.as_deref() == common.to_str(),
            "任务目录已不属于原仓库"
        );
        ensure!(
            checkouts(path)?.iter().any(|entry| entry.path == canonical),
            "任务目录已不在 Git Worktree 列表中"
        );
    }
    Ok(())
}

// Reconcile interrupted creation and external removal without deleting any files.
pub(crate) fn recover_workspace(project: &Path, mut workspace: Workspace) -> Workspace {
    changes::reconcile_merge(&mut workspace);
    if workspace.status == WorkspaceStatus::Removed || !workspace.managed {
        return workspace;
    }
    let entry = checkouts(project).ok().and_then(|entries| {
        entries
            .into_iter()
            .find(|entry| entry.path == Path::new(&workspace.path))
    });
    if let Some(entry) = entry.filter(|entry| entry.path.is_dir()) {
        if workspace.status == WorkspaceStatus::Creating {
            // The creation SHA is persisted before adding the worktree. Never infer it
            // from a branch that may have moved since the interruption.
            if workspace.base_sha.is_none()
                || workspace.repository.is_none()
                || entry.branch != workspace.branch
            {
                workspace.status = WorkspaceStatus::Missing;
                return workspace;
            }
        }
        workspace.status = WorkspaceStatus::Ready;
        if validate_workspace(&workspace).is_err() {
            workspace.status = WorkspaceStatus::Missing;
        } else if entry.branch != workspace.branch
            && entry.branch.is_some()
            && local_branches(project).is_ok_and(|branches| {
                !workspace
                    .branch
                    .as_ref()
                    .is_some_and(|branch| branches.contains(branch))
            })
        {
            // A rename removes the old ref. Keep the write guard for branch switches.
            workspace.branch = entry.branch;
        }
    } else {
        workspace.status = WorkspaceStatus::Missing;
    }
    workspace
}

pub(crate) fn is_git_dirty(path: &Path) -> bool {
    SystemCommand::new("git")
        .args(["status", "--porcelain"])
        .current_dir(path)
        .output()
        .map(|output| output.status.success() && !output.stdout.is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use uuid::Uuid;

    pub(crate) fn repository_fixture() -> (tempfile::TempDir, nexus_domain::Project) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("repo");
        std::fs::create_dir(&path).unwrap();
        git(&path, &["init", "-b", "main"]).unwrap();
        git(&path, &["config", "user.name", "Nexus Test"]).unwrap();
        git(&path, &["config", "user.email", "nexus@example.invalid"]).unwrap();
        // Fixture bytes must not depend on the host's Git line-ending policy.
        git(&path, &["config", "core.autocrlf", "false"]).unwrap();
        std::fs::write(path.join("tracked.txt"), "base\n").unwrap();
        git(&path, &["add", "."]).unwrap();
        git(&path, &["commit", "-m", "base"]).unwrap();
        let now = chrono::Utc::now();
        let project = nexus_domain::Project {
            id: Uuid::new_v4(),
            display_name: "repo".into(),
            canonical_path: path.canonicalize().unwrap().to_string_lossy().into_owned(),
            created_at: now,
            last_opened_at: now,
        };
        (directory, project)
    }

    fn create(project: &Path, workspace: Workspace) -> Workspace {
        create_worktree(
            project,
            resolve_workspace(project, workspace, "HEAD").unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn local_commit_generation_matches_selected_working_tree_contents() {
        let (_directory, project) = repository_fixture();
        let cwd = Path::new(&project.canonical_path);
        let workspace = Workspace::local(&project);
        std::fs::write(cwd.join("tracked.txt"), "intermediate staged\n").unwrap();
        git(cwd, &["add", "tracked.txt"]).unwrap();
        std::fs::write(cwd.join("tracked.txt"), "final selected\n").unwrap();
        std::fs::write(cwd.join("unrelated.txt"), "unrelated secret\n").unwrap();
        git(cwd, &["add", "unrelated.txt"]).unwrap();
        std::fs::write(cwd.join("new space.txt"), "new selected\n").unwrap();
        let review = changes::review(&workspace).unwrap();
        let files = vec!["tracked.txt".into(), "new space.txt".into()];
        let diff = changes::selected_diff(&workspace, &review, &files).unwrap();
        assert!(diff.contains("+final selected"));
        assert!(diff.contains("new space.txt\nnew selected"));
        assert!(!diff.contains("intermediate staged"));
        assert!(!diff.contains("unrelated"));
        assert!(changes::selected_diff(&workspace, &review, &[]).is_err());
        assert!(changes::selected_diff(&workspace, &review, &["missing".into()]).is_err());
        let message = "fix: Keep selected contents\n\nPreserve unrelated staging.";
        changes::commit_files(&workspace, &review, &files, message).unwrap();
        assert_eq!(
            git(cwd, &["log", "-1", "--format=%B"]).unwrap().trim(),
            message
        );
        assert_eq!(
            git(cwd, &["diff", "--cached", "--name-only"])
                .unwrap()
                .trim(),
            "unrelated.txt"
        );
        assert!(changes::selected_diff(&workspace, &review, &files).is_err());
    }

    #[test]
    fn review_and_selected_commits_include_all_task_changes_without_committing_other_staged_files()
    {
        let (directory, project) = repository_fixture();
        let path = Path::new(&project.canonical_path);
        let root = directory.path().canonicalize().unwrap().join("worktrees");
        let workspace = create(
            path,
            planned_workspace(&root, &project, &WorkspaceDraft::default()),
        );
        let cwd = Path::new(&workspace.path);
        std::fs::write(cwd.join("tracked.txt"), "committed\n").unwrap();
        git(cwd, &["commit", "-am", "agent commit"]).unwrap();
        std::fs::write(cwd.join("tracked.txt"), "unstaged\n").unwrap();
        std::fs::write(cwd.join("staged.txt"), "staged\n").unwrap();
        git(cwd, &["add", "staged.txt"]).unwrap();
        std::fs::write(cwd.join("new space.txt"), "untracked\n").unwrap();
        let review = changes::review(&workspace).unwrap();
        assert!(review.committed.contains("+committed"));
        assert!(review.staged.contains("+staged"));
        assert!(review.unstaged.contains("+unstaged"));
        assert_eq!(
            review.untracked,
            vec![("new space.txt".into(), "untracked\n".into())]
        );
        let next = changes::commit_files(
            &workspace,
            &review,
            &["tracked.txt".into(), "new space.txt".into()],
            "selected files",
        )
        .unwrap();
        assert_eq!(
            git(cwd, &["diff", "--cached", "--name-only"])
                .unwrap()
                .trim(),
            "staged.txt"
        );
        assert!(
            !git(cwd, &["show", "--format=", "--name-only", "HEAD"])
                .unwrap()
                .contains("staged.txt")
        );
        assert!(
            changes::commit_files(&workspace, &review, &["staged.txt".into()], "stale").is_err()
        );
        changes::commit_files(&workspace, &next, &["staged.txt".into()], "remaining file").unwrap();
        std::fs::write(cwd.join("asset.bin"), [255, 0, 1]).unwrap();
        let binary_review = changes::review(&workspace).unwrap();
        std::fs::write(cwd.join("asset.bin"), [255, 0, 2]).unwrap();
        assert!(
            changes::commit_files(
                &workspace,
                &binary_review,
                &["asset.bin".into()],
                "stale binary"
            )
            .is_err()
        );
        let binary_review = changes::review(&workspace).unwrap();
        changes::commit_files(
            &workspace,
            &binary_review,
            &["asset.bin".into()],
            "reviewed binary",
        )
        .unwrap();
        git(path, &["branch", "receive"]).unwrap();
        let plan = changes::plan_merge(path, &workspace, "receive").unwrap();
        assert!(plan.diff.contains("new space.txt"));
        let workspace = changes::merge(path, workspace, &plan).unwrap();
        assert_eq!(current_branch(path).as_deref(), Some("receive"));
        assert_eq!(
            std::fs::read_to_string(path.join("tracked.txt")).unwrap(),
            "unstaged\n"
        );
        assert_eq!(workspace.merge_target.as_deref(), Some("receive"));
    }

    #[test]
    fn merge_conflicts_can_be_aborted_or_resolved_and_reject_a_stale_preview() {
        let (directory, project) = repository_fixture();
        let path = Path::new(&project.canonical_path);
        let root = directory.path().canonicalize().unwrap().join("worktrees");
        let workspace = create(
            path,
            planned_workspace(&root, &project, &WorkspaceDraft::default()),
        );
        let cwd = Path::new(&workspace.path);
        std::fs::write(cwd.join("tracked.txt"), "task\n").unwrap();
        git(cwd, &["commit", "-am", "task change"]).unwrap();
        let stale = changes::plan_merge(path, &workspace, "main").unwrap();
        std::fs::write(path.join("tracked.txt"), "target\n").unwrap();
        git(path, &["commit", "-am", "target change"]).unwrap();
        assert!(changes::merge(path, workspace.clone(), &stale).is_err());
        let plan = changes::plan_merge(path, &workspace, "main").unwrap();
        let conflicted = changes::merge(path, workspace, &plan).unwrap();
        assert!(conflicted.merge.is_some());
        let review = changes::review(&conflicted).unwrap();
        assert_eq!(review.conflicts, vec!["tracked.txt"]);
        assert!(changes::finish_merge(conflicted.clone(), &review, false).is_err());
        let aborted = changes::finish_merge(conflicted, &review, true).unwrap();
        assert!(aborted.merge.is_none());
        assert_eq!(
            std::fs::read_to_string(path.join("tracked.txt")).unwrap(),
            "target\n"
        );
        let plan = changes::plan_merge(path, &aborted, "main").unwrap();
        let conflicted = changes::merge(path, aborted, &plan).unwrap();
        std::fs::write(path.join("tracked.txt"), "resolved\n").unwrap();
        git(path, &["add", "tracked.txt"]).unwrap();
        let review = changes::review(&conflicted).unwrap();
        assert!(review.conflicts.is_empty());
        assert!(review.resolution_diff.contains("resolved"));
        std::fs::write(path.join("tracked.txt"), "different staged resolution\n").unwrap();
        git(path, &["add", "tracked.txt"]).unwrap();
        std::fs::write(path.join("tracked.txt"), "resolved\n").unwrap();
        assert!(changes::finish_merge(conflicted.clone(), &review, false).is_err());
        let unstaged = changes::review(&conflicted).unwrap();
        assert!(changes::finish_merge(conflicted.clone(), &unstaged, false).is_err());
        git(path, &["add", "tracked.txt"]).unwrap();
        let review = changes::review(&conflicted).unwrap();
        let merged = changes::finish_merge(conflicted, &review, false).unwrap();
        assert_eq!(merged.merge_target.as_deref(), Some("main"));
        assert!(merged.merge.is_none());
    }

    #[test]
    fn interrupted_creation_and_external_removal_preserve_history_without_recreating_directories() {
        let (directory, project) = repository_fixture();
        let path = Path::new(&project.canonical_path);
        let root = directory
            .path()
            .canonicalize()
            .unwrap()
            .join("worktrees 中文 空格");
        let planned = resolve_workspace(
            path,
            planned_workspace(&root, &project, &WorkspaceDraft::default()),
            "HEAD",
        )
        .unwrap();
        let interrupted = recover_workspace(path, planned.clone());
        assert_eq!(interrupted.status, WorkspaceStatus::Missing);
        assert!(validate_workspace(&interrupted).is_err());
        assert!(!Path::new(&interrupted.path).exists());
        let ready = create_worktree(path, planned.clone()).unwrap();
        let recovered = recover_workspace(path, planned);
        assert_eq!(recovered.status, WorkspaceStatus::Ready);
        assert_eq!(recovered.base_sha, ready.base_sha);
        git(
            path,
            &["worktree", "remove", &git_path_argument(&ready.path)],
        )
        .unwrap();
        let missing = recover_workspace(path, ready);
        assert_eq!(missing.status, WorkspaceStatus::Missing);
        assert!(validate_workspace(&missing).is_err());
        assert!(!Path::new(&missing.path).exists());
        assert!(
            git(
                path,
                &[
                    "show-ref",
                    "--verify",
                    &format!("refs/heads/{}", missing.branch.unwrap())
                ]
            )
            .is_ok()
        );
    }

    #[test]
    fn worktrees_use_committed_head_and_stable_independent_directories() {
        let (directory, project) = repository_fixture();
        let path = Path::new(&project.canonical_path);
        std::fs::write(path.join("tracked.txt"), "uncommitted\n").unwrap();
        let root = directory.path().canonicalize().unwrap().join("worktrees");
        let first = create(
            path,
            planned_workspace(&root, &project, &WorkspaceDraft::default()),
        );
        let second = create(
            path,
            planned_workspace(&root, &project, &WorkspaceDraft::default()),
        );
        assert_ne!(first.path, second.path);
        assert_ne!(first.branch, second.branch);
        assert_eq!(
            repository(Path::new(&first.path)).unwrap(),
            repository(path).unwrap()
        );
        assert!(Path::new(&first.path).join(".git").is_file());
        std::fs::write(Path::new(&first.path).join("tracked.txt"), "first\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(Path::new(&second.path).join("tracked.txt")).unwrap(),
            "base\n"
        );
        assert_eq!(
            std::fs::read_to_string(path.join("tracked.txt")).unwrap(),
            "uncommitted\n"
        );
        validate_workspace(&first).unwrap();
        let first = recover_workspace(path, first);
        assert_eq!(first.status, WorkspaceStatus::Ready);
        let cwd = Path::new(&first.path);
        git(cwd, &["checkout", "-b", "other-branch"]).unwrap();
        let switched = recover_workspace(path, first.clone());
        assert_eq!(switched.branch, first.branch);
        assert!(changes::plan_merge(path, &switched, "main").is_err());
        git(cwd, &["checkout", first.branch.as_deref().unwrap()]).unwrap();
        git(cwd, &["branch", "-m", "fix/renamed-task"]).unwrap();
        let renamed = recover_workspace(path, first);
        assert_eq!(renamed.branch.as_deref(), Some("fix/renamed-task"));
    }
}
