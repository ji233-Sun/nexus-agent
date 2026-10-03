use super::*;

impl Presenter {
    pub(crate) fn select_provider_profile(&mut self, profile_id: Option<Uuid>) -> bool {
        if self.model.conversation.active_run.is_some() {
            return false;
        }
        let harness = self.model.conversation.selected_harness;
        if let Some(profile_id) = profile_id {
            let Some(profile_index) = self
                .model
                .provider_profiles
                .iter()
                .position(|profile| profile.id == profile_id && profile.harness == harness)
            else {
                return false;
            };
            let credential_result =
                provider_credential_configured(self.credentials.as_ref(), profile_id);
            self.model.provider_profiles[profile_index].credential_configured =
                credential_result.as_ref().copied().unwrap_or(false);
            let profile_name = self.model.provider_profiles[profile_index].name.clone();
            self.model
                .conversation
                .active_provider_profiles
                .insert(harness, profile_id);
            let _ = self.storage.set_setting(
                &active_profile_setting_key(harness),
                &profile_id.to_string(),
            );
            self.model.log_status(match credential_result {
                Ok(true) => LocalizedText::new(
                    "已启用 Provider Profile：{profile_name}",
                    &[("profile_name", (profile_name).to_string())],
                ),
                Ok(false) => LocalizedText::new(
                    "已选择 {profile_name}，但系统凭据库中没有 API Key。",
                    &[("profile_name", (profile_name).to_string())],
                ),
                Err(error) => LocalizedText::new(
                    "已选择 {profile_name}，但无法读取系统凭据库：{error}",
                    &[
                        ("profile_name", (profile_name).to_string()),
                        ("error", (error).to_string()),
                    ],
                ),
            });
        } else {
            self.model
                .conversation
                .active_provider_profiles
                .remove(&harness);
            let _ = self
                .storage
                .set_setting(&active_profile_setting_key(harness), "");
            self.model.log_status(LocalizedText::new(
                "{harness} 将使用 CLI 当前登录配置。",
                &[("harness", (harness).to_string())],
            ));
        }
        self.restore_catalog_preferences();
        self.refresh_model_catalog();
        true
    }

    pub(crate) fn save_provider_profile(&mut self, draft: ProviderProfileDraft) -> Option<Uuid> {
        if self.model.active_run_count() > 0 {
            return None;
        }
        let harness = self.model.conversation.selected_harness;
        let name = draft.name.trim().to_owned();
        let api_key_env = draft.api_key_env.trim().to_owned();
        let api_key = draft.api_key.trim();
        let base_url = optional_trimmed(draft.base_url);
        let base_url_env = base_url
            .as_ref()
            .map(|_| draft.base_url_env.trim().to_owned());
        let model = optional_trimmed(draft.model);
        if name.is_empty() {
            self.model
                .log_status("Provider Profile 名称不能为空。".into());
            return None;
        }
        if name.chars().count() > PROVIDER_PROFILE_NAME_MAX_CHARS {
            self.model.log_status(LocalizedText::new(
                "Provider Profile 名称不能超过 {PROVIDER_PROFILE_NAME_MAX_CHARS} 个字符。",
                &[(
                    "PROVIDER_PROFILE_NAME_MAX_CHARS",
                    (PROVIDER_PROFILE_NAME_MAX_CHARS).to_string(),
                )],
            ));
            return None;
        }
        if model
            .as_deref()
            .is_some_and(|model| model.chars().count() > PROVIDER_MODEL_MAX_CHARS)
        {
            self.model.log_status(LocalizedText::new(
                "默认模型不能超过 {PROVIDER_MODEL_MAX_CHARS} 个字符。",
                &[(
                    "PROVIDER_MODEL_MAX_CHARS",
                    (PROVIDER_MODEL_MAX_CHARS).to_string(),
                )],
            ));
            return None;
        }
        if !is_secret_environment_name(&api_key_env) {
            self.model
                .log_status("API Key 环境变量必须是安全的 *_API_KEY 或 *_TOKEN 名称。".into());
            return None;
        }
        if base_url_env
            .as_deref()
            .is_some_and(|name| !is_base_url_environment_name(name))
        {
            self.model
                .log_status("Base URL 环境变量必须是安全的 *_BASE_URL 名称。".into());
            return None;
        }
        if self.model.provider_profiles.iter().any(|profile| {
            profile.harness == harness
                && profile.id != draft.id.unwrap_or_default()
                && profile.name.eq_ignore_ascii_case(&name)
        }) {
            self.model.log_status(LocalizedText::new(
                "{harness} 已存在同名 Provider Profile。",
                &[("harness", (harness).to_string())],
            ));
            return None;
        }

        let existing = draft.id.and_then(|id| {
            self.model
                .provider_profiles
                .iter()
                .find(|profile| profile.id == id)
        });
        if existing.is_some_and(|profile| profile.harness != harness) {
            self.model
                .log_status("不能跨 Harness 修改 Provider Profile。".into());
            return None;
        }
        if api_key.is_empty() && existing.is_none() {
            self.model
                .log_status("新建 Provider Profile 时必须填写 API Key。".into());
            return None;
        }

        let profile_id = draft.id.unwrap_or_else(Uuid::new_v4);
        let credential_configured = if api_key.is_empty() {
            match provider_credential_configured(self.credentials.as_ref(), profile_id) {
                Ok(true) => true,
                Ok(false) => {
                    self.model
                        .log_status("系统凭据库中没有 API Key，请重新填写后再保存。".into());
                    return None;
                }
                Err(error) => {
                    self.model.log_status(LocalizedText::new(
                        "无法读取系统凭据库：{error}",
                        &[("error", (error).to_string())],
                    ));
                    return None;
                }
            }
        } else {
            if let Err(error) = self.credentials.set_api_key(profile_id, api_key) {
                self.model.log_status(LocalizedText::new(
                    "无法安全保存 API Key：{error}",
                    &[("error", (error).to_string())],
                ));
                return None;
            }
            true
        };
        let profile = ProviderProfile {
            id: profile_id,
            name,
            harness,
            api_key_env,
            base_url_env,
            base_url,
            model,
            credential_configured,
        };
        let mut profiles = self.model.provider_profiles.clone();
        if let Some(index) = profiles.iter().position(|item| item.id == profile_id) {
            profiles[index] = profile.clone();
        } else {
            profiles.push(profile.clone());
        }
        if let Err(error) = self.storage.set_provider_profiles(&profiles) {
            self.model.log_status(LocalizedText::new(
                "无法保存 Provider Profile：{error}",
                &[("error", (error).to_string())],
            ));
            return None;
        }
        self.model.provider_profiles = profiles;
        self.model
            .conversation
            .active_provider_profiles
            .insert(harness, profile_id);
        let _ = self.storage.set_setting(
            &active_profile_setting_key(harness),
            &profile_id.to_string(),
        );
        self.model.log_status(LocalizedText::new(
            "Provider Profile 已保存并启用：{0}",
            &[("0", (profile.name).to_string())],
        ));
        self.restore_catalog_preferences();
        self.refresh_model_catalog();
        Some(profile_id)
    }

    pub(crate) fn delete_provider_profile(&mut self, profile_id: Uuid) -> bool {
        if self.model.active_run_count() > 0
            || !self
                .model
                .provider_profiles
                .iter()
                .any(|profile| profile.id == profile_id)
        {
            return false;
        }
        if let Err(error) = self.credentials.delete_api_key(profile_id) {
            self.model.log_status(LocalizedText::new(
                "无法从系统凭据库删除 API Key：{error}",
                &[("error", (error).to_string())],
            ));
            return false;
        }
        let mut profiles = self.model.provider_profiles.clone();
        profiles.retain(|profile| profile.id != profile_id);
        if let Err(error) = self.storage.set_provider_profiles(&profiles) {
            self.model.log_status(LocalizedText::new(
                "无法删除 Provider Profile：{error}",
                &[("error", (error).to_string())],
            ));
            return false;
        }
        self.model.provider_profiles = profiles;
        let harnesses = self
            .model
            .conversation
            .active_provider_profiles
            .iter()
            .filter_map(|(harness, active_id)| (*active_id == profile_id).then_some(*harness))
            .collect::<Vec<_>>();
        for harness in harnesses {
            self.model
                .conversation
                .active_provider_profiles
                .remove(&harness);
            let _ = self
                .storage
                .set_setting(&active_profile_setting_key(harness), "");
        }
        self.model.log_status("Provider Profile 已删除。".into());
        self.restore_catalog_preferences();
        self.refresh_model_catalog();
        true
    }

    pub(super) fn provider_launch_configuration_in(
        &self,
        context: Uuid,
        harness: HarnessKind,
    ) -> Result<Vec<EnvironmentVariable>> {
        let mut environment = self.codebuddy_environment(harness);
        let Some(profile) = self.model.provider_profile_in(context, harness) else {
            return Ok(environment);
        };
        let Some(api_key) = self.credentials.api_key(profile.id)? else {
            bail!("系统凭据库中找不到 {} 的 API Key", profile.name);
        };
        environment.push(EnvironmentVariable {
            name: profile.api_key_env.clone(),
            value: api_key,
        });
        if let (Some(name), Some(value)) = (&profile.base_url_env, &profile.base_url) {
            environment.push(EnvironmentVariable {
                name: name.clone(),
                value: value.clone(),
            });
        }
        if environment.iter().any(|variable| !variable.has_safe_name()) {
            bail!("Provider Profile 包含不安全的环境变量名称");
        }
        Ok(environment)
    }
}
