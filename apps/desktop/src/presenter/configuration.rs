use super::*;

impl Presenter {
    pub(crate) fn probe(&mut self, executable: &str) {
        if self.model.conversation.active_run.is_some() || self.model.harness_manager.busy {
            return;
        }
        let executable = executable.trim().to_owned();
        let harness = self.model.conversation.selected_harness;
        if self.model.conversation.executable != executable {
            self.model.conversation.model_catalog = ModelCatalogState::Idle;
            self.invalidate_generation_catalogs(harness);
        }
        self.model.conversation.executable = executable.clone();
        self.model.harnesses.remove(&harness);
        self.model.harness_manager.installations.remove(&harness);
        if let Some(runner) = &self.runner {
            let _ = runner.send(CommandEnvelope::new(Command::HarnessProbe {
                environment: self.codebuddy_environment(harness),
                harness,
                executable: executable.clone(),
            }));
            let _ = self
                .storage
                .set_setting(executable_setting_key(harness), &executable);
            self.model.log_status(LocalizedText::new(
                "正在探测 {harness}…",
                &[("harness", (harness).to_string())],
            ));
        }
        self.refresh_model_catalog();
    }

    pub(crate) fn select_harness(
        &mut self,
        harness: HarnessKind,
        current_executable: &str,
    ) -> bool {
        if !self.switch_harness(harness, current_executable) {
            return false;
        }
        self.restore_catalog_preferences();
        self.refresh_model_catalog();
        true
    }

    pub(crate) fn select_model_configuration(
        &mut self,
        harness: HarnessKind,
        profile_id: Option<Uuid>,
        current_executable: &str,
    ) -> bool {
        if self.model.conversation.active_run.is_some()
            || (self.model.conversation.selected_harness == harness
                && self
                    .model
                    .selected_provider_profile()
                    .map(|profile| profile.id)
                    == profile_id)
            || profile_id.is_some_and(|id| {
                !self
                    .model
                    .provider_profiles
                    .iter()
                    .any(|profile| profile.id == id && profile.harness == harness)
            })
        {
            return false;
        }
        self.switch_harness(harness, current_executable);
        self.select_provider_profile(profile_id)
    }

    pub(super) fn switch_harness(
        &mut self,
        harness: HarnessKind,
        current_executable: &str,
    ) -> bool {
        if self.model.conversation.active_run.is_some()
            || self.model.conversation.selected_harness == harness
        {
            return false;
        }
        let current_executable = current_executable.trim().to_owned();
        if !current_executable.is_empty() {
            let _ = self.storage.set_setting(
                executable_setting_key(self.model.conversation.selected_harness),
                &current_executable,
            );
        }

        self.model.conversation.selected_harness = harness;
        self.model.conversation.permission_mode = load_permission_mode(&self.storage, harness);
        let _ = self.storage.set_setting(
            "default_harness",
            self.model.conversation.selected_harness.as_str(),
        );
        let executable = self
            .storage
            .setting(executable_setting_key(
                self.model.conversation.selected_harness,
            ))
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                self.model
                    .conversation
                    .selected_harness
                    .default_executable()
                    .into()
            });
        self.model.conversation.executable = executable.clone();
        if let Some(runner) = &self.runner {
            let _ = runner.send(CommandEnvelope::new(Command::HarnessProbe {
                environment: self.codebuddy_environment(self.model.conversation.selected_harness),
                harness: self.model.conversation.selected_harness,
                executable,
            }));
            self.model.log_status(LocalizedText::new(
                "正在探测 {0}…",
                &[("0", (self.model.conversation.selected_harness).to_string())],
            ));
        }
        true
    }

    pub(crate) fn select_permission_mode(&mut self, mode: PermissionMode) {
        if self.model.conversation.active_run.is_some() && !self.model.can_queue() {
            return;
        }
        if self
            .storage
            .set_setting(
                &permission_setting_key(self.model.conversation.selected_harness),
                mode.as_str(),
            )
            .is_err()
        {
            self.model.log_status("无法保存权限设置。".into());
            return;
        }
        self.model.conversation.permission_mode = mode;
    }

    pub(crate) fn harness_transport(&self, harness: HarnessKind) -> nexus_domain::HarnessTransport {
        if harness.default_transport() == nexus_domain::HarnessTransport::Acp
            || self
                .storage
                .setting(&format!("harness_transport.{}", harness.as_str()))
                .ok()
                .flatten()
                .as_deref()
                == Some("acp")
        {
            nexus_domain::HarnessTransport::Acp
        } else {
            nexus_domain::HarnessTransport::Cli
        }
    }

    pub(crate) fn set_harness_transport(&mut self, transport: nexus_domain::HarnessTransport) {
        if self.model.active_run_count() > 0 {
            return;
        }
        let value = if transport == nexus_domain::HarnessTransport::Acp {
            "acp"
        } else {
            "cli"
        };
        if self
            .storage
            .set_setting(
                &format!(
                    "harness_transport.{}",
                    self.model.conversation.selected_harness.as_str()
                ),
                value,
            )
            .is_err()
        {
            self.model.log_status("无法保存接入方式。".into());
        }
    }

    pub(crate) fn codebuddy_region(&self) -> String {
        self.storage
            .setting("codebuddy_region")
            .ok()
            .flatten()
            .filter(|v| matches!(v.as_str(), "internal" | "external"))
            .unwrap_or_default()
    }

    pub(super) fn codebuddy_environment(&self, harness: HarnessKind) -> Vec<EnvironmentVariable> {
        let region = self.codebuddy_region();
        if harness == HarnessKind::Codebuddy && !region.is_empty() {
            vec![EnvironmentVariable {
                name: "CODEBUDDY_INTERNET_ENVIRONMENT".into(),
                value: region,
            }]
        } else {
            vec![]
        }
    }

    pub(crate) fn set_codebuddy_region(&mut self, region: &str) {
        if self.model.active_run_count() > 0 || !matches!(region, "" | "internal" | "external") {
            return;
        }
        if self
            .storage
            .set_setting("codebuddy_region", region)
            .is_err()
        {
            self.model.log_status("无法保存 CodeBuddy 地区。".into());
            return;
        }
        self.model.conversation.model_catalog = ModelCatalogState::Idle;
        let executable = self.model.conversation.executable.clone();
        self.probe(&executable);
    }
}
