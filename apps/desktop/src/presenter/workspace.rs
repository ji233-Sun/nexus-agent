use super::Presenter;
use crate::model::GenerationKind;
use crate::model::workspace::PendingWorkspaceStart;
use crate::model::workspace::{MergePlan, WorkspaceReview};
use crate::{
    infrastructure::git,
    model::workspace::{Workspace, WorkspaceDraft, WorkspaceKind, WorkspaceStatus},
};
use anyhow::Result;
use nexus_domain::PermissionMode;
use nexus_protocol::{Command, CommandEnvelope};
use std::{path::Path, sync::mpsc};
use uuid::Uuid;

pub(super) enum WorkspaceEvent {
    Resolved(Result<Workspace>),
    Created(Result<Workspace>),
    Reviewed(Result<WorkspaceReview>),
    Committed(Result<WorkspaceReview>),
    CommitPrepared {
        request_id: Uuid,
        command: Result<Command>,
    },
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
            if self.model.project_is_git {
                let path = Path::new(&project.canonical_path);
                let branches = git::local_branches(path).unwrap_or_default();
                self.model.workspace_draft.base = git::current_branch(path)
                    .filter(|branch| branches.contains(branch))
                    .or_else(|| branches.into_iter().next())
                    .unwrap_or_default();
            }
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

    pub(crate) fn workspace_base_branches(&self) -> Result<Vec<String>> {
        let project = self
            .model
            .selected_project
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("请先选择项目目录。"))?;
        git::local_branches(Path::new(&project.canonical_path))
    }

    pub(crate) fn select_workspace_base(&mut self, base: String) {
        if self.model.selected_task.is_none()
            && self.model.workspace_draft.kind == WorkspaceKind::Worktree
            && self
                .model
                .selected_workspace
                .as_ref()
                .is_none_or(|workspace| {
                    self.model.workspace_retry && workspace.status == WorkspaceStatus::Missing
                })
            && !self.model.workspace_busy
            && self
                .workspace_base_branches()
                .is_ok_and(|branches| branches.contains(&base))
        {
            self.model.workspace_draft.base = base;
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
        if self.model.workspace_draft.base.is_empty() {
            self.model.status = "请选择来源分支。".into();
            return false;
        }
        let root = match &self.worktree_root {
            Ok(root) => root,
            Err(error) => {
                self.model.status = error.to_string().into();
                return false;
            }
        };
        let workspace = git::planned_workspace(root, &project, &self.model.workspace_draft);
        let base = format!("refs/heads/{}", self.model.workspace_draft.base);
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
        let committed = matches!(&event, WorkspaceEvent::Committed(_));
        let changes_operation = matches!(
            &event,
            WorkspaceEvent::Reviewed(_)
                | WorkspaceEvent::Committed(_)
                | WorkspaceEvent::CommitPrepared { .. }
                | WorkspaceEvent::Planned(_)
                | WorkspaceEvent::Merged(_)
        );
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
            WorkspaceEvent::Reviewed(result) | WorkspaceEvent::Committed(result) => {
                result.map(|review| {
                    if committed {
                        self.model.commit_message.clear();
                        self.model.commit_message_request = None;
                        self.model.commit_editor_open = false;
                    }
                    if review.dirty_paths.is_empty() {
                        self.model.commit_editor_open = false;
                    }
                    self.model
                        .selected_changes
                        .retain(|path| review.dirty_paths.contains(path));
                    self.model.workspace_review = Some(review);
                    self.model.merge_plan = None;
                    self.model.changes_status = committed.then(|| "提交成功。".into());
                })
            }
            WorkspaceEvent::CommitPrepared {
                request_id,
                command,
            } => {
                if self.model.commit_message_request != Some(request_id) {
                    Ok(())
                } else {
                    command
                        .and_then(|command| {
                            self.runner
                                .as_ref()
                                .ok_or_else(|| anyhow::anyhow!("Runner 不可用。"))?
                                .send(CommandEnvelope::new(command))
                        })
                        .inspect_err(|_| {
                            self.model.commit_message_request = None;
                        })
                }
            }
            WorkspaceEvent::Planned(result) => result.map(|plan| {
                self.model.merge_plan = Some(plan);
                self.model.changes_status = None;
            }),
            WorkspaceEvent::Merged(result) => result.and_then(|workspace| {
                let conflicted = workspace.merge.is_some();
                self.storage.save_workspace(&workspace)?;
                self.model.merge_plan = None;
                self.reload_workspaces();
                self.model.workspace_review = Some(git::changes::review(&workspace)?);
                self.model.changes_status = Some(if conflicted {
                    "合并存在冲突。请在目标目录解决并暂存文件，然后继续合并；也可以中止。".into()
                } else {
                    "本地合并操作已完成。".into()
                });
                Ok(())
            }),
        };
        if let Err(error) = result {
            let message = format!("Git 操作失败：{error}").into();
            if changes_operation {
                self.model.changes_status = Some(message);
            } else {
                self.model.status = message;
            }
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
        self.reload_workspaces();
        let Ok(Some(workspace)) = self.storage.workspace(id) else {
            return false;
        };
        if self
            .model
            .workspace_review
            .as_ref()
            .is_none_or(|review| review.workspace_id != id)
        {
            self.model.selected_changes.clear();
        }
        self.model.workspace_review = None;
        self.model.commit_message_request = None;
        self.model.merge_plan = None;
        self.spawn_workspace_operation(move || {
            WorkspaceEvent::Reviewed(git::changes::review(&workspace))
        });
        self.model.changes_status = None;
        true
    }

    pub(crate) fn select_changed_file(&mut self, path: String, selected: bool) {
        self.model.commit_message_request = None;
        self.model.changes_status = None;
        if selected {
            self.model.selected_changes.insert(path);
        } else {
            self.model.selected_changes.remove(&path);
        }
    }

    pub(crate) fn toggle_changes_sidebar(&mut self) {
        self.model.changes_sidebar_open = !self.model.changes_sidebar_open;
        if self.model.changes_sidebar_open {
            self.review_conversation_changes();
        }
    }

    pub(crate) fn toggle_changes_files(&mut self) {
        self.model.changes_files_expanded = !self.model.changes_files_expanded;
    }

    pub(crate) fn toggle_commit_editor(&mut self) {
        if self.model.commit_editor_open {
            self.model.commit_editor_open = false;
        } else if let Some(review) = &self.model.workspace_review
            && !review.dirty_paths.is_empty()
        {
            let files = review.dirty_paths.clone();
            if self.model.selected_changes.is_empty() {
                self.model.selected_changes.extend(files);
            }
            self.model.commit_editor_open = true;
        }
    }

    pub(crate) fn review_conversation_changes(&mut self) -> bool {
        if self.model.workspace_busy || self.model.selected_codex_thread.is_some() {
            return false;
        }
        let workspace = self.model.selected_workspace.clone().or_else(|| {
            (self.model.selected_task.is_none()
                && self.model.workspace_draft.kind == WorkspaceKind::Local)
                .then(|| self.model.selected_project.as_ref().map(Workspace::local))
                .flatten()
        });
        let Some(workspace) = workspace else {
            self.model.changes_status = Some("任务目录尚未就绪，无法查看变更。".into());
            return false;
        };
        if let Err(error) = self.storage.save_workspace(&workspace) {
            self.model.changes_status = Some(error.to_string().into());
            return false;
        }
        self.review_workspace(workspace.id)
    }

    pub(crate) fn set_commit_message(&mut self, message: String) {
        if self.model.commit_message != message {
            self.model.commit_message = message;
            self.model.commit_message_request = None;
            self.model.changes_status = None;
        }
    }

    pub(crate) fn generate_workspace_commit_message(&mut self) -> bool {
        if self.model.commit_message_request.is_some() {
            return false;
        }
        let Some(review) = self.model.workspace_review.clone() else {
            return false;
        };
        let files = self
            .model
            .selected_changes
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        if files.is_empty() {
            return false;
        }
        let configuration = self
            .workspace_for_write(review.workspace_id)
            .and_then(|workspace| {
                Ok((
                    workspace,
                    self.generation_configuration(GenerationKind::Commit)?,
                ))
            });
        let (workspace, configuration) = match configuration {
            Ok(configuration) => configuration,
            Err(error) => {
                self.model.changes_status = Some(error.to_string().into());
                return false;
            }
        };
        let request_id = Uuid::new_v4();
        let language = self.model.language.as_str().to_owned();
        self.model.commit_message_request = Some(request_id);
        self.model.workspace_operation_paths = vec![Path::new(&workspace.path).to_path_buf()];
        self.spawn_workspace_operation(move || WorkspaceEvent::CommitPrepared {
            request_id,
            command: git::changes::selected_diff(&workspace, &review, &files).map(|diff| {
                Command::GenerateCommitMessage {
                    request_id,
                    cwd: workspace.path,
                    diff,
                    language,
                    configuration,
                }
            }),
        });
        self.model.changes_status = None;
        true
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
                self.model.changes_status = Some(error.to_string().into());
                return false;
            }
        };
        self.model.workspace_operation_paths = vec![Path::new(&workspace.path).to_path_buf()];
        self.model.changes_status = None;
        self.spawn_workspace_operation(move || {
            WorkspaceEvent::Committed(git::changes::commit_files(
                &workspace, &review, &files, &message,
            ))
        });
        true
    }

    pub(crate) fn preview_workspace_merge(&mut self, id: Uuid, target: String) -> bool {
        let workspace = match self.workspace_for_write(id) {
            Ok(workspace) => workspace,
            Err(error) => {
                self.model.changes_status = Some(error.to_string().into());
                return false;
            }
        };
        let Ok(Some(project)) = self.storage.project(workspace.project_id) else {
            return false;
        };
        self.model.merge_plan = None;
        self.model.changes_status = None;
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
                self.model.changes_status = Some(error.to_string().into());
                return false;
            }
        };
        if self
            .model
            .checkout_running(Path::new(&plan.state.target_path))
        {
            self.model.changes_status = Some("目标目录仍有任务运行，不能合入。".into());
            return false;
        }
        let Ok(Some(project)) = self.storage.project(workspace.project_id) else {
            return false;
        };
        workspace.merge = Some(plan.state.clone());
        if let Err(error) = self.storage.save_workspace(&workspace) {
            self.model.changes_status = Some(error.to_string().into());
            return false;
        }
        self.model.workspace_operation_paths = vec![
            Path::new(&workspace.path).to_path_buf(),
            Path::new(&plan.state.target_path).to_path_buf(),
        ];
        self.model.changes_status = None;
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
                self.model.changes_status = Some(error.to_string().into());
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
        self.model.changes_status = None;
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
            self.model.workspace_draft = WorkspaceDraft {
                kind: previous.kind,
                base: previous.base,
                ..WorkspaceDraft::default()
            };
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
