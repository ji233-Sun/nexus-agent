use super::*;
use crate::model::workspace::{MergePlan, MergeState, WorkspaceReview};
use sha2::{Digest as _, Sha256};

fn paths(output: String) -> Vec<String> {
    output
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect()
}

fn diff(path: &Path, args: &[&str]) -> Result<String> {
    let mut command = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--binary",
    ];
    command.extend(args);
    git(path, &command)
}

fn untracked_content(root: &Path, name: &str) -> Result<String> {
    let path = root.join(name);
    let metadata = std::fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() {
        return Ok(format!("symlink → {}", std::fs::read_link(path)?.display()));
    }
    if !metadata.is_file() {
        return Ok("目录 / 子模块".into());
    }
    let content = std::fs::read(path)?;
    Ok(String::from_utf8(content).unwrap_or_else(|error| {
        format!(
            "二进制文件（{} 字节，SHA-256: {:x}）",
            error.as_bytes().len(),
            Sha256::digest(error.as_bytes())
        )
    }))
}

pub(crate) fn review(workspace: &Workspace) -> Result<WorkspaceReview> {
    validate_workspace(workspace)?;
    let root = checkout_path(Path::new(&workspace.path))?;
    let cwd = root.as_path();
    let head = git(cwd, &["rev-parse", "HEAD"])?.trim().to_owned();
    let base = workspace.base_sha.as_deref().unwrap_or(&head);
    let mut dirty_paths = paths(git(
        cwd,
        &[
            "diff",
            "--no-ext-diff",
            "--no-renames",
            "--name-only",
            "-z",
            "HEAD",
            "--",
        ],
    )?);
    let untracked = paths(git(
        cwd,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?)
    .into_iter()
    .map(|name| Ok((name.clone(), untracked_content(cwd, &name)?)))
    .collect::<Result<Vec<_>>>()?;
    dirty_paths.extend(untracked.iter().map(|(name, _)| name.clone()));
    dirty_paths.sort();
    dirty_paths.dedup();
    let (conflicts, resolution_diff, resolution_staged) = if let Some(merge) = &workspace.merge {
        let target = Path::new(&merge.target_path);
        (
            paths(git(
                target,
                &["diff", "--name-only", "--diff-filter=U", "-z"],
            )?),
            diff(target, &[&merge.target_sha, "--"])?,
            diff(target, &["--cached", &merge.target_sha, "--"])?,
        )
    } else {
        (Vec::new(), String::new(), String::new())
    };
    Ok(WorkspaceReview {
        workspace_id: workspace.id,
        head: head.clone(),
        branch: current_branch(cwd),
        committed: diff(cwd, &[base, &head, "--"])?,
        staged: diff(cwd, &["--cached", "--"])?,
        unstaged: diff(cwd, &["--"])?,
        untracked,
        dirty_paths,
        target_branches: local_branches(cwd)?,
        conflicts,
        resolution_diff,
        resolution_staged,
    })
}

fn require_task_branch(workspace: &Workspace) -> Result<()> {
    ensure!(workspace.managed, "成果接收仅用于 Nexus 任务 Worktree");
    validate_workspace(workspace)?;
    ensure!(
        workspace.branch == current_branch(Path::new(&workspace.path)),
        "实际分支已改变，请切回任务分支后重试"
    );
    Ok(())
}

fn require_clean(path: &Path) -> Result<()> {
    ensure!(
        git(
            path,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"]
        )?
        .is_empty(),
        "目录存在未提交修改或未跟踪文件，请先处理：{}",
        path.display()
    );
    ensure!(
        git(path, &["rev-parse", "--verify", "MERGE_HEAD"]).is_err(),
        "目录已有待完成的合并"
    );
    Ok(())
}

pub(crate) fn commit_files(
    workspace: &Workspace,
    expected: &WorkspaceReview,
    files: &[String],
    message: &str,
) -> Result<WorkspaceReview> {
    let root = checkout_path(Path::new(&workspace.path))?;
    let cwd = root.as_path();
    with_repository(cwd, || {
        validate_workspace(workspace)?;
        if workspace.managed {
            require_task_branch(workspace)?;
        }
        ensure!(
            git(cwd, &["rev-parse", "--verify", "MERGE_HEAD"]).is_err(),
            "请先完成或中止已有合并"
        );
        ensure!(
            !message.trim().is_empty() && !files.is_empty(),
            "请选择文件并填写提交说明"
        );
        let current = review(workspace)?;
        ensure!(&current == expected, "变更已更新，请刷新审查后重新确认提交");
        ensure!(
            files.iter().all(|file| current.dirty_paths.contains(file)),
            "选择中包含未审查的文件"
        );
        let mut add = vec!["add", "--"];
        add.extend(files.iter().map(String::as_str));
        git(cwd, &add)?;
        // --only excludes unrelated staged files and keeps their index entries intact.
        let mut commit = vec!["commit", "--only", "-m", message, "--"];
        commit.extend(files.iter().map(String::as_str));
        git(cwd, &commit)?;
        review(workspace)
    })
}

pub(crate) fn selected_diff(
    workspace: &Workspace,
    expected: &WorkspaceReview,
    files: &[String],
) -> Result<String> {
    let root = checkout_path(Path::new(&workspace.path))?;
    with_repository(&root, || {
        ensure!(
            &review(workspace)? == expected,
            "变更已更新，请刷新后重新生成提交说明"
        );
        ensure!(
            !files.is_empty() && files.iter().all(|file| expected.dirty_paths.contains(file)),
            "请选择需要提交的文件"
        );
        // git commit --only takes the selected working-tree contents, including
        // unstaged edits. Generate from that same result relative to HEAD.
        let mut args = vec!["HEAD", "--"];
        args.extend(files.iter().map(String::as_str));
        let mut content = diff(&root, &args)?;
        for (name, text) in &expected.untracked {
            if files.contains(name) {
                content.push_str(&format!("\nNew file: {name}\n{text}\n"));
            }
        }
        ensure!(!content.trim().is_empty(), "所选文件没有可提交的变更");
        ensure!(
            content.len() <= nexus_protocol::MAX_COMMIT_DIFF_BYTES,
            "所选变更过大，请减少所选文件或手动填写提交说明"
        );
        ensure!(
            &review(workspace)? == expected,
            "变更已更新，请刷新后重新生成提交说明"
        );
        Ok(content)
    })
}

pub(crate) fn plan_merge(project: &Path, workspace: &Workspace, target: &str) -> Result<MergePlan> {
    require_task_branch(workspace)?;
    ensure!(!target.starts_with('-'), "目标分支名不能以 - 开头");
    ensure!(workspace.merge.is_none(), "请先完成或中止已有合并");
    ensure!(
        workspace.branch.as_deref() != Some(target),
        "不能将任务分支合入自身"
    );
    ensure!(
        repository(project)? == repository(Path::new(&workspace.path))?,
        "目标项目不属于任务仓库"
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
    )?
    .trim()
    .to_owned();
    let source_sha = git(Path::new(&workspace.path), &["rev-parse", "HEAD"])?
        .trim()
        .to_owned();
    let entries = checkouts(project)?;
    let checkout = entries
        .iter()
        .find(|entry| entry.branch.as_deref() == Some(target));
    ensure!(
        checkout.is_none_or(|entry| !entry.locked),
        "目标 checkout 已锁定"
    );
    let target_path = checkout
        .map(|entry| entry.path.clone())
        .unwrap_or(checkout_path(project)?);
    ensure!(
        target_path != Path::new(&workspace.path),
        "源目录与目标目录相同"
    );
    require_clean(Path::new(&workspace.path))?;
    require_clean(&target_path)?;
    Ok(MergePlan {
        workspace_id: workspace.id,
        source_branch: workspace.branch.clone().context("缺少任务分支")?,
        state: MergeState {
            source_sha: source_sha.clone(),
            target_sha: target_sha.clone(),
            target_branch: target.into(),
            target_path: target_path.to_string_lossy().into_owned(),
        },
        diff: diff(project, &[&format!("{target_sha}...{source_sha}"), "--"])?,
    })
}

pub(crate) fn merge(
    project: &Path,
    mut workspace: Workspace,
    plan: &MergePlan,
) -> Result<Workspace> {
    with_repository(project, || {
        ensure!(workspace.id == plan.workspace_id, "合并预览不属于此任务");
        workspace.merge = None;
        let current = plan_merge(project, &workspace, &plan.state.target_branch)?;
        ensure!(
            current.state == plan.state && current.diff == plan.diff,
            "分支或变更已更新，请重新预览合并"
        );
        let target = Path::new(&plan.state.target_path);
        if current_branch(target).as_deref() != Some(&plan.state.target_branch) {
            git(
                target,
                &["switch", "--no-overwrite-ignore", &plan.state.target_branch],
            )?;
        }
        require_clean(target)?;
        let result = git(
            target,
            &[
                "merge",
                "--no-edit",
                "--no-overwrite-ignore",
                &plan.state.source_sha,
            ],
        );
        if result.is_err() {
            if git(target, &["rev-parse", "--verify", "MERGE_HEAD"]).is_ok() {
                workspace.merge = Some(plan.state.clone());
                return Ok(workspace);
            }
            result?;
        }
        workspace.merge_target = Some(plan.state.target_branch.clone());
        Ok(workspace)
    })
}

fn validate_merge(workspace: &Workspace) -> Result<&MergeState> {
    let state = workspace.merge.as_ref().context("没有待处理的合并")?;
    let target = Path::new(&state.target_path);
    ensure!(
        repository(target)?.to_str() == workspace.repository.as_deref(),
        "合并目录已不属于原仓库"
    );
    ensure!(
        current_branch(target).as_deref() == Some(&state.target_branch),
        "目标目录分支已改变"
    );
    ensure!(
        git(target, &["rev-parse", "MERGE_HEAD"])?.trim() == state.source_sha,
        "目标目录已不再是本次合并"
    );
    ensure!(
        git(target, &["rev-parse", "HEAD"])?.trim() == state.target_sha,
        "目标分支已改变"
    );
    Ok(state)
}

pub(crate) fn finish_merge(
    mut workspace: Workspace,
    expected: &WorkspaceReview,
    abort: bool,
) -> Result<Workspace> {
    let cwd = PathBuf::from(&workspace.path);
    with_repository(&cwd, || {
        let state = validate_merge(&workspace)?;
        let target = Path::new(&state.target_path);
        ensure!(
            &review(&workspace)? == expected,
            "冲突解决内容已更新，请刷新后重新确认"
        );
        if abort {
            git(target, &["merge", "--abort"])?;
        } else {
            ensure!(
                expected.conflicts.is_empty(),
                "仍有未解决的冲突，请编辑文件并 git add 后继续"
            );
            ensure!(
                diff(target, &["--"])?.is_empty(),
                "冲突解决内容尚未全部暂存，请 git add 后刷新审查"
            );
            git(target, &["commit", "--no-edit"])?;
            workspace.merge_target = Some(state.target_branch.clone());
        }
        workspace.merge = None;
        Ok(workspace)
    })
}

pub(crate) fn reconcile_merge(workspace: &mut Workspace) {
    let Some(state) = workspace.merge.clone() else {
        return;
    };
    let target = Path::new(&state.target_path);
    if repository(target).ok().as_deref().and_then(Path::to_str) != workspace.repository.as_deref()
    {
        return;
    }
    if git(target, &["rev-parse", "--verify", "MERGE_HEAD"]).is_ok() {
        return;
    }
    if git(
        target,
        &[
            "merge-base",
            "--is-ancestor",
            &state.source_sha,
            &format!("refs/heads/{}", state.target_branch),
        ],
    )
    .is_ok()
    {
        workspace.merge_target = Some(state.target_branch);
    }
    workspace.merge = None;
}
