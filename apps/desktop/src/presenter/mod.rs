mod history;
mod remote;
mod runs;

#[cfg(test)]
pub(crate) mod tests;

use crate::{
    infrastructure::{
        codex_history::Client as CodexHistoryClient,
        credentials::{CredentialStore, SystemCredentialStore},
        git::is_git_dirty,
        storage::Storage,
    },
    model::AppModel,
    remote_control::{RemoteCommand, RemoteControl, TOKEN_SETTING_KEY},
};
use anyhow::{Result, bail};
use nexus_domain::{ClaudeModel, HarnessKind, Project, ProviderProfile, ThinkingEffort};
use nexus_protocol::{Command, CommandEnvelope, EnvironmentVariable, EventEnvelope};
use std::{collections::BTreeMap, path::Path, str::FromStr as _, time::Instant};
use uuid::Uuid;

const PROVIDER_PROFILE_NAME_MAX_CHARS: usize = 48;
const PROVIDER_MODEL_MAX_CHARS: usize = 128;

pub(crate) trait RunnerPort {
    fn send(&self, command: CommandEnvelope) -> Result<()>;
    fn drain_events(&self) -> Vec<EventEnvelope>;
}

pub(crate) struct Presenter {
    model: AppModel,
    active_run_started_at: Option<Instant>,
    storage: Storage,
    runner: Option<Box<dyn RunnerPort>>,
    codex_history_client: Option<CodexHistoryClient>,
    codex_history_executable: Option<String>,
    remote_control: Option<RemoteControl>,
    remote_control_error: Option<String>,
    credentials: Box<dyn CredentialStore>,
}

pub(crate) struct ProviderProfileDraft {
    pub(crate) id: Option<Uuid>,
    pub(crate) name: String,
    pub(crate) api_key_env: String,
    pub(crate) api_key: String,
    pub(crate) base_url_env: String,
    pub(crate) base_url: String,
    pub(crate) model: String,
}

impl Presenter {
    pub(crate) fn new(
        storage: Storage,
        runner: Result<Box<dyn RunnerPort>>,
        storage_error: Option<String>,
    ) -> Self {
        Self::new_with_credentials(
            storage,
            runner,
            storage_error,
            Box::new(SystemCredentialStore),
        )
    }

    fn new_with_credentials(
        storage: Storage,
        runner: Result<Box<dyn RunnerPort>>,
        storage_error: Option<String>,
        credentials: Box<dyn CredentialStore>,
    ) -> Self {
        let projects = storage.projects().unwrap_or_default();
        let selected_harness = storage
            .setting("default_harness")
            .ok()
            .flatten()
            .and_then(|value| HarnessKind::from_str(&value).ok())
            .unwrap_or_default();
        let model = storage
            .setting("claude_model")
            .ok()
            .flatten()
            .and_then(|value| ClaudeModel::from_str(&value).ok())
            .unwrap_or_default();
        let effort = storage
            .setting("thinking_effort")
            .ok()
            .flatten()
            .and_then(|value| ThinkingEffort::from_str(&value).ok())
            .unwrap_or_default();
        let executable = storage
            .setting(executable_setting_key(selected_harness))
            .ok()
            .flatten()
            .unwrap_or_else(|| selected_harness.default_executable().into());
        let mut provider_profiles = storage.provider_profiles().unwrap_or_default();
        let mut credential_store_error = None;
        for profile in &mut provider_profiles {
            match provider_credential_configured(credentials.as_ref(), profile.id) {
                Ok(configured) => profile.credential_configured = configured,
                Err(error) => {
                    profile.credential_configured = false;
                    credential_store_error.get_or_insert_with(|| error.to_string());
                }
            }
        }
        let active_provider_profiles = HarnessKind::ALL
            .into_iter()
            .filter_map(|harness| {
                let profile_id = storage
                    .setting(&active_profile_setting_key(harness))
                    .ok()
                    .flatten()
                    .and_then(|value| Uuid::parse_str(&value).ok())?;
                provider_profiles
                    .iter()
                    .any(|profile| profile.id == profile_id && profile.harness == harness)
                    .then_some((harness, profile_id))
            })
            .collect::<BTreeMap<_, _>>();
        let remote_token = storage
            .setting(TOKEN_SETTING_KEY)
            .ok()
            .flatten()
            .filter(|token| !token.is_empty())
            .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
        let _ = storage.set_setting(TOKEN_SETTING_KEY, &remote_token);
        let (remote_control, remote_control_error) = match RemoteControl::start(remote_token) {
            Ok(remote_control) => (Some(remote_control), None),
            Err(error) => (None, Some(error.to_string())),
        };

        let has_storage_error = storage_error.is_some();
        let (runner, runner_error) = match runner {
            Ok(runner) => (Some(runner), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let mut presenter = Self {
            storage,
            runner,
            active_run_started_at: None,
            model: AppModel {
                projects,
                selected_harness,
                claude_model: model,
                effort,
                executable,
                provider_profiles,
                active_provider_profiles,
                status: storage_error
                    .or_else(|| {
                        credential_store_error.map(|error| format!("无法读取系统凭据库：{error}"))
                    })
                    .unwrap_or_else(|| "正在连接本地 Runner…".into()),
                ..AppModel::default()
            },
            codex_history_client: None,
            codex_history_executable: None,
            remote_control,
            remote_control_error,
            credentials,
        };
        if let Some(runner) = &presenter.runner {
            let _ = runner.send(CommandEnvelope::new(Command::RunnerHello));
            for harness in HarnessKind::ALL {
                let executable = presenter
                    .storage
                    .setting(executable_setting_key(harness))
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| harness.default_executable().into());
                let _ = runner.send(CommandEnvelope::new(Command::HarnessProbe {
                    harness,
                    executable,
                }));
            }
        } else if !has_storage_error {
            presenter.model.status = runner_error.unwrap_or_default();
        }
        presenter
    }

    pub(crate) fn model(&self) -> &AppModel {
        &self.model
    }

    pub(crate) fn drain_events(&mut self) -> bool {
        let runner_events = self
            .runner
            .as_ref()
            .map(|runner| runner.drain_events())
            .unwrap_or_default();
        let history_events = self
            .codex_history_client
            .as_ref()
            .map(CodexHistoryClient::drain_events)
            .unwrap_or_default();
        let remote_commands = self
            .remote_control
            .as_ref()
            .map(RemoteControl::drain_commands)
            .unwrap_or_default();
        let mut changed = !runner_events.is_empty() || !history_events.is_empty();
        for envelope in runner_events {
            self.handle_event(envelope.event);
        }
        for event in history_events {
            self.handle_codex_history_event(event);
        }
        for command in remote_commands {
            changed |= self.handle_remote_command(command);
        }
        if changed {
            self.notify_remote_changed();
        }
        changed
    }

    pub(crate) fn open_project(&mut self, path: &Path) {
        match self.storage.open_project(path) {
            Ok(project) => {
                self.select_project(project);
                self.model.projects = self.storage.projects().unwrap_or_default();
            }
            Err(error) => self.model.status = format!("无法打开项目：{error}"),
        }
    }

    pub(crate) fn new_task(&mut self) {
        if self.model.active_run.is_some() || self.model.selected_project.is_none() {
            return;
        }
        self.model.selected_task = None;
        self.model.selected_codex_thread = None;
        self.model.messages.clear();
        self.model.codex_history_messages.clear();
        self.model.codex_thread_loading = false;
        self.model.streaming_text.clear();
        self.model.status = "已准备好新任务。".into();
    }

    pub(crate) fn select_project(&mut self, project: Project) {
        self.model.project_dirty = is_git_dirty(Path::new(&project.canonical_path));
        self.model.selected_project = Some(project);
        self.model.selected_task = None;
        self.model.selected_codex_thread = None;
        self.model.messages.clear();
        self.model.codex_history_messages.clear();
        self.model.codex_thread_loading = false;
        self.model.streaming_text.clear();
        self.reload_tasks();
    }

    fn reload_tasks(&mut self) {
        self.model.tasks = self
            .model
            .selected_project
            .as_ref()
            .and_then(|project| self.storage.tasks(project.id).ok())
            .unwrap_or_default();
    }

    pub(crate) fn select_task(&mut self, task_id: Uuid) {
        self.model.selected_task = Some(task_id);
        self.model.selected_codex_thread = None;
        self.model.codex_history_messages.clear();
        self.model.codex_thread_loading = false;
        self.model.messages = self.storage.messages(task_id).unwrap_or_default();
        if let Ok(Some(config)) = self.storage.conversation_config(task_id) {
            self.model.selected_harness = config.harness;
            self.model.effort = config.effort;
            if config.harness == HarnessKind::Claude {
                self.model.claude_model = ClaudeModel::from_str(&config.model).unwrap_or_default();
            }
            let executable = if config.executable.is_empty() {
                self.storage
                    .setting(executable_setting_key(config.harness))
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| config.harness.default_executable().into())
            } else {
                config.executable
            };
            self.model.executable = executable;
        }
    }

    pub(crate) fn probe(&mut self, executable: &str) {
        let executable = executable.trim().to_owned();
        let harness = self.model.selected_harness;
        if let Some(runner) = &self.runner {
            let _ = runner.send(CommandEnvelope::new(Command::HarnessProbe {
                harness,
                executable: executable.clone(),
            }));
            let _ = self
                .storage
                .set_setting(executable_setting_key(harness), &executable);
            self.model.status = format!("正在探测 {harness}…");
        }
    }

    pub(crate) fn select_harness(
        &mut self,
        harness: HarnessKind,
        current_executable: &str,
    ) -> bool {
        if self.model.active_run.is_some() || self.model.selected_harness == harness {
            return false;
        }
        let current_executable = current_executable.trim().to_owned();
        if !current_executable.is_empty() {
            let _ = self.storage.set_setting(
                executable_setting_key(self.model.selected_harness),
                &current_executable,
            );
        }

        self.model.selected_harness = harness;
        let _ = self
            .storage
            .set_setting("default_harness", self.model.selected_harness.as_str());
        let executable = self
            .storage
            .setting(executable_setting_key(self.model.selected_harness))
            .ok()
            .flatten()
            .unwrap_or_else(|| self.model.selected_harness.default_executable().into());
        self.model.executable = executable.clone();
        if let Some(runner) = &self.runner {
            let _ = runner.send(CommandEnvelope::new(Command::HarnessProbe {
                harness: self.model.selected_harness,
                executable,
            }));
            self.model.status = format!("正在探测 {}…", self.model.selected_harness);
        }
        true
    }

    pub(crate) fn select_model(&mut self, model: ClaudeModel) {
        if self.model.active_run.is_some()
            || self.model.selected_harness != HarnessKind::Claude
            || self.model.claude_model == model
        {
            return;
        }
        self.model.claude_model = model;
        let _ = self
            .storage
            .set_setting("claude_model", self.model.claude_model.as_str());
    }

    pub(crate) fn select_effort(&mut self, effort: ThinkingEffort) {
        if self.model.active_run.is_some() || self.model.effort == effort {
            return;
        }
        self.model.effort = effort;
        let _ = self
            .storage
            .set_setting("thinking_effort", self.model.effort.as_str());
    }

    pub(crate) fn select_provider_profile(&mut self, profile_id: Option<Uuid>) -> bool {
        if self.model.active_run.is_some() {
            return false;
        }
        let harness = self.model.selected_harness;
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
                .active_provider_profiles
                .insert(harness, profile_id);
            let _ = self.storage.set_setting(
                &active_profile_setting_key(harness),
                &profile_id.to_string(),
            );
            self.model.status = match credential_result {
                Ok(true) => format!("已启用 Provider Profile：{profile_name}"),
                Ok(false) => {
                    format!("已选择 {profile_name}，但系统凭据库中没有 API Key。")
                }
                Err(error) => format!("已选择 {profile_name}，但无法读取系统凭据库：{error}"),
            };
        } else {
            self.model.active_provider_profiles.remove(&harness);
            let _ = self
                .storage
                .set_setting(&active_profile_setting_key(harness), "");
            self.model.status = format!("{harness} 将使用 CLI 当前登录配置。");
        }
        true
    }

    pub(crate) fn save_provider_profile(&mut self, draft: ProviderProfileDraft) -> Option<Uuid> {
        if self.model.active_run.is_some() {
            return None;
        }
        let harness = self.model.selected_harness;
        let name = draft.name.trim().to_owned();
        let api_key_env = draft.api_key_env.trim().to_owned();
        let api_key = draft.api_key.trim();
        let base_url = optional_trimmed(draft.base_url);
        let base_url_env = base_url
            .as_ref()
            .map(|_| draft.base_url_env.trim().to_owned());
        let model = optional_trimmed(draft.model);
        if name.is_empty() {
            self.model.status = "Provider Profile 名称不能为空。".into();
            return None;
        }
        if name.chars().count() > PROVIDER_PROFILE_NAME_MAX_CHARS {
            self.model.status =
                format!("Provider Profile 名称不能超过 {PROVIDER_PROFILE_NAME_MAX_CHARS} 个字符。");
            return None;
        }
        if model
            .as_deref()
            .is_some_and(|model| model.chars().count() > PROVIDER_MODEL_MAX_CHARS)
        {
            self.model.status = format!("默认模型不能超过 {PROVIDER_MODEL_MAX_CHARS} 个字符。");
            return None;
        }
        if !is_secret_environment_name(&api_key_env) {
            self.model.status = "API Key 环境变量必须是安全的 *_API_KEY 或 *_TOKEN 名称。".into();
            return None;
        }
        if base_url_env
            .as_deref()
            .is_some_and(|name| !is_base_url_environment_name(name))
        {
            self.model.status = "Base URL 环境变量必须是安全的 *_BASE_URL 名称。".into();
            return None;
        }
        if self.model.provider_profiles.iter().any(|profile| {
            profile.harness == harness
                && profile.id != draft.id.unwrap_or_default()
                && profile.name.eq_ignore_ascii_case(&name)
        }) {
            self.model.status = format!("{harness} 已存在同名 Provider Profile。");
            return None;
        }

        let existing = draft.id.and_then(|id| {
            self.model
                .provider_profiles
                .iter()
                .find(|profile| profile.id == id)
        });
        if existing.is_some_and(|profile| profile.harness != harness) {
            self.model.status = "不能跨 Harness 修改 Provider Profile。".into();
            return None;
        }
        if api_key.is_empty() && existing.is_none() {
            self.model.status = "新建 Provider Profile 时必须填写 API Key。".into();
            return None;
        }

        let profile_id = draft.id.unwrap_or_else(Uuid::new_v4);
        let credential_configured = if api_key.is_empty() {
            match provider_credential_configured(self.credentials.as_ref(), profile_id) {
                Ok(true) => true,
                Ok(false) => {
                    self.model.status = "系统凭据库中没有 API Key，请重新填写后再保存。".into();
                    return None;
                }
                Err(error) => {
                    self.model.status = format!("无法读取系统凭据库：{error}");
                    return None;
                }
            }
        } else {
            if let Err(error) = self.credentials.set_api_key(profile_id, api_key) {
                self.model.status = format!("无法安全保存 API Key：{error}");
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
            self.model.status = format!("无法保存 Provider Profile：{error}");
            return None;
        }
        self.model.provider_profiles = profiles;
        self.model
            .active_provider_profiles
            .insert(harness, profile_id);
        let _ = self.storage.set_setting(
            &active_profile_setting_key(harness),
            &profile_id.to_string(),
        );
        self.model.status = format!("Provider Profile 已保存并启用：{}", profile.name);
        Some(profile_id)
    }

    pub(crate) fn delete_provider_profile(&mut self, profile_id: Uuid) -> bool {
        if self.model.active_run.is_some()
            || !self
                .model
                .provider_profiles
                .iter()
                .any(|profile| profile.id == profile_id)
        {
            return false;
        }
        if let Err(error) = self.credentials.delete_api_key(profile_id) {
            self.model.status = format!("无法从系统凭据库删除 API Key：{error}");
            return false;
        }
        let mut profiles = self.model.provider_profiles.clone();
        profiles.retain(|profile| profile.id != profile_id);
        if let Err(error) = self.storage.set_provider_profiles(&profiles) {
            self.model.status = format!("无法删除 Provider Profile：{error}");
            return false;
        }
        self.model.provider_profiles = profiles;
        let harnesses = self
            .model
            .active_provider_profiles
            .iter()
            .filter_map(|(harness, active_id)| (*active_id == profile_id).then_some(*harness))
            .collect::<Vec<_>>();
        for harness in harnesses {
            self.model.active_provider_profiles.remove(&harness);
            let _ = self
                .storage
                .set_setting(&active_profile_setting_key(harness), "");
        }
        self.model.status = "Provider Profile 已删除。".into();
        true
    }

    fn provider_launch_configuration(&self) -> Result<(Vec<EnvironmentVariable>, Option<String>)> {
        let Some(profile) = self.model.selected_provider_profile() else {
            return Ok((Vec::new(), None));
        };
        let Some(api_key) = self.credentials.api_key(profile.id)? else {
            bail!("系统凭据库中找不到 {} 的 API Key", profile.name);
        };
        let mut environment = vec![EnvironmentVariable {
            name: profile.api_key_env.clone(),
            value: api_key,
        }];
        if let (Some(name), Some(value)) = (&profile.base_url_env, &profile.base_url) {
            environment.push(EnvironmentVariable {
                name: name.clone(),
                value: value.clone(),
            });
        }
        if environment.iter().any(|variable| !variable.has_safe_name()) {
            bail!("Provider Profile 包含不安全的环境变量名称");
        }
        Ok((environment, profile.model.clone()))
    }
}

fn executable_setting_key(harness: HarnessKind) -> &'static str {
    match harness {
        HarnessKind::Claude => "claude_executable",
        HarnessKind::Codex => "codex_executable",
        HarnessKind::Omp => "omp_executable",
    }
}

fn active_profile_setting_key(harness: HarnessKind) -> String {
    format!("active_provider_profile_{}", harness.as_str())
}

fn optional_trimmed(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn is_secret_environment_name(name: &str) -> bool {
    EnvironmentVariable {
        name: name.to_owned(),
        value: String::new(),
    }
    .has_safe_name()
        && (name.ends_with("_API_KEY") || name.ends_with("_TOKEN"))
}

fn is_base_url_environment_name(name: &str) -> bool {
    EnvironmentVariable {
        name: name.to_owned(),
        value: String::new(),
    }
    .has_safe_name()
        && name.ends_with("_BASE_URL")
}

fn provider_credential_configured(
    credentials: &dyn CredentialStore,
    profile_id: Uuid,
) -> Result<bool> {
    Ok(credentials
        .api_key(profile_id)?
        .is_some_and(|api_key| !api_key.is_empty()))
}
