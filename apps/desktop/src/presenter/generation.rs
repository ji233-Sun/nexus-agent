use super::*;

impl Presenter {
    pub(crate) fn select_generation_harness(
        &mut self,
        kind: GenerationKind,
        harness: HarnessKind,
    ) -> bool {
        if self.model.generation_settings(kind).harness == harness {
            return false;
        }
        if !self.set_generation_settings(
            kind,
            GenerationSettings {
                harness,
                ..Default::default()
            },
        ) {
            return false;
        }
        *self.model.generation_catalog_mut(kind) = ModelCatalogState::Idle;
        true
    }

    pub(crate) fn select_generation_model(
        &mut self,
        kind: GenerationKind,
        model: Option<String>,
    ) -> bool {
        if model.as_deref().is_some_and(|id| {
            !self
                .model
                .generation_catalog(kind)
                .models()
                .is_some_and(|models| {
                    models
                        .iter()
                        .any(|model| model.id == id && model.availability.is_selectable())
                })
        }) {
            return false;
        }
        let settings = self.model.generation_settings(kind);
        let effort = if self
            .model
            .generation_catalog_model(kind, model.as_deref())
            .is_some_and(|descriptor| descriptor.supports_effort(&settings.effort))
        {
            settings.effort
        } else {
            ThinkingEffort::Default
        };
        self.set_generation_settings(
            kind,
            GenerationSettings {
                model,
                effort,
                ..settings.clone()
            },
        )
    }

    pub(crate) fn select_generation_effort(
        &mut self,
        kind: GenerationKind,
        effort: ThinkingEffort,
    ) -> bool {
        let settings = self.model.generation_settings(kind);
        if !effort.is_default()
            && self
                .model
                .generation_catalog_model(kind, settings.model.as_deref())
                .is_none_or(|model| !model.supports_effort(&effort))
        {
            return false;
        }
        self.set_generation_settings(
            kind,
            GenerationSettings {
                effort,
                ..settings.clone()
            },
        )
    }

    pub(super) fn normalize_generation_effort(&mut self, kind: GenerationKind) {
        let settings = self.model.generation_settings(kind);
        if !settings.effort.is_default()
            && self
                .model
                .generation_catalog_model(kind, settings.model.as_deref())
                .is_none_or(|model| !model.supports_effort(&settings.effort))
        {
            self.select_generation_effort(kind, ThinkingEffort::Default);
        }
    }

    fn set_generation_settings(
        &mut self,
        kind: GenerationKind,
        settings: GenerationSettings,
    ) -> bool {
        let value = serde_json::to_string(&settings).expect("serializable generation settings");
        if self
            .storage
            .set_setting(kind.setting_key(), &value)
            .is_err()
        {
            self.model.log_status("无法保存文本生成设置。".into());
            return false;
        }
        match kind {
            GenerationKind::Title => self.model.title_generation = settings,
            GenerationKind::Commit => self.model.commit_message_generation = settings,
        }
        true
    }

    pub(super) fn generation_configuration(
        &self,
        kind: GenerationKind,
    ) -> Result<TextGenerationConfig> {
        let settings = self.model.generation_settings(kind);
        let executable = self
            .storage
            .setting(executable_setting_key(settings.harness))?
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| settings.harness.default_executable().into());
        let model = settings.model.clone().or_else(|| {
            self.model
                .provider_profile_for(settings.harness)
                .and_then(|profile| profile.model.clone())
        });
        Ok(TextGenerationConfig {
            harness: settings.harness,
            executable,
            model,
            effort: settings.effort,
            environment: self.provider_launch_configuration(settings.harness)?,
        })
    }

    pub(crate) fn refresh_generation_model_catalog(&mut self, kind: GenerationKind) -> bool {
        let Some(cwd) = self.model.working_directory().map(str::to_owned) else {
            *self.model.generation_catalog_mut(kind) = ModelCatalogState::Idle;
            return false;
        };
        let configuration = match self.generation_configuration(kind) {
            Ok(configuration) => configuration,
            Err(error) => {
                *self.model.generation_catalog_mut(kind) =
                    ModelCatalogState::NotReady(error.to_string().into());
                return false;
            }
        };
        let request_id = Uuid::new_v4();
        let command = CommandEnvelope::new(Command::ModelCatalogRefresh {
            context_id: Some(self.model.conversation.id),
            request_id,
            purpose: match kind {
                GenerationKind::Title => ModelCatalogPurpose::TitleGeneration,
                GenerationKind::Commit => ModelCatalogPurpose::CommitMessageGeneration,
            },
            harness: configuration.harness,
            executable: configuration.executable,
            cwd,
            environment: configuration.environment,
        });
        let models = self
            .model
            .generation_catalog(kind)
            .models()
            .unwrap_or_default()
            .to_vec();
        *self.model.generation_catalog_mut(kind) =
            ModelCatalogState::Loading { request_id, models };
        if self
            .runner
            .as_ref()
            .is_none_or(|runner| runner.send(command).is_err())
        {
            self.model
                .generation_catalog_mut(kind)
                .fail("Runner 不可用。".into());
            return false;
        }
        true
    }

    pub(super) fn invalidate_generation_catalogs(&mut self, harness: HarnessKind) {
        for kind in GenerationKind::ALL {
            if self.model.generation_settings(kind).harness == harness {
                *self.model.generation_catalog_mut(kind) = ModelCatalogState::Idle;
            }
        }
    }
}
