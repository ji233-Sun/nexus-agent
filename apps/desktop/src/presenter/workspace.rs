use super::Presenter;
use crate::{
    infrastructure::git,
    model::workspace::{Workspace, WorkspaceDraft, WorkspaceKind, WorkspaceStatus},
};
use anyhow::Result;
use nexus_domain::PermissionMode;
use std::{path::Path, sync::mpsc};
use uuid::Uuid;

pub(super) struct PendingWorkspaceStart {
    prompt: String,
    executable: String,
    permission: PermissionMode,
}

pub(super) enum WorkspaceEvent {
    Resolved(Result<Workspace>),
    Created(Result<Workspace>),
    Cleaned(Result<Workspace>),
}

impl Presenter {
    pub(super) fn reset_workspace_draft(&mut self) {
        self.model.selected_workspace = None;
        self.pending_workspace_start = None;
        self.model.workspace_retry = false;
        self.model.workspace_draft = WorkspaceDraft::default();
        if let Some(project) = &self.model.selected_project {
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
        if let Some(project) = &self.model.selected_project {
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
            && self.model.selected_workspace.is_none()
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
                let recovered =
                    git::recover_workspace(Path::new(&project.canonical_path), workspace);
                let _ = self.storage.save_workspace(&recovered);
                recovered
            })
            .collect();
        if let Some(selected) = &mut self.model.selected_workspace
            && let Some(workspace) = self
                .model
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
        self.pending_workspace_start = Some(PendingWorkspaceStart {
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
        let (sender, receiver) = mpsc::channel();
        self.workspace_events = Some(receiver);
        self.model.workspace_busy = true;
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
        self.workspace_events = None;
        self.model.workspace_busy = false;
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
            WorkspaceEvent::Cleaned(result) => result.and_then(|workspace| {
                self.storage.save_workspace(&workspace)?;
                self.reload_workspaces();
                self.model.status = "Worktree 目录已清理，聊天记录和任务分支已保留。".into();
                Ok(())
            }),
        };
        if let Err(error) = result {
            self.model.status = format!("Worktree 操作失败：{error}").into();
            self.model.workspace_retry = self.pending_workspace_start.is_some();
            self.reload_workspaces();
        }
        true
    }

    pub(crate) fn retry_workspace_start(&mut self) -> bool {
        let Some(pending) = self.pending_workspace_start.take() else {
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
            let kind = self.model.workspace_draft.kind;
            let base = self.model.workspace_draft.base.clone();
            self.model.workspace_draft = WorkspaceDraft::default();
            self.model.workspace_draft.kind = kind;
            self.model.workspace_draft.base = base;
            self.model.selected_workspace = None;
            self.begin_worktree(&pending.prompt, &pending.executable, pending.permission)
        };
        if !started {
            self.pending_workspace_start = Some(pending);
            self.model.workspace_retry = true;
        }
        started
    }

    pub(crate) fn cleanup_workspace(&mut self, id: Uuid, target: String) -> bool {
        if self.model.workspace_busy || self.model.active_run.is_some() {
            return false;
        }
        let Some(workspace) = self
            .model
            .workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .cloned()
        else {
            return false;
        };
        let Ok(Some(project)) = self.storage.project(workspace.project_id) else {
            return false;
        };
        let Ok(root) = &self.worktree_root else {
            return false;
        };
        let root = root.clone();
        self.spawn_workspace_operation(move || {
            WorkspaceEvent::Cleaned(git::cleanup_worktree(
                &root,
                Path::new(&project.canonical_path),
                workspace,
                &target,
            ))
        });
        self.model.status = "正在检查并清理 Worktree…".into();
        true
    }
}
