use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command as SystemCommand,
    sync::{Arc, LazyLock, Mutex},
};

use crate::model::workspace::{Workspace, WorkspaceDraft, WorkspaceKind, WorkspaceStatus};
use anyhow::{Context as _, Result, bail, ensure};

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
    git(path, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()
        .map(|value| value.trim_end().to_owned())
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
                path: path.into(),
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
        base_sha: None,
        branch: Some(draft.branch.clone()),
        merge_target: None,
        status: WorkspaceStatus::Creating,
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
                &workspace.path,
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
        "任务目录已清理、缺失或尚未创建，无法继续运行；请新建任务或显式恢复目录"
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
        }
    } else {
        workspace.status = WorkspaceStatus::Missing;
    }
    workspace
}

pub(crate) fn cleanup_worktree(
    root: &Path,
    project: &Path,
    mut workspace: Workspace,
    target: &str,
) -> Result<Workspace> {
    if !workspace.managed {
        workspace.status = WorkspaceStatus::Removed;
        return Ok(workspace);
    }
    with_repository(project, || {
        let task = workspace.task_id.context("目录没有 Nexus 任务归属")?;
        let expected = root
            .join(workspace.project_id.to_string())
            .join(task.to_string());
        ensure!(
            Path::new(&workspace.path) == expected,
            "仅允许清理 Nexus 创建的任务目录"
        );
        validate_workspace(&workspace)?;
        let path = Path::new(&workspace.path);
        ensure!(
            checkouts(project)?
                .iter()
                .any(|entry| entry.path == path && !entry.locked),
            "Worktree 被锁定或已解除关联"
        );
        let status = git(
            path,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--ignored",
            ],
        )?;
        ensure!(
            status.is_empty(),
            "存在未提交修改、未跟踪或被忽略文件，请先处理：\n{}",
            status.replace('\0', "\n")
        );
        ensure!(
            current_branch(path) == workspace.branch,
            "实际分支与任务分支不一致，请先切回任务分支"
        );
        let target_ref = format!("refs/heads/{target}");
        let target_sha = git(
            project,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{target_ref}^{{commit}}"),
            ],
        )?;
        if git(
            path,
            &["merge-base", "--is-ancestor", "HEAD", target_sha.trim()],
        )
        .is_err()
        {
            bail!("任务仍有未合入 {target} 的提交，请先接收成果");
        }
        git(project, &["worktree", "remove", &workspace.path])?;
        workspace.status = WorkspaceStatus::Removed;
        workspace.merge_target = Some(target.to_owned());
        Ok(workspace)
    })
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
        assert_eq!(
            recover_workspace(path, first).status,
            WorkspaceStatus::Ready
        );
    }

    #[test]
    fn cleanup_blocks_ignored_files_and_unmerged_commits_and_preserves_branch() {
        let (directory, project) = repository_fixture();
        let path = Path::new(&project.canonical_path);
        let root = directory.path().canonicalize().unwrap().join("worktrees");
        let workspace = create(
            path,
            planned_workspace(&root, &project, &WorkspaceDraft::default()),
        );
        let cwd = Path::new(&workspace.path);
        std::fs::write(cwd.join(".gitignore"), ".env\n").unwrap();
        git(cwd, &["add", ".gitignore"]).unwrap();
        git(cwd, &["commit", "-m", "ignore env"]).unwrap();
        assert!(
            cleanup_worktree(&root, path, workspace.clone(), "main")
                .unwrap_err()
                .to_string()
                .contains("未合入")
        );
        git(
            path,
            &["merge", "--ff-only", workspace.branch.as_deref().unwrap()],
        )
        .unwrap();
        std::fs::write(cwd.join(".env"), "private").unwrap();
        assert!(
            cleanup_worktree(&root, path, workspace.clone(), "main")
                .unwrap_err()
                .to_string()
                .contains("被忽略")
        );
        assert!(cwd.join(".env").exists());
        std::fs::remove_file(cwd.join(".env")).unwrap();
        let removed = cleanup_worktree(&root, path, workspace.clone(), "main").unwrap();
        assert_eq!(removed.status, WorkspaceStatus::Removed);
        assert!(!cwd.exists());
        assert!(
            git(
                path,
                &[
                    "show-ref",
                    "--verify",
                    &format!("refs/heads/{}", workspace.branch.unwrap())
                ]
            )
            .is_ok()
        );
        assert!(validate_workspace(&removed).is_err());
    }
}
