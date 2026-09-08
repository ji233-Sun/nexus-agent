use super::Presenter;
use crate::model::workspace::PendingWorkspaceStart;
use crate::model::workspace::{MergePlan, WorkspaceReview};
use crate::{
    infrastructure::git,
    model::workspace::{Workspace, WorkspaceDraft, WorkspaceKind, WorkspaceStatus},
};
use anyhow::Result;
use nexus_domain::PermissionMode;
use std::{path::Path, sync::mpsc};
use uuid::Uuid;

pub(super) enum WorkspaceEvent {
    Resolved(Result<Workspace>),
    Created(Result<Workspace>),
    Reviewed(Result<WorkspaceReview>),
    Planned(Result<MergePlan>),
    Merged(Result<Workspace>),
}

impl Presenter {
    pub(super) fn reset_workspace_draft(&mut self) {
        self.model.selected_workspace = None;
        self.model.pending_workspace_start = None;
        self.model.workspace_retry = false;
        self.model.workspace_draft = WorkspaceDraft::default();
        if let Some(project) = self.model.selected_project.clone() {
            self.model.project_dirty = git::is_git_dirty(Path::new(&project.canonical_path));
            self.model.project_is_git = git::repository(Path::new(&project.canonical_path)).is_ok();
            if self.model.project_is_git
                && self
                    .storage
                    .setting(&format!("workspace_mode:{}", project.id))
                    .ok()
                    .flatten()
                    .as_deref()
                    == Some("worktree")
            {
                self.model.workspace_draft.kind = WorkspaceKind::Worktree;
            }
        }
        self.refresh_workspace_branch();
    }

    pub(crate) fn select_workspace_kind(&mut self, kind: WorkspaceKind) {
        if self.model.selected_task.is_some()
            || self.model.selected_workspace.is_some()
            || self.model.workspace_busy
            || (kind == WorkspaceKind::Worktree && !self.model.project_is_git)
        {
            return;
        }
        self.model.workspace_draft.kind = kind;
        if let Some(project) = self.model.selected_project.clone() {
            let value = if kind == WorkspaceKind::Worktree {
                "worktree"
            } else {
                "local"
            };
            if let Err(error) = self
                .storage
                .set_setting(&format!("workspace_mode:{}", project.id), value)
            {
                self.model.status = error.to_string().into();
            }
        }
    }

    pub(crate) fn configure_workspace(&mut self, base: String, branch: String) {
        if self.model.selected_task.is_none()
            && self
                .model
                .selected_workspace
                .as_ref()
                .is_none_or(|workspace| {
                    self.model.workspace_retry && workspace.status == WorkspaceStatus::Missing
                })
            && !self.model.workspace_busy
        {
            self.model.workspace_draft.base = base.trim().to_owned();
            self.model.workspace_draft.branch = branch.trim().to_owned();
        }
    }

    pub(crate) fn reload_workspaces(&mut self) {
        let Some(project) = &self.model.selected_project else {
            return;
        };
        self.model.workspaces = self
            .storage
            .workspaces(project.id)
            .unwrap_or_default()
            .into_iter()
            .map(|workspace| {
                let recovered = if self.model.workspace_busy {
                    workspace
                } else {
                    git::recover_workspace(Path::new(&project.canonical_path), workspace)
                };
                let _ = self.storage.save_workspace(&recovered);
                recovered
            })
            .collect();
        if let Some(selected) = &mut self.model.conversation.selected_workspace
            && let Some(workspace) = self
                .model
                .conversation
                .workspaces
                .iter()
                .find(|workspace| workspace.id == selected.id)
        {
            *selected = workspace.clone();
        }
        self.refresh_workspace_branch();
    }

    pub(super) fn refresh_workspace_branch(&mut self) {
        self.model.workspace_branch = self
            .model
            .working_directory()
            .and_then(|path| git::current_branch(Path::new(path)));
    }

    pub(super) fn begin_worktree(
        &mut self,
        prompt: &str,
        executable: &str,
        permission: PermissionMode,
    ) -> bool {
        if self.model.workspace_busy || prompt.trim().is_empty() || !self.model.can_submit() {
            return false;
        }
        let Some(project) = self.model.selected_project.clone() else {
            return false;
        };
        let root = match &self.worktree_root {
            Ok(root) => root,
            Err(error) => {
                self.model.status = error.to_string().into();
                return false;
            }
        };
        let workspace = git::planned_workspace(root, &project, &self.model.workspace_draft);
        let base = self.model.workspace_draft.base.clone();
        self.model.pending_workspace_start = Some(PendingWorkspaceStart {
            context_id: self.model.conversation.id,
            prompt: prompt.trim().to_owned(),
            executable: executable.to_owned(),
            permission,
        });
        self.spawn_workspace_operation(move || {
            WorkspaceEvent::Resolved(git::resolve_workspace(
                Path::new(&project.canonical_path),
                workspace,
                &base,
            ))
        });
        self.model.status = "正在解析 Worktree 创建基准…".into();
        true
    }

    fn spawn_workspace_operation(
        &mut self,
        operation: impl FnOnce() -> WorkspaceEvent + Send + 'static,
    ) {
        for path in &mut self.model.workspace_operation_paths {
            if let Ok(checkout) = git::checkout_path(path) {
                *path = checkout;
            }
        }
        let (sender, receiver) = mpsc::channel();
        self.workspace_events = Some(receiver);
        self.model.workspace_busy = true;
        self.model.workspace_operation_context = Some(self.model.conversation.id);
        self.model.workspace_retry = false;
        std::thread::spawn(move || {
            let _ = sender.send(operation());
        });
    }

    pub(super) fn drain_workspace_events(&mut self) -> bool {
        let Some(event) = self
            .workspace_events
            .as_ref()
            .and_then(|events| events.try_recv().ok())
        else {
            return false;
        };
        let selected = self.model.conversation.id;
        if let Some(owner) = self.model.workspace_operation_context {
            self.model.activate_conversation(owner);
        }
        self.workspace_events = None;
        self.model.workspace_busy = false;
        self.model.workspace_operation_context = None;
        self.model.workspace_operation_paths.clear();
        let result = match event {
            WorkspaceEvent::Resolved(result) => {
                result.and_then(|workspace| {
                    // Persist the exact SHA and ownership before Git creates a directory.
                    self.storage.save_workspace(&workspace)?;
                    self.model.selected_workspace = Some(workspace.clone());
                    let project = self
                        .storage
                        .project(workspace.project_id)?
                        .ok_or_else(|| anyhow::anyhow!("项目不存在"))?;
                    self.spawn_workspace_operation(move || {
                        WorkspaceEvent::Created(git::create_worktree(
                            Path::new(&project.canonical_path),
                            workspace,
                        ))
                    });
                    self.model.status = "正在创建任务专属 Worktree…".into();
                    Ok(())
                })
            }
            WorkspaceEvent::Created(result) => result.and_then(|workspace| {
                self.storage.save_workspace(&workspace)?;
                self.model.selected_workspace = Some(workspace);
                self.reload_workspaces();
                // Catalog discovery and the Harness now resolve the same task cwd.
                if self.refresh_model_catalog() {
                    self.model.status = "Worktree 已创建，正在读取任务目录的模型配置…".into();
                } else {
                    self.retry_workspace_start();
                }
                Ok(())
            }),
            WorkspaceEvent::Reviewed(result) => result.map(|review| {
                self.model
                    .selected_changes
                    .retain(|path| review.dirty_paths.contains(path));
                self.model.workspace_review = Some(review);
                self.model.merge_plan = None;
                self.model.status = "变更已刷新。请选择文件提交，或预览本地合入。".into();
            }),
            WorkspaceEvent::Planned(result) => result.map(|plan| {
                self.model.merge_plan = Some(plan);
                self.model.status = "请确认源分支、目标目录和变更后合入。".into();
            }),
            WorkspaceEvent::Merged(result) => result.and_then(|workspace| {
                let conflicted = workspace.merge.is_some();
                self.storage.save_workspace(&workspace)?;
                self.model.merge_plan = None;
                self.reload_workspaces();
                self.model.workspace_review = Some(git::changes::review(&workspace)?);
                self.model.status = if conflicted {
                    "合并存在冲突。请在目标目录解决并暂存文件，然后继续合并；也可以中止。".into()
                } else {
                    "本地合并操作已完成。".into()
                };
                Ok(())
            }),
        };
        if let Err(error) = result {
            self.model.status = format!("Worktree 操作失败：{error}").into();
            self.model.workspace_retry = self.model.pending_workspace_start.is_some();
            self.reload_workspaces();
        }
        self.model.activate_conversation(selected);
        self.reload_tasks();
        true
    }

    fn workspace_for_write(&self, id: Uuid) -> Result<Workspace> {
        anyhow::ensure!(!self.model.workspace_busy, "请等待当前 Worktree 操作完成");
        anyhow::ensure!(
            !self.model.updates.state.is_installing(),
            "正在安装应用更新，重启后可继续任务。"
        );
        let workspace = self
            .storage
            .workspace(id)?
            .ok_or_else(|| anyhow::anyhow!("工作区不存在"))?;
        git::validate_workspace(&workspace)?;
        let path = git::checkout_path(Path::new(&workspace.path))?;
        anyhow::ensure!(
            !self.model.checkout_running(&path),
            "任务仍在运行，请先等待结束或停止"
        );
        if let Some(merge) = &workspace.merge {
            anyhow::ensure!(
                !self.model.checkout_running(Path::new(&merge.target_path)),
                "合并目标目录仍有任务运行"
            );
        }
        Ok(workspace)
    }

    pub(crate) fn review_workspace(&mut self, id: Uuid) -> bool {
        if self.model.workspace_busy {
            return false;
        }
        let Ok(Some(workspace)) = self.storage.workspace(id) else {
            return false;
        };
        self.model.workspace_review = None;
        self.model.selected_changes.clear();
        self.model.merge_plan = None;
        self.spawn_workspace_operation(move || {
            WorkspaceEvent::Reviewed(git::changes::review(&workspace))
        });
        self.model.status = "正在读取完整任务变更…".into();
        true
    }

    pub(crate) fn select_changed_file(&mut self, path: String, selected: bool) {
        if selected {
            self.model.selected_changes.insert(path);
        } else {
            self.model.selected_changes.remove(&path);
        }
    }

    pub(crate) fn commit_workspace_files(
        &mut self,
        review: WorkspaceReview,
        files: Vec<String>,
        message: String,
    ) -> bool {
        let workspace = match self.workspace_for_write(review.workspace_id) {
            Ok(workspace) => workspace,
            Err(error) => {
                self.model.status = error.to_string().into();
                return false;
            }
        };
        self.model.workspace_operation_paths = vec![Path::new(&workspace.path).to_path_buf()];
        self.spawn_workspace_operation(move || {
            WorkspaceEvent::Reviewed(git::changes::commit_files(
                &workspace, &review, &files, &message,
            ))
        });
        true
    }

    pub(crate) fn preview_workspace_merge(&mut self, id: Uuid, target: String) -> bool {
        let workspace = match self.workspace_for_write(id) {
            Ok(workspace) => workspace,
            Err(error) => {
                self.model.status = error.to_string().into();
                return false;
            }
        };
        let Ok(Some(project)) = self.storage.project(workspace.project_id) else {
            return false;
        };
        self.model.merge_plan = None;
        self.spawn_workspace_operation(move || {
            WorkspaceEvent::Planned(git::changes::plan_merge(
                Path::new(&project.canonical_path),
                &workspace,
                &target,
            ))
        });
        true
    }

    pub(crate) fn merge_workspace(&mut self, plan: MergePlan) -> bool {
        let mut workspace = match self.workspace_for_write(plan.workspace_id) {
            Ok(workspace) => workspace,
            Err(error) => {
                self.model.status = error.to_string().into();
                return false;
            }
        };
        if self
            .model
            .checkout_running(Path::new(&plan.state.target_path))
        {
            self.model.status = "目标目录仍有任务运行，不能合入。".into();
            return false;
        }
        let Ok(Some(project)) = self.storage.project(workspace.project_id) else {
            return false;
        };
        workspace.merge = Some(plan.state.clone());
        if let Err(error) = self.storage.save_workspace(&workspace) {
            self.model.status = error.to_string().into();
            return false;
        }
        self.model.workspace_operation_paths = vec![
            Path::new(&workspace.path).to_path_buf(),
            Path::new(&plan.state.target_path).to_path_buf(),
        ];
        self.spawn_workspace_operation(move || {
            WorkspaceEvent::Merged(git::changes::merge(
                Path::new(&project.canonical_path),
                workspace,
                &plan,
            ))
        });
        true
    }

    pub(crate) fn finish_workspace_merge(&mut self, review: WorkspaceReview, abort: bool) -> bool {
        let workspace = match self.workspace_for_write(review.workspace_id) {
            Ok(workspace) => workspace,
            Err(error) => {
                self.model.status = error.to_string().into();
                return false;
            }
        };
        let Some(state) = workspace.merge.as_ref() else {
            return false;
        };
        self.model.workspace_operation_paths = vec![
            Path::new(&workspace.path).to_path_buf(),
            Path::new(&state.target_path).to_path_buf(),
        ];
        self.spawn_workspace_operation(move || {
            WorkspaceEvent::Merged(git::changes::finish_merge(workspace, &review, abort))
        });
        true
    }

    pub(crate) fn retry_workspace_start(&mut self) -> bool {
        if self
            .model
            .pending_workspace_start
            .as_ref()
            .is_some_and(|pending| pending.context_id != self.model.conversation.id)
        {
            return false;
        }
        let Some(pending) = self.model.pending_workspace_start.take() else {
            return false;
        };
        self.model.workspace_retry = false;
        let started = if self
            .model
            .selected_workspace
            .as_ref()
            .is_some_and(|workspace| workspace.status == WorkspaceStatus::Ready)
        {
            self.start_run(
                None,
                &pending.prompt,
                &pending.executable,
                pending.permission,
            )
        } else {
            // Failed creation keeps its record visible. A retry uses a new stable ID.
            let previous = self.model.workspace_draft.clone();
            let mut draft = WorkspaceDraft {
                kind: previous.kind,
                base: previous.base,
                ..WorkspaceDraft::default()
            };
            if previous.branch != format!("feat/nx-{}", &previous.task_id.simple().to_string()[..8])
            {
                draft.branch = previous.branch;
            }
            self.model.workspace_draft = draft;
            self.model.selected_workspace = None;
            self.begin_worktree(&pending.prompt, &pending.executable, pending.permission)
        };
        if !started {
            self.model.pending_workspace_start = Some(pending);
            self.model.workspace_retry = true;
        }
        started
    }
}
