mod catalogs;
mod configuration;
mod events;
mod generation;
mod harness_installation;
mod issues;
mod provider_profiles;
mod remote;
mod runs;
mod tasks;
mod updates;
mod voice;
mod workspace;

#[cfg(test)]
pub(crate) mod tests;

use crate::{
    i18n::{Language, LocalizedText},
    infrastructure::{
        credentials::{CredentialStore, SystemCredentialStore},
        storage::Storage,
    },
    model::{
        AppModel, AppearanceSettings, ConversationState, FontSettings, GenerationKind,
        GenerationSettings, ModelCatalogState, SoundSettings,
        issues::IssueProvider,
        updates::{UpdateChannel, UpdateModel, UpdateState},
    },
    remote_control::{RemoteCommand, RemoteControl},
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
    issues_client: crate::infrastructure::issues::Client,
    model: AppModel,
    voice_worker: Option<crate::infrastructure::voice::Worker>,
    storage: Storage,
    runner: Option<Box<dyn RunnerPort>>,
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
        let fonts = storage
            .setting("fonts")
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default();
        let sound = storage
            .setting("sound")
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
        let has_storage_error = storage_error.is_some();
        let (runner, runner_error) = match runner {
            Ok(runner) => (Some(runner), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let mut presenter = Self {
            issues_client: crate::infrastructure::issues::Client::default(),
            voice_worker: None,
            storage,
            runner,
            model: AppModel {
                language,
                appearance,
                fonts,
                sound,
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
                    ..ConversationState::default()
                },
                ..AppModel::default()
            },
            remote_control: None,
            remote_control_error: None,
            credentials,
            update_events: None,
            installation_worker: None,
            cli_installation_result: None,
            workspace_events: None,
            worktree_root: crate::infrastructure::paths::worktree_directory(),
        };
        presenter.model.log_status(
            storage_error
                .map(LocalizedText::from)
                .or_else(|| {
                    credential_store_error.map(|error| {
                        LocalizedText::new(
                            "无法读取系统凭据库：{error}",
                            &[("error", error.to_string())],
                        )
                    })
                })
                .unwrap_or_else(|| "正在连接本地 Runner…".into()),
        );
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
                    environment: presenter.codebuddy_environment(harness),
                    harness,
                    executable,
                }));
            }
        } else if !has_storage_error {
            presenter
                .model
                .log_status(runner_error.unwrap_or_default().into());
        }
        presenter.load_voice_settings();
        for provider in IssueProvider::ALL {
            presenter.model.issues_mut(provider).enabled = presenter
                .storage
                .setting(&format!("{}_enabled", provider.key()))
                .ok()
                .flatten()
                .is_none_or(|value| value != "false");
        }
        presenter.reload_tasks();
        presenter.refresh_model_catalog();
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
                self.model.log_status(LocalizedText::new(
                    "无法保存语言偏好：{error}",
                    &[("error", error.to_string())],
                ));
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
                self.model.log_status(LocalizedText::new(
                    "无法保存外观偏好：{error}",
                    &[("error", (error).to_string())],
                ));
                false
            }
        }
    }

    pub(crate) fn set_fonts(&mut self, fonts: FontSettings) -> Result<()> {
        self.storage
            .set_setting("fonts", &serde_json::to_string(&fonts)?)?;
        self.model.fonts = fonts;
        Ok(())
    }

    pub(crate) fn set_sound(&mut self, sound: SoundSettings) -> bool {
        let result = serde_json::to_string(&sound)
            .map_err(anyhow::Error::from)
            .and_then(|value| self.storage.set_setting("sound", &value));
        match result {
            Ok(()) => {
                self.model.sound = sound;
                true
            }
            Err(error) => {
                self.model.log_status(LocalizedText::new(
                    "无法保存提示音设置：{error}",
                    &[("error", (error).to_string())],
                ));
                false
            }
        }
    }

    pub(crate) fn report_font_load_error(&mut self, error: String) {
        self.model.log_status(LocalizedText::new(
            "无法加载导入字体：{error}",
            &[("error", error)],
        ));
    }

    pub(crate) fn drain_events(&mut self) -> bool {
        let runner_events = self
            .runner
            .as_ref()
            .map(|runner| runner.drain_events())
            .unwrap_or_default();
        let remote_commands = self
            .remote_control
            .as_ref()
            .map(RemoteControl::drain_commands)
            .unwrap_or_default();
        let mut changed = self.drain_workspace_events() || !runner_events.is_empty();
        changed |= self.drain_cli_installation_result();
        for envelope in runner_events {
            if envelope.protocol_version != nexus_protocol::PROTOCOL_VERSION {
                self.model
                    .log_status("Desktop 与 Runner 协议版本不匹配，请重启或更新应用。".into());
                continue;
            }
            self.handle_event(envelope.event);
        }
        for command in remote_commands {
            changed |= self.handle_remote_command(command);
        }
        if changed {
            self.notify_remote_changed();
        }
        changed
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
        HarnessKind::Pi => "pi_executable",
        HarnessKind::Kimi => "kimi_executable",
        HarnessKind::Qoder => "qoder_executable",
        HarnessKind::QoderCn => "qodercn_executable",
        HarnessKind::Codebuddy => "codebuddy_executable",
        HarnessKind::Opencode => "opencode_executable",
        HarnessKind::Deepseek => "deepseek_executable",
        HarnessKind::CommandCode => "commandcode_executable",
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
