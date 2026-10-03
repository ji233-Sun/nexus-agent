use super::*;

impl Presenter {
    pub(crate) fn select_catalog_model(&mut self, model_id: Option<String>) {
        if self.model.conversation.active_run.is_some() {
            return;
        }
        if model_id.as_deref().is_some_and(|id| {
            !self
                .model
                .conversation
                .model_catalog
                .can_select_model(self.model.conversation.selected_harness, id)
        }) {
            self.model.log_status(LocalizedText::new(
                "所选模型不在当前 {0} 目录中，请刷新后重试。",
                &[("0", (self.model.conversation.selected_harness).to_string())],
            ));
            return;
        }
        if self.model.conversation.model_override == model_id {
            return;
        }
        self.model.conversation.model_override = model_id;
        self.remember_model_name();
        let harness = self.model.conversation.selected_harness;
        let profile_id = self
            .model
            .selected_provider_profile()
            .map(|profile| profile.id);
        let _ = self.storage.set_setting(
            &catalog_model_setting_key(harness, profile_id),
            self.model
                .conversation
                .model_override
                .as_deref()
                .unwrap_or_default(),
        );
        let effort_reset = self.normalize_catalog_effort();
        self.model.log_status(if effort_reset {
            "模型已切换；原 effort 不受支持，已恢复为模型默认。".into()
        } else if let Some(model) = &self.model.conversation.model_override {
            LocalizedText::new(
                "本次 {harness} 任务将使用 {model}。",
                &[
                    ("harness", (harness).to_string()),
                    ("model", (model).to_string()),
                ],
            )
        } else {
            LocalizedText::new(
                "{harness} 模型已恢复为跟随 Profile / CLI 默认。",
                &[("harness", (harness).to_string())],
            )
        });
    }

    pub(crate) fn select_effort(&mut self, effort: ThinkingEffort) {
        if self.model.conversation.active_run.is_some() || self.model.conversation.effort == effort
        {
            return;
        }
        if !effort.is_default()
            && self
                .model
                .selected_catalog_model()
                .is_none_or(|model| !model.supports_effort(&effort))
        {
            self.model.log_status(LocalizedText::new(
                "当前 {0} 模型不支持所选 effort。",
                &[("0", (self.model.conversation.selected_harness).to_string())],
            ));
            return;
        }
        self.model.conversation.effort =
            normalize_effort_for_harness(self.model.conversation.selected_harness, effort);
        self.persist_catalog_effort();
    }

    pub(crate) fn refresh_model_catalog(&mut self) -> bool {
        self.refresh_model_catalog_in(self.model.conversation.id)
    }

    pub(crate) fn refresh_model_catalog_in(&mut self, context: Uuid) -> bool {
        if self.model[context].active_run.is_some() {
            return false;
        }
        if self.model[context].selected_project.is_none()
            && self.model[context].selected_task.is_none()
            && self.model[context].selected_workspace.is_none()
        {
            match self
                .storage
                .prepare_projectless_workspace(self.model[context].workspace_draft.task_id)
            {
                Ok(workspace) => self.model[context].selected_workspace = Some(workspace),
                Err(error) => {
                    self.model[context].model_catalog =
                        ModelCatalogState::NotReady(error.to_string().into());
                    return false;
                }
            }
        }
        let project_id = self.model[context]
            .selected_project
            .as_ref()
            .map(|project| project.id);
        let project_changed = self.model[context].catalog_project != project_id;
        if project_changed {
            self.model[context].title_model_catalog = ModelCatalogState::Idle;
            self.model[context].commit_model_catalog = ModelCatalogState::Idle;
        }
        self.model[context].catalog_project = project_id;
        let harness = self.model[context].selected_harness;
        let Some(cwd) = self.model.working_directory_in(context).map(str::to_owned) else {
            self.model[context].model_catalog = ModelCatalogState::Idle;
            return false;
        };
        if self
            .model
            .selected_probe_in(context)
            .is_some_and(|probe| !probe.available)
        {
            self.model[context].model_catalog = ModelCatalogState::NotReady(
                "可执行文件尚未就绪，请在设置中检查并重新探测。".into(),
            );
            return false;
        }
        let environment = match self.provider_launch_configuration_in(context, harness) {
            Ok(configuration) => configuration,
            Err(error) => {
                self.model[context].model_catalog =
                    ModelCatalogState::NotReady(error.to_string().into());
                return false;
            }
        };
        let Some(runner) = &self.runner else {
            self.model[context]
                .model_catalog
                .fail("Runner 不可用。".into());
            return false;
        };
        let request_id = Uuid::new_v4();
        let command = CommandEnvelope::new(Command::ModelCatalogRefresh {
            context_id: Some(self.model[context].id),
            request_id,
            purpose: ModelCatalogPurpose::Conversation,
            harness,
            executable: self.model[context].executable.clone(),
            cwd,
            environment,
        });
        let mut models = self.model[context]
            .model_catalog
            .models()
            .unwrap_or_default()
            .to_vec();
        if project_changed {
            // Explicit model capabilities remain useful while refreshing, but the CLI
            // default can differ between project configurations.
            for model in &mut models {
                model.is_default = false;
            }
        }
        self.model[context].model_catalog = ModelCatalogState::Loading { request_id, models };
        if runner.send(command).is_err() {
            self.model[context]
                .model_catalog
                .fail("Runner 不可用。".into());
            return false;
        }
        true
    }

    pub(super) fn restore_catalog_preferences(&mut self) {
        let harness = self.model.conversation.selected_harness;
        self.invalidate_generation_catalogs(harness);
        let profile_id = self
            .model
            .selected_provider_profile()
            .map(|profile| profile.id);
        let legacy_effort = self
            .storage
            .setting("thinking_effort")
            .ok()
            .flatten()
            .and_then(|value| ThinkingEffort::from_str(&value).ok())
            .unwrap_or(ThinkingEffort::Default);
        (
            self.model.conversation.model_override,
            self.model.conversation.effort,
        ) = load_catalog_preferences(&self.storage, harness, profile_id, legacy_effort);
        self.model.conversation.model_override_name = self
            .storage
            .setting(&catalog_model_name_setting_key(harness, profile_id))
            .ok()
            .flatten()
            .filter(|name| !name.is_empty());
        self.model.conversation.model_catalog = ModelCatalogState::Idle;
    }

    pub(super) fn remember_model_name(&mut self) {
        self.remember_model_name_in(self.model.conversation.id)
    }

    pub(super) fn remember_model_name_in(&mut self, context: Uuid) {
        self.model[context].model_override_name =
            self.model[context].model_override.as_ref().and_then(|_| {
                self.model
                    .selected_catalog_model_in(context)
                    .map(|model| model.display_name.clone())
            });
        let profile_id = self
            .model
            .selected_provider_profile_in(context)
            .map(|profile| profile.id);
        let _ = self.storage.set_setting(
            &catalog_model_name_setting_key(self.model[context].selected_harness, profile_id),
            self.model[context]
                .model_override_name
                .as_deref()
                .unwrap_or_default(),
        );
    }

    pub(super) fn persist_catalog_effort(&self) {
        self.persist_catalog_effort_in(self.model.conversation.id)
    }

    pub(super) fn persist_catalog_effort_in(&self, context: Uuid) {
        let harness = self.model[context].selected_harness;
        let profile_id = self
            .model
            .selected_provider_profile_in(context)
            .map(|profile| profile.id);
        let _ = self.storage.set_setting(
            &catalog_effort_setting_key(harness, profile_id),
            self.model[context].effort.as_str(),
        );
    }

    pub(super) fn normalize_catalog_effort(&mut self) -> bool {
        self.normalize_catalog_effort_in(self.model.conversation.id)
    }

    pub(super) fn normalize_catalog_effort_in(&mut self, context: Uuid) -> bool {
        if self.model[context].effort.is_default()
            || self
                .model
                .selected_catalog_model_in(context)
                .is_some_and(|model| model.supports_effort(&self.model[context].effort))
        {
            return false;
        }
        self.model[context].effort = ThinkingEffort::Default;
        self.persist_catalog_effort_in(context);
        true
    }
}
