mod cnb;
mod generation;
mod harness_installation;
mod history;
mod remote;
mod runs;
mod updates;
mod voice;
mod workspace;

#[cfg(test)]
pub(crate) mod tests;

use crate::{
    i18n::{Language, LocalizedText},
    infrastructure::{
        codex_history::Client as CodexHistoryClient,
        credentials::{CredentialStore, SystemCredentialStore},
        storage::Storage,
    },
    model::{
        AppModel, AppearanceSettings, ConversationState, GenerationKind, GenerationSettings,
        ModelCatalogState,
        updates::{UpdateChannel, UpdateModel, UpdateState},
    },
    remote_control::{RemoteCommand, RemoteControl, TOKEN_SETTING_KEY},
};
use anyhow::{Result, bail};
use nexus_domain::{
    ClaudeModel, HarnessKind, PermissionMode, Project, ProviderProfile, ThinkingEffort,
    UserAskAnswer,
};
use nexus_protocol::{
    Command, CommandEnvelope, EnvironmentVariable, EventEnvelope, ModelCatalogPurpose,
    TextGenerationConfig,
};
use std::{collections::BTreeMap, path::Path, str::FromStr as _};
use uuid::Uuid;

const PROVIDER_PROFILE_NAME_MAX_CHARS: usize = 48;
const PROVIDER_MODEL_MAX_CHARS: usize = 128;

pub(crate) trait RunnerPort {
    fn send(&self, command: CommandEnvelope) -> Result<()>;
    fn drain_events(&self) -> Vec<EventEnvelope>;

    #[cfg_attr(not(test), allow(dead_code))]
    fn answer_user_ask(
        &self,
        run_id: Uuid,
        request_id: Uuid,
        answers: Vec<UserAskAnswer>,
    ) -> Result<()> {
        self.send(CommandEnvelope::new(Command::RunUserAskAnswer {
            run_id,
            request_id,
            answers,
        }))
    }
}

pub(crate) struct Presenter {
    cnb_client: crate::infrastructure::cnb::Client,
    model: AppModel,
    voice_worker: Option<crate::infrastructure::voice::Worker>,
    storage: Storage,
    runner: Option<Box<dyn RunnerPort>>,
    codex_history_client: Option<CodexHistoryClient>,
    codex_history_executable: Option<String>,
    remote_control: Option<RemoteControl>,
    remote_control_error: Option<String>,
    credentials: Box<dyn CredentialStore>,
    update_events: Option<std::sync::mpsc::Receiver<UpdateState>>,
    installation_worker: Option<crate::infrastructure::harness_installation::Worker>,
    cli_installation_result: Option<std::sync::mpsc::Receiver<Result<()>>>,
    workspace_events: Option<std::sync::mpsc::Receiver<workspace::WorkspaceEvent>>,
    worktree_root: Result<std::path::PathBuf>,
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
        let archived_tasks = storage.archived_tasks().unwrap_or_default();
        let language = storage
            .setting("language")
            .ok()
            .flatten()
            .map(|value| Language::from_setting(&value))
            .unwrap_or_default();
        let appearance = storage
            .setting("appearance")
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default();
        let updates = UpdateModel {
            channel: storage
                .setting("update_channel")
                .ok()
                .flatten()
                .and_then(|value| UpdateChannel::from_setting(&value))
                .unwrap_or_default(),
            check_on_startup: storage
                .setting("update_check_on_startup")
                .ok()
                .flatten()
                .is_none_or(|value| value != "false"),
            ..UpdateModel::default()
        };
        let selected_harness = storage
            .setting("default_harness")
            .ok()
            .flatten()
            .and_then(|value| HarnessKind::from_str(&value).ok())
            .unwrap_or_default();
        let stored_effort = storage
            .setting("thinking_effort")
            .ok()
            .flatten()
            .and_then(|value| ThinkingEffort::from_str(&value).ok())
            .unwrap_or(ThinkingEffort::Default);
        let permission_mode = load_permission_mode(&storage, selected_harness);
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
        let profile_id = active_provider_profiles.get(&selected_harness).copied();
        let (model_override, effort) =
            load_catalog_preferences(&storage, selected_harness, profile_id, stored_effort);
        let title_generation = storage
            .setting("title_generation")
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_else(|| GenerationSettings {
                harness: selected_harness,
                model: model_override.clone(),
                ..GenerationSettings::default()
            });
        let _ = storage.set_setting(
            "title_generation",
            &serde_json::to_string(&title_generation).expect("serializable title settings"),
        );
        let commit_message_generation = storage
            .setting("commit_message_generation")
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_else(|| GenerationSettings {
                harness: selected_harness,
                ..Default::default()
            });
        let _ = storage.set_setting(
            "commit_message_generation",
            &serde_json::to_string(&commit_message_generation)
                .expect("serializable generation settings"),
        );
        let model_override_name = storage
            .setting(&catalog_model_name_setting_key(
                selected_harness,
                profile_id,
            ))
            .ok()
            .flatten()
            .filter(|name| !name.is_empty());
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
            cnb_client: crate::infrastructure::cnb::Client::default(),
            voice_worker: None,
            storage,
            runner,
            model: AppModel {
                language,
                appearance,
                title_generation,
                commit_message_generation,
                updates,
                projects,
                archived_tasks,
                provider_profiles,
                conversation: ConversationState {
                    selected_harness,
                    permission_mode,
                    model_override,
                    model_override_name,
                    effort,
                    executable,
                    active_provider_profiles,
                    status: storage_error
                        .map(LocalizedText::from)
                        .or_else(|| {
                            credential_store_error.map(|error| {
                                LocalizedText::new(
                                    "无法读取系统凭据库：{error}",
                                    &[("error", (error).to_string())],
                                )
                            })
                        })
                        .unwrap_or_else(|| "正在连接本地 Runner…".into()),
                    ..ConversationState::default()
                },
                ..AppModel::default()
            },
            codex_history_client: None,
            codex_history_executable: None,
            remote_control,
            remote_control_error,
            credentials,
            update_events: None,
            installation_worker: None,
            cli_installation_result: None,
            workspace_events: None,
            worktree_root: crate::infrastructure::paths::worktree_directory(),
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
            presenter.model.status = runner_error.unwrap_or_default().into();
        }
        presenter.load_voice_settings();
        presenter.model.cnb.enabled = presenter
            .storage
            .setting("cnb_enabled")
            .ok()
            .flatten()
            .is_none_or(|value| value != "false");
        presenter
    }

    pub(crate) fn model(&self) -> &AppModel {
        &self.model
    }

    pub(crate) fn install_cli(&mut self) {
        if self.cli_installation_result.is_some() || self.model.updates.state.is_installing() {
            return;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        match std::thread::Builder::new()
            .name("nexus-cli-install".into())
            .spawn(move || {
                let _ = sender.send(crate::infrastructure::cli_installation::install());
            }) {
            Ok(_) => {
                self.cli_installation_result = Some(receiver);
                self.model.cli_installation_busy = true;
                self.model.cli_installation_message = None;
            }
            Err(error) => {
                self.model.cli_installation_message = Some(LocalizedText::new(
                    "CLI 安装失败：{error}",
                    &[("error", error.to_string())],
                ))
            }
        }
    }

    fn drain_cli_installation_result(&mut self) -> bool {
        let Some(receiver) = &self.cli_installation_result else {
            return false;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err(anyhow::anyhow!("CLI 安装进程已中断"))
            }
        };
        self.cli_installation_result = None;
        self.model.cli_installation_busy = false;
        self.model.cli_installation_message = Some(match result {
            Ok(()) => "CLI 已安装。重新打开终端后可运行 nexus-desktop .".into(),
            Err(error) => {
                LocalizedText::new("CLI 安装失败：{error}", &[("error", format!("{error:#}"))])
            }
        });
        true
    }

    pub(crate) fn set_language(&mut self, language: Language) -> bool {
        match self.storage.set_setting("language", language.as_str()) {
            Ok(()) => {
                self.model.language = language;
                true
            }
            Err(error) => {
                self.model.status = LocalizedText::new(
                    "无法保存语言偏好：{error}",
                    &[("error", error.to_string())],
                );
                false
            }
        }
    }

    pub(crate) fn set_appearance(&mut self, appearance: AppearanceSettings) -> bool {
        let result = serde_json::to_string(&appearance)
            .map_err(anyhow::Error::from)
            .and_then(|value| self.storage.set_setting("appearance", &value));
        match result {
            Ok(()) => {
                self.model.appearance = appearance;
                true
            }
            Err(error) => {
                self.model.status = LocalizedText::new(
                    "无法保存外观偏好：{error}",
                    &[("error", (error).to_string())],
                );
                false
            }
        }
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
        let mut changed = self.drain_workspace_events()
            || !runner_events.is_empty()
            || !history_events.is_empty();
        changed |= self.drain_cli_installation_result();
        for envelope in runner_events {
            if envelope.protocol_version != nexus_protocol::PROTOCOL_VERSION {
                self.model.status = "Desktop 与 Runner 协议版本不匹配，请重启或更新应用。".into();
                continue;
            }
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
                self.reload_projects();
            }
            Err(error) => {
                self.model.status =
                    LocalizedText::new("无法打开项目：{error}", &[("error", (error).to_string())])
            }
        }
    }

    pub(crate) fn new_task(&mut self) {
        if self.model.selected_project.is_none() {
            return;
        }
        self.model.cnb.opened = false;
        self.cancel_voice();
        self.model.fresh_conversation();
        self.model.selected_task = None;
        self.reset_workspace_draft();
        self.model.selected_codex_thread = None;
        self.model.messages.clear();
        self.model.codex_history_messages.clear();
        self.model.codex_thread_loading = false;
        self.model.streaming_text.clear();
        self.model.status = "已准备好新任务。".into();
        self.model.permission_mode =
            load_permission_mode(&self.storage, self.model.selected_harness);
        self.refresh_model_catalog();
    }

    pub(crate) fn select_project(&mut self, project: Project) {
        self.cancel_voice();
        self.model.fresh_conversation();
        self.model.permission_mode =
            load_permission_mode(&self.storage, self.model.selected_harness);
        self.model.selected_project = Some(project);
        self.model.selected_task = None;
        self.reset_workspace_draft();
        self.reload_workspaces();
        self.model.selected_codex_thread = None;
        self.model.messages.clear();
        self.model.codex_history_messages.clear();
        self.model.codex_thread_loading = false;
        self.model.streaming_text.clear();
        self.reload_tasks();
        self.refresh_model_catalog();
        self.reset_cnb_project();
    }

    fn reload_projects(&mut self) {
        let Ok(mut projects) = self.storage.projects() else {
            return;
        };
        if let Some(selected_project) = &self.model.selected_project
            && !projects
                .iter()
                .any(|project| project.id == selected_project.id)
        {
            projects.push(selected_project.clone());
        }
        self.model.projects = projects;
    }

    fn reload_tasks(&mut self) {
        self.model.tasks = self
            .model
            .selected_project
            .as_ref()
            .and_then(|project| self.storage.tasks(project.id).ok())
            .unwrap_or_default();
        self.model.archived_tasks = self.storage.archived_tasks().unwrap_or_default();
    }

    pub(crate) fn select_task(&mut self, task_id: Uuid) {
        self.model.cnb.opened = false;
        self.cancel_voice();
        if self.model.selected_task != Some(task_id) {
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
        self.model.selected_task = Some(task_id);
        self.model.selected_workspace = self.storage.task_workspace(task_id).ok().flatten();
        self.reload_workspaces();
        self.model.selected_codex_thread = None;
        self.model.codex_history_messages.clear();
        self.model.codex_thread_loading = false;
        self.model.messages = self.storage.messages(task_id).unwrap_or_default();
        self.reload_tasks();
        if self.model.active_run.is_some() {
            return;
        }
        if let Ok(Some(config)) = self.storage.conversation_config(task_id) {
            self.model.selected_harness = config.harness;
            self.model.permission_mode = config.permission_mode;
            self.model.effort = normalize_effort_for_harness(config.harness, config.effort);
            self.model.model_override = (config.model != "default").then_some(config.model.clone());
            self.model.model_override_name = None;
            self.model.model_catalog = ModelCatalogState::Idle;
            let executable = if config.executable.is_empty() {
                self.storage
                    .setting(executable_setting_key(config.harness))
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| config.harness.default_executable().into())
            } else {
                config.executable
            };
            self.model.executable = executable.clone();
            if !self
                .model
                .selected_probe()
                .is_some_and(|probe| probe.executable == executable)
            {
                self.model.harnesses.remove(&config.harness);
                self.model.status = if let Some(runner) = &self.runner
                    && runner
                        .send(CommandEnvelope::new(Command::HarnessProbe {
                            harness: config.harness,
                            executable,
                        }))
                        .is_ok()
                {
                    LocalizedText::new("正在探测 {0}…", &[("0", (config.harness).to_string())])
                } else {
                    "Runner 不可用，无法探测任务使用的可执行文件。".into()
                };
            }
            self.refresh_model_catalog();
        }
    }

    pub(crate) fn archive_task(&mut self, task_id: Uuid) -> bool {
        if self.model.task_running(task_id) || self.model.workspace_busy {
            return false;
        }
        if let Err(error) = self.storage.archive_task(task_id) {
            self.model.status =
                LocalizedText::new("无法归档对话：{error}", &[("error", (error).to_string())]);
            return false;
        }
        if self.model.selected_task == Some(task_id) {
            self.new_task();
        }
        self.reload_tasks();
        self.model.status = "对话已归档。".into();
        true
    }

    pub(crate) fn restore_task(&mut self, task_id: Uuid) -> bool {
        if self.model.task_running(task_id) || self.model.workspace_busy {
            return false;
        }
        if let Err(error) = self.storage.restore_task(task_id) {
            self.model.status =
                LocalizedText::new("无法取消归档：{error}", &[("error", (error).to_string())]);
            return false;
        }
        self.reload_projects();
        self.reload_tasks();
        self.model.status = "对话已恢复。".into();
        true
    }

    pub(crate) fn delete_task(&mut self, task_id: Uuid) -> bool {
        if self.model.task_running(task_id) || self.model.workspace_busy {
            return false;
        }
        if let Err(error) = self.storage.delete_task(task_id) {
            self.model.status =
                LocalizedText::new("无法删除对话：{error}", &[("error", (error).to_string())]);
            return false;
        }
        self.model
            .queued_messages
            .retain(|message| message.task_id != task_id);
        if self.model.selected_task == Some(task_id) {
            self.new_task();
        }
        self.model
            .conversations
            .retain(|_, conversation| conversation.selected_task != Some(task_id));
        self.reload_projects();
        self.reload_tasks();
        self.model.status = "对话已删除。".into();
        true
    }

    pub(crate) fn delete_archived_tasks(&mut self) -> bool {
        if self.model.active_run.is_some() {
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
                self.model.status = LocalizedText::new(
                    "已删除 {count} 个归档对话。",
                    &[("count", (count).to_string())],
                );
                true
            }
            Err(error) => {
                self.model.status = LocalizedText::new(
                    "无法清空归档对话：{error}",
                    &[("error", (error).to_string())],
                );
                false
            }
        }
    }

    pub(crate) fn probe(&mut self, executable: &str) {
        if self.model.active_run.is_some() || self.model.harness_manager.busy {
            return;
        }
        let executable = executable.trim().to_owned();
        let harness = self.model.selected_harness;
        if self.model.executable != executable {
            self.model.model_catalog = ModelCatalogState::Idle;
            self.invalidate_generation_catalogs(harness);
        }
        self.model.executable = executable.clone();
        self.model.harnesses.remove(&harness);
        self.model.harness_manager.installations.remove(&harness);
        if let Some(runner) = &self.runner {
            let _ = runner.send(CommandEnvelope::new(Command::HarnessProbe {
                harness,
                executable: executable.clone(),
            }));
            let _ = self
                .storage
                .set_setting(executable_setting_key(harness), &executable);
            self.model.status =
                LocalizedText::new("正在探测 {harness}…", &[("harness", (harness).to_string())]);
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
        if self.model.active_run.is_some()
            || (self.model.selected_harness == harness
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

    fn switch_harness(&mut self, harness: HarnessKind, current_executable: &str) -> bool {
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
        self.model.permission_mode = load_permission_mode(&self.storage, harness);
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
            self.model.status = LocalizedText::new(
                "正在探测 {0}…",
                &[("0", (self.model.selected_harness).to_string())],
            );
        }
        true
    }

    pub(crate) fn select_catalog_model(&mut self, model_id: Option<String>) {
        if self.model.active_run.is_some() {
            return;
        }
        if model_id.as_deref().is_some_and(|id| {
            !self.model.model_catalog.models().is_some_and(|models| {
                models
                    .iter()
                    .any(|model| model.id == id && model.availability.is_selectable())
            })
        }) {
            self.model.status = LocalizedText::new(
                "所选模型不在当前 {0} 目录中，请刷新后重试。",
                &[("0", (self.model.selected_harness).to_string())],
            );
            return;
        }
        if self.model.model_override == model_id {
            return;
        }
        self.model.model_override = model_id;
        self.remember_model_name();
        let harness = self.model.selected_harness;
        let profile_id = self
            .model
            .selected_provider_profile()
            .map(|profile| profile.id);
        let _ = self.storage.set_setting(
            &catalog_model_setting_key(harness, profile_id),
            self.model.model_override.as_deref().unwrap_or_default(),
        );
        let effort_reset = self.normalize_catalog_effort();
        self.model.status = if effort_reset {
            "模型已切换；原 effort 不受支持，已恢复为模型默认。".into()
        } else if let Some(model) = &self.model.model_override {
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
        };
    }

    pub(crate) fn select_permission_mode(&mut self, mode: PermissionMode) {
        if self.model.active_run.is_some() && !self.model.can_queue() {
            return;
        }
        if self
            .storage
            .set_setting(
                &permission_setting_key(self.model.selected_harness),
                mode.as_str(),
            )
            .is_err()
        {
            self.model.status = "无法保存权限设置。".into();
            return;
        }
        self.model.permission_mode = mode;
    }

    pub(crate) fn select_effort(&mut self, effort: ThinkingEffort) {
        if self.model.active_run.is_some() || self.model.effort == effort {
            return;
        }
        if !effort.is_default()
            && self
                .model
                .selected_catalog_model()
                .is_none_or(|model| !model.supports_effort(&effort))
        {
            self.model.status = LocalizedText::new(
                "当前 {0} 模型不支持所选 effort。",
                &[("0", (self.model.selected_harness).to_string())],
            );
            return;
        }
        self.model.effort = normalize_effort_for_harness(self.model.selected_harness, effort);
        self.persist_catalog_effort();
    }

    pub(crate) fn refresh_model_catalog(&mut self) -> bool {
        if self.model.active_run.is_some() {
            return false;
        }
        let project_id = self
            .model
            .selected_project
            .as_ref()
            .map(|project| project.id);
        let project_changed = self.model.catalog_project != project_id;
        if project_changed {
            self.model.title_model_catalog = ModelCatalogState::Idle;
            self.model.commit_model_catalog = ModelCatalogState::Idle;
        }
        self.model.catalog_project = project_id;
        let harness = self.model.selected_harness;
        let Some(cwd) = self.model.working_directory().map(str::to_owned) else {
            self.model.model_catalog = ModelCatalogState::Idle;
            return false;
        };
        if self
            .model
            .selected_probe()
            .is_some_and(|probe| !probe.available)
        {
            self.model.model_catalog = ModelCatalogState::NotReady(
                "可执行文件尚未就绪，请在设置中检查并重新探测。".into(),
            );
            return false;
        }
        let environment = match self.provider_launch_configuration(harness) {
            Ok(configuration) => configuration,
            Err(error) => {
                self.model.model_catalog = ModelCatalogState::NotReady(error.to_string().into());
                return false;
            }
        };
        let Some(runner) = &self.runner else {
            self.model.model_catalog.fail("Runner 不可用。".into());
            return false;
        };
        let request_id = Uuid::new_v4();
        let command = CommandEnvelope::new(Command::ModelCatalogRefresh {
            context_id: Some(self.model.conversation.id),
            request_id,
            purpose: ModelCatalogPurpose::Conversation,
            harness,
            executable: self.model.executable.clone(),
            cwd,
            environment,
        });
        let mut models = self
            .model
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
        self.model.model_catalog = ModelCatalogState::Loading { request_id, models };
        if runner.send(command).is_err() {
            self.model.model_catalog.fail("Runner 不可用。".into());
            return false;
        }
        true
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
            };
        } else {
            self.model.active_provider_profiles.remove(&harness);
            let _ = self
                .storage
                .set_setting(&active_profile_setting_key(harness), "");
            self.model.status = LocalizedText::new(
                "{harness} 将使用 CLI 当前登录配置。",
                &[("harness", (harness).to_string())],
            );
        }
        self.restore_catalog_preferences();
        self.refresh_model_catalog();
        true
    }

    pub(crate) fn save_provider_profile(&mut self, draft: ProviderProfileDraft) -> Option<Uuid> {
        if self.model.active_run_count() > 0 {
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
            self.model.status = LocalizedText::new(
                "Provider Profile 名称不能超过 {PROVIDER_PROFILE_NAME_MAX_CHARS} 个字符。",
                &[(
                    "PROVIDER_PROFILE_NAME_MAX_CHARS",
                    (PROVIDER_PROFILE_NAME_MAX_CHARS).to_string(),
                )],
            );
            return None;
        }
        if model
            .as_deref()
            .is_some_and(|model| model.chars().count() > PROVIDER_MODEL_MAX_CHARS)
        {
            self.model.status = LocalizedText::new(
                "默认模型不能超过 {PROVIDER_MODEL_MAX_CHARS} 个字符。",
                &[(
                    "PROVIDER_MODEL_MAX_CHARS",
                    (PROVIDER_MODEL_MAX_CHARS).to_string(),
                )],
            );
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
            self.model.status = LocalizedText::new(
                "{harness} 已存在同名 Provider Profile。",
                &[("harness", (harness).to_string())],
            );
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
                    self.model.status = LocalizedText::new(
                        "无法读取系统凭据库：{error}",
                        &[("error", (error).to_string())],
                    );
                    return None;
                }
            }
        } else {
            if let Err(error) = self.credentials.set_api_key(profile_id, api_key) {
                self.model.status = LocalizedText::new(
                    "无法安全保存 API Key：{error}",
                    &[("error", (error).to_string())],
                );
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
            self.model.status = LocalizedText::new(
                "无法保存 Provider Profile：{error}",
                &[("error", (error).to_string())],
            );
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
        self.model.status = LocalizedText::new(
            "Provider Profile 已保存并启用：{0}",
            &[("0", (profile.name).to_string())],
        );
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
            self.model.status = LocalizedText::new(
                "无法从系统凭据库删除 API Key：{error}",
                &[("error", (error).to_string())],
            );
            return false;
        }
        let mut profiles = self.model.provider_profiles.clone();
        profiles.retain(|profile| profile.id != profile_id);
        if let Err(error) = self.storage.set_provider_profiles(&profiles) {
            self.model.status = LocalizedText::new(
                "无法删除 Provider Profile：{error}",
                &[("error", (error).to_string())],
            );
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
        self.restore_catalog_preferences();
        self.refresh_model_catalog();
        true
    }

    fn provider_launch_configuration(
        &self,
        harness: HarnessKind,
    ) -> Result<Vec<EnvironmentVariable>> {
        let Some(profile) = self.model.provider_profile_for(harness) else {
            return Ok(Vec::new());
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
        Ok(environment)
    }

    fn restore_catalog_preferences(&mut self) {
        let harness = self.model.selected_harness;
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
        (self.model.model_override, self.model.effort) =
            load_catalog_preferences(&self.storage, harness, profile_id, legacy_effort);
        self.model.model_override_name = self
            .storage
            .setting(&catalog_model_name_setting_key(harness, profile_id))
            .ok()
            .flatten()
            .filter(|name| !name.is_empty());
        self.model.model_catalog = ModelCatalogState::Idle;
    }

    fn remember_model_name(&mut self) {
        self.model.model_override_name = self.model.model_override.as_ref().and_then(|_| {
            self.model
                .selected_catalog_model()
                .map(|model| model.display_name.clone())
        });
        let profile_id = self
            .model
            .selected_provider_profile()
            .map(|profile| profile.id);
        let _ = self.storage.set_setting(
            &catalog_model_name_setting_key(self.model.selected_harness, profile_id),
            self.model
                .model_override_name
                .as_deref()
                .unwrap_or_default(),
        );
    }

    fn persist_catalog_effort(&self) {
        let harness = self.model.selected_harness;
        let profile_id = self
            .model
            .selected_provider_profile()
            .map(|profile| profile.id);
        let _ = self.storage.set_setting(
            &catalog_effort_setting_key(harness, profile_id),
            self.model.effort.as_str(),
        );
    }

    fn normalize_catalog_effort(&mut self) -> bool {
        if self.model.effort.is_default()
            || self
                .model
                .selected_catalog_model()
                .is_some_and(|model| model.supports_effort(&self.model.effort))
        {
            return false;
        }
        self.model.effort = ThinkingEffort::Default;
        self.persist_catalog_effort();
        true
    }
}

fn permission_setting_key(harness: HarnessKind) -> String {
    format!("permission_mode.{}", harness.as_str())
}

fn load_permission_mode(storage: &Storage, harness: HarnessKind) -> PermissionMode {
    storage
        .setting(&permission_setting_key(harness))
        .ok()
        .flatten()
        .and_then(|value| value.parse().ok())
        .unwrap_or_default()
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

fn catalog_model_setting_key(harness: HarnessKind, profile_id: Option<Uuid>) -> String {
    format!(
        "{}_model_override_{}",
        harness.as_str(),
        profile_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "cli".into())
    )
}

fn catalog_effort_setting_key(harness: HarnessKind, profile_id: Option<Uuid>) -> String {
    format!(
        "{}_effort_{}",
        harness.as_str(),
        profile_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "cli".into())
    )
}

fn catalog_model_name_setting_key(harness: HarnessKind, profile_id: Option<Uuid>) -> String {
    format!("{}_name", catalog_model_setting_key(harness, profile_id))
}

fn load_catalog_preferences(
    storage: &Storage,
    harness: HarnessKind,
    profile_id: Option<Uuid>,
    legacy_effort: ThinkingEffort,
) -> (Option<String>, ThinkingEffort) {
    let model_override = storage
        .setting(&catalog_model_setting_key(harness, profile_id))
        .ok()
        .flatten()
        .or_else(|| {
            (harness == HarnessKind::Claude && profile_id.is_none()).then(|| {
                storage
                    .setting("claude_model")
                    .ok()
                    .flatten()
                    .and_then(|value| ClaudeModel::from_str(&value).ok())
                    .and_then(|model| model.cli_value().map(str::to_owned))
                    .unwrap_or_default()
            })
        })
        .filter(|model| !model.is_empty());
    let stored_effort = storage
        .setting(&catalog_effort_setting_key(harness, profile_id))
        .ok()
        .flatten()
        .and_then(|value| ThinkingEffort::from_str(&value).ok());
    let effort = normalize_effort_for_harness(
        harness,
        stored_effort.unwrap_or_else(|| {
            if profile_id.is_none() && matches!(harness, HarnessKind::Claude | HarnessKind::Omp) {
                legacy_effort
            } else {
                ThinkingEffort::Default
            }
        }),
    );
    if stored_effort != Some(effort) {
        let _ = storage.set_setting(
            &catalog_effort_setting_key(harness, profile_id),
            effort.as_str(),
        );
    }
    let _ = storage.set_setting(
        &catalog_model_setting_key(harness, profile_id),
        model_override.as_deref().unwrap_or_default(),
    );
    (model_override, effort)
}

fn normalize_effort_for_harness(harness: HarnessKind, effort: ThinkingEffort) -> ThinkingEffort {
    if harness != HarnessKind::Omp {
        return effort;
    }
    match effort {
        ThinkingEffort::None => ThinkingEffort::Off,
        ThinkingEffort::Max => ThinkingEffort::XHigh,
        ThinkingEffort::Ultra => ThinkingEffort::Default,
        effort => effort,
    }
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
