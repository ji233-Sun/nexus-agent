use super::*;

impl Presenter {
    pub(crate) fn open_project(&mut self, path: &Path) {
        match self.storage.open_project(path) {
            Ok(project) => {
                self.select_project(project);
                self.reload_projects();
            }
            Err(error) => self.model.log_status(LocalizedText::new(
                "无法打开项目：{error}",
                &[("error", (error).to_string())],
            )),
        }
    }

    pub(crate) fn can_delete_project(&self, project_id: Uuid) -> bool {
        !self.model.workspace_busy
            && !self.model.all_conversations().any(|conversation| {
                conversation
                    .selected_project
                    .as_ref()
                    .is_some_and(|project| project.id == project_id)
                    && (conversation.active_run.is_some()
                        || (conversation.pending_workspace_start.is_some()
                            && !conversation.workspace_retry))
            })
    }

    pub(crate) fn delete_project(&mut self, project_id: Uuid) -> bool {
        if !self.can_delete_project(project_id) {
            self.model
                .log_status("项目仍有活动任务或工作区操作，请结束后再删除。".into());
            return false;
        }
        if let Err(error) = self.storage.delete_project(project_id) {
            self.model.log_status(LocalizedText::new(
                "无法删除项目：{error}",
                &[("error", error.to_string())],
            ));
            return false;
        }
        if self
            .model
            .conversation
            .selected_project
            .as_ref()
            .is_some_and(|project| project.id == project_id)
        {
            self.new_projectless_task();
        }
        self.model.conversations.retain(|_, conversation| {
            conversation
                .selected_project
                .as_ref()
                .is_none_or(|project| project.id != project_id)
        });
        self.reload_projects();
        self.reload_tasks();
        self.model
            .log_status("项目已删除，磁盘上的文件和 Git 分支已保留。".into());
        true
    }

    pub(crate) fn new_task(&mut self) {
        for provider in IssueProvider::ALL {
            self.model.issues_mut(provider).opened = false;
        }
        self.cancel_voice();
        self.model.fresh_conversation();
        self.model.conversation.selected_task = None;
        self.reset_workspace_draft();
        self.model.conversation.messages.clear();
        self.model.conversation.streaming_text.clear();
        self.model.log_status("已准备好新任务。".into());
        self.model.conversation.permission_mode =
            load_permission_mode(&self.storage, self.model.conversation.selected_harness);
        self.refresh_model_catalog();
    }

    pub(crate) fn new_projectless_task(&mut self) {
        self.select_project_context(None);
    }

    pub(crate) fn select_project(&mut self, project: Project) {
        self.select_project_context(Some(project));
    }

    pub(super) fn select_project_context(&mut self, project: Option<Project>) {
        self.cancel_voice();
        self.model.fresh_conversation();
        self.model.conversation.permission_mode =
            load_permission_mode(&self.storage, self.model.conversation.selected_harness);
        self.model.conversation.selected_project = project;
        self.model.conversation.selected_task = None;
        self.reset_workspace_draft();
        self.reload_workspaces();
        self.model.conversation.messages.clear();
        self.model.conversation.streaming_text.clear();
        self.reload_tasks();
        self.refresh_model_catalog();
        self.reset_issues_project();
    }

    pub(super) fn reload_projects(&mut self) {
        let Ok(mut projects) = self.storage.projects() else {
            return;
        };
        if let Some(selected_project) = &self.model.conversation.selected_project
            && !projects
                .iter()
                .any(|project| project.id == selected_project.id)
        {
            projects.push(selected_project.clone());
        }
        self.model.projects = projects;
    }

    pub(crate) fn reorder_project(&mut self, project_id: Uuid, target_id: Uuid) -> bool {
        let projects = &self.model.projects;
        let Some(source) = projects.iter().position(|project| project.id == project_id) else {
            return false;
        };
        let Some(target) = projects.iter().position(|project| project.id == target_id) else {
            return false;
        };
        if source == target {
            return false;
        }
        let mut order: Vec<_> = projects.iter().map(|project| project.id).collect();
        order.remove(source);
        order.insert(target, project_id);
        if let Err(error) = self.storage.save_project_order(&order) {
            self.model.log_status(LocalizedText::new(
                "无法保存项目顺序：{error}",
                &[("error", error.to_string())],
            ));
            return false;
        }
        let project = self.model.projects.remove(source);
        self.model.projects.insert(target, project);
        self.notify_remote_changed();
        true
    }

    pub(super) fn reload_tasks(&mut self) {
        self.model.projectless_tasks = self.storage.tasks(None).unwrap_or_default();
        let contexts: Vec<_> = self
            .model
            .all_conversations()
            .map(|conversation| conversation.id)
            .collect();
        for context in contexts {
            let project = self.model[context]
                .selected_project
                .as_ref()
                .map(|project| project.id);
            self.model[context].tasks = self.storage.tasks(project).unwrap_or_default();
        }
        self.model.archived_tasks = self.storage.archived_tasks().unwrap_or_default();
    }

    pub(crate) fn select_task(&mut self, task_id: Uuid) {
        let Ok(Some(task)) = self.storage.task(task_id) else {
            return;
        };
        let project_changed = self
            .model
            .conversation
            .selected_project
            .as_ref()
            .map(|project| project.id)
            != task.project_id;
        for provider in IssueProvider::ALL {
            self.model.issues_mut(provider).opened = false;
        }
        self.cancel_voice();
        if self.model.conversation.selected_task != Some(task_id) {
            let existing = self
                .model
                .conversations
                .values()
                .find(|conversation| conversation.selected_task == Some(task_id))
                .map(|conversation| conversation.id);
            if let Some(id) = existing {
                self.model.activate_conversation(id);
            } else {
                self.model.fresh_conversation();
            }
        }
        self.reload_task_context(self.model.conversation.id, task_id);
        if project_changed {
            self.reset_issues_project();
        }
    }

    pub(super) fn reload_task_context(&mut self, context: Uuid, task_id: Uuid) {
        let Ok(Some(task)) = self.storage.task(task_id) else {
            return;
        };
        // Resolve context from the saved task, never from the previously selected project.
        self.model[context].selected_project = task
            .project_id
            .and_then(|id| self.storage.project(id).ok().flatten());
        self.model[context].selected_task = Some(task_id);
        self.model[context].selected_workspace =
            self.storage.task_workspace(task_id).ok().flatten();
        self.reload_workspaces_in(context);
        self.model[context].messages = self.storage.messages(task_id).unwrap_or_default();
        self.model[context].completed_runs =
            self.storage.completed_runs(task_id).unwrap_or_default();
        self.reload_tasks();
        if self.model[context].active_run.is_some() {
            return;
        }
        if let Ok(Some(config)) = self.storage.conversation_config(task_id) {
            self.model[context].selected_harness = config.harness;
            self.model[context].permission_mode = config.permission_mode;
            self.model[context].effort =
                normalize_effort_for_harness(config.harness, config.effort);
            self.model[context].model_override =
                (config.model != "default").then_some(config.model.clone());
            self.model[context].model_override_name = None;
            self.model[context].model_catalog = ModelCatalogState::Idle;
            let executable = if config.executable.is_empty() {
                self.storage
                    .setting(executable_setting_key(config.harness))
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| config.harness.default_executable().into())
            } else {
                config.executable
            };
            self.model[context].executable = executable.clone();
            if !self
                .model
                .selected_probe_in(context)
                .is_some_and(|probe| probe.executable == executable)
            {
                self.model.harnesses.remove(&config.harness);
                self.model.log_status(
                    if let Some(runner) = &self.runner
                        && runner
                            .send(CommandEnvelope::new(Command::HarnessProbe {
                                environment: self.codebuddy_environment(config.harness),
                                harness: config.harness,
                                executable,
                            }))
                            .is_ok()
                    {
                        LocalizedText::new("正在探测 {0}…", &[("0", (config.harness).to_string())])
                    } else {
                        "Runner 不可用，无法探测任务使用的可执行文件。".into()
                    },
                );
            }
            self.refresh_model_catalog_in(context);
        }
    }

    pub(crate) fn archive_task(&mut self, task_id: Uuid) -> bool {
        if self.model.task_running(task_id) || self.model.workspace_busy {
            return false;
        }
        if let Err(error) = self.storage.archive_task(task_id) {
            self.model.log_status(LocalizedText::new(
                "无法归档对话：{error}",
                &[("error", (error).to_string())],
            ));
            return false;
        }
        if self.model.conversation.selected_task == Some(task_id) {
            self.new_task();
        }
        self.reload_tasks();
        self.model.log_status("对话已归档。".into());
        true
    }

    pub(crate) fn restore_task(&mut self, task_id: Uuid) -> bool {
        if self.model.task_running(task_id) || self.model.workspace_busy {
            return false;
        }
        if let Err(error) = self.storage.restore_task(task_id) {
            self.model.log_status(LocalizedText::new(
                "无法取消归档：{error}",
                &[("error", (error).to_string())],
            ));
            return false;
        }
        self.reload_projects();
        self.reload_tasks();
        self.model.log_status("对话已恢复。".into());
        true
    }

    pub(crate) fn delete_task(&mut self, task_id: Uuid) -> bool {
        if self.model.task_running(task_id) || self.model.workspace_busy {
            return false;
        }
        if let Err(error) = self.storage.delete_task(task_id) {
            self.model.log_status(LocalizedText::new(
                "无法删除对话：{error}",
                &[("error", (error).to_string())],
            ));
            return false;
        }
        self.model
            .conversation
            .queued_messages
            .retain(|message| message.task_id != task_id);
        if self.model.conversation.selected_task == Some(task_id) {
            self.new_task();
        }
        self.model
            .conversations
            .retain(|_, conversation| conversation.selected_task != Some(task_id));
        self.reload_projects();
        self.reload_tasks();
        self.model.log_status("对话已删除。".into());
        true
    }

    pub(crate) fn delete_archived_tasks(&mut self) -> bool {
        if self.model.conversation.active_run.is_some() {
            return false;
        }
        match self.storage.delete_archived_tasks() {
            Ok(count) => {
                self.model.conversations.retain(|_, conversation| {
                    !self
                        .model
                        .archived_tasks
                        .iter()
                        .any(|task| Some(task.id) == conversation.selected_task)
                });
                self.model.conversation.queued_messages.retain(|message| {
                    !self
                        .model
                        .archived_tasks
                        .iter()
                        .any(|task| task.id == message.task_id)
                });
                self.reload_projects();
                self.reload_tasks();
                self.model.log_status(LocalizedText::new(
                    "已删除 {count} 个归档对话。",
                    &[("count", (count).to_string())],
                ));
                true
            }
            Err(error) => {
                self.model.log_status(LocalizedText::new(
                    "无法清空归档对话：{error}",
                    &[("error", (error).to_string())],
                ));
                false
            }
        }
    }
}
