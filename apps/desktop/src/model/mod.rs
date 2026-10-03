mod conversation;
pub(crate) use conversation::{
    ConversationState, PendingUserAsk, QueuedMessage, UserAskSubmissionState,
};
pub(crate) mod harness_installation;
pub(crate) mod issues;
pub(crate) mod pull_requests;
pub(crate) mod tools;
pub(crate) mod updates;
pub(crate) mod voice;
pub(crate) mod workspace;

use crate::i18n::{Language, LocalizedText};
use nexus_domain::{
    HarnessKind, ModelDescriptor, Project, ProviderProfile, TaskSummary, ThinkingEffort,
};
use nexus_protocol::HarnessProbe;
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, Default)]
pub(crate) enum ModelCatalogState {
    #[default]
    Idle,
    Loading {
        request_id: Uuid,
        models: Vec<ModelDescriptor>,
    },
    Ready(Vec<ModelDescriptor>),
    Empty,
    NotReady(LocalizedText),
    Failed {
        message: LocalizedText,
        models: Vec<ModelDescriptor>,
    },
}

impl ModelCatalogState {
    pub(crate) fn models(&self) -> Option<&[ModelDescriptor]> {
        match self {
            Self::Ready(models) | Self::Loading { models, .. } | Self::Failed { models, .. } => {
                Some(models)
            }
            Self::Idle | Self::Empty | Self::NotReady(_) => None,
        }
    }

    pub(crate) fn can_select_model(&self, harness: HarnessKind, id: &str) -> bool {
        self.models()
            .and_then(|models| models.iter().find(|model| model.id == id))
            .map_or_else(
                // Claude exposes aliases, not a complete catalog of third-party models.
                || {
                    harness == HarnessKind::Claude
                        && !id.trim().is_empty()
                        && !id.chars().any(char::is_control)
                },
                |model| model.availability.is_selectable(),
            )
    }

    pub(crate) fn accepts(&self, request_id: Uuid) -> bool {
        matches!(self, Self::Loading { request_id: current, .. } if *current == request_id)
    }

    pub(crate) fn fail(&mut self, message: LocalizedText) {
        let models = self.models().unwrap_or_default().to_vec();
        *self = Self::Failed { message, models };
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct AppearanceSettings {
    pub(crate) theme: ThemePreference,
    pub(crate) glass: bool,
    pub(crate) reduced_motion: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct FontSettings {
    pub(crate) reading: Option<String>,
    pub(crate) code: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct SoundSettings {
    pub(crate) task_complete: bool,
}

impl Default for SoundSettings {
    fn default() -> Self {
        Self {
            task_complete: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct GenerationSettings {
    pub(crate) harness: HarnessKind,
    pub(crate) model: Option<String>,
    pub(crate) effort: ThinkingEffort,
}

impl Default for GenerationSettings {
    fn default() -> Self {
        Self {
            harness: HarnessKind::default(),
            model: None,
            effort: ThinkingEffort::Default,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum GenerationKind {
    Title,
    Commit,
}

impl GenerationKind {
    pub(crate) const ALL: [Self; 2] = [Self::Title, Self::Commit];

    pub(crate) fn setting_key(self) -> &'static str {
        match self {
            Self::Title => "title_generation",
            Self::Commit => "commit_message_generation",
        }
    }

    pub(crate) fn prefix(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Commit => "commit",
        }
    }
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            glass: true,
            reduced_motion: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedModelSelection {
    pub(crate) model: Option<String>,
    pub(crate) effort: ThinkingEffort,
}

pub(crate) struct RuntimeLogEntry {
    pub(crate) timestamp: chrono::DateTime<chrono::Local>,
    pub(crate) message: LocalizedText,
}

#[derive(Default)]
pub(crate) struct AppModel {
    // Logs belong to this launch, independent of conversation selection or deletion.
    pub(crate) runtime_log: Vec<RuntimeLogEntry>,
    pub(crate) cnb: issues::IssuesModel,
    pub(crate) github: issues::IssuesModel,
    pub(crate) language: Language,
    pub(crate) voice: voice::VoiceModel,
    pub(crate) appearance: AppearanceSettings,
    pub(crate) fonts: FontSettings,
    pub(crate) title_generation: GenerationSettings,
    pub(crate) commit_message_generation: GenerationSettings,
    pub(crate) updates: updates::UpdateModel,
    pub(crate) harness_manager: harness_installation::HarnessManager,
    pub(crate) cli_installation_busy: bool,
    pub(crate) cli_installation_message: Option<LocalizedText>,
    pub(crate) sound: SoundSettings,
    pub(crate) projects: Vec<Project>,
    pub(crate) projectless_tasks: Vec<TaskSummary>,
    pub(crate) archived_tasks: Vec<TaskSummary>,
    pub(crate) workspace_busy: bool,
    pub(crate) workspace_operation_context: Option<Uuid>,
    pub(crate) workspace_operation_paths: Vec<std::path::PathBuf>,
    pub(crate) harnesses: BTreeMap<HarnessKind, HarnessProbe>,
    pub(crate) provider_profiles: Vec<ProviderProfile>,
    pub(crate) conversation: ConversationState,
    pub(crate) conversations: BTreeMap<Uuid, ConversationState>,
}

impl AppModel {
    pub(crate) fn all_conversations(&self) -> impl Iterator<Item = &ConversationState> {
        std::iter::once(&self.conversation).chain(self.conversations.values())
    }

    pub(crate) fn active_run_count(&self) -> usize {
        self.all_conversations()
            .filter(|conversation| conversation.active_run.is_some())
            .count()
    }

    pub(crate) fn occupied_run_slots(&self) -> usize {
        self.all_conversations()
            .filter(|conversation| {
                conversation.active_run.is_some()
                    || (conversation.pending_workspace_start.is_some()
                        && !conversation.workspace_retry)
            })
            .count()
    }

    pub(crate) fn workspace_locked(&self, path: &std::path::Path) -> bool {
        self.workspace_operation_paths
            .iter()
            .any(|locked| locked == path)
    }

    pub(crate) fn task_running(&self, task_id: Uuid) -> bool {
        self.all_conversations().any(|conversation| {
            conversation.active_task == Some(task_id)
                || (conversation.pending_workspace_start.is_some()
                    && !conversation.workspace_retry
                    && conversation
                        .selected_workspace
                        .as_ref()
                        .and_then(|workspace| workspace.task_id)
                        == Some(task_id))
        })
    }

    pub(crate) fn checkout_running(&self, path: &std::path::Path) -> bool {
        self.all_conversations()
            .filter(|conversation| conversation.active_run.is_some())
            .any(|conversation| conversation.active_checkout.as_deref() == Some(path))
    }

    pub(crate) fn activate_conversation(&mut self, id: Uuid) {
        if self.conversation.id == id {
            return;
        }
        if let Some(next) = self.conversations.remove(&id) {
            let previous = std::mem::replace(&mut self.conversation, next);
            self.conversations.insert(previous.id, previous);
        }
    }

    pub(crate) fn fresh_conversation(&mut self) {
        let previous_id = self.fork_conversation();
        let previous = &self.conversations[&previous_id];
        if previous.selected_task.is_none()
            && previous.active_run.is_none()
            && previous.pending_workspace_start.is_none()
            && self.workspace_operation_context != Some(previous_id)
        {
            self.conversations.remove(&previous_id);
        }
    }

    // Modal task configuration keeps the source draft available until cancellation.
    pub(crate) fn fork_conversation(&mut self) -> Uuid {
        let next = ConversationState {
            id: Uuid::new_v4(),
            selected_project: self.conversation.selected_project.clone(),
            selected_harness: self.conversation.selected_harness,
            permission_mode: self.conversation.permission_mode,
            model_override: self.conversation.model_override.clone(),
            model_override_name: self.conversation.model_override_name.clone(),
            model_catalog: match &self.conversation.model_catalog {
                ModelCatalogState::Loading { models, .. } => {
                    ModelCatalogState::Ready(models.clone())
                }
                catalog => catalog.clone(),
            },
            effort: self.conversation.effort,
            executable: self.conversation.executable.clone(),
            active_provider_profiles: self.conversation.active_provider_profiles.clone(),
            tasks: self.conversation.tasks.clone(),
            ..ConversationState::default()
        };
        let previous = std::mem::replace(&mut self.conversation, next);
        let previous_id = previous.id;
        self.conversations.insert(previous_id, previous);
        previous_id
    }

    pub(crate) fn working_directory(&self) -> Option<&str> {
        self.working_directory_in(self.conversation.id)
    }

    pub(crate) fn working_directory_in(&self, context: Uuid) -> Option<&str> {
        let cwd = if let Some(workspace) = &self[context].selected_workspace {
            Some(workspace.path.as_str())
        } else if self[context].selected_task.is_some() {
            None
        } else {
            self[context]
                .selected_project
                .as_ref()
                .map(|project| project.canonical_path.as_str())
        };
        cwd.filter(|path| !path.is_empty())
    }

    pub(crate) fn latest_log_text(&self, language: Language) -> &str {
        self.runtime_log
            .last()
            .map(|entry| entry.message.render(language))
            .unwrap_or_default()
    }

    pub(crate) fn log_status(&mut self, message: LocalizedText) {
        self.runtime_log.push(RuntimeLogEntry {
            timestamp: chrono::Local::now(),
            message,
        });
    }

    pub(crate) fn set_run_status(&mut self, message: LocalizedText) {
        self.set_run_status_in(self.conversation.id, message)
    }

    pub(crate) fn set_run_status_in(&mut self, context: Uuid, message: LocalizedText) {
        self[context].run_status = message.clone();
        self.log_status(message);
    }

    pub(crate) fn selected_probe(&self) -> Option<&HarnessProbe> {
        self.selected_probe_in(self.conversation.id)
    }

    pub(crate) fn selected_probe_in(&self, context: Uuid) -> Option<&HarnessProbe> {
        self.harnesses.get(&self[context].selected_harness)
    }

    pub(crate) fn can_submit(&self) -> bool {
        self.can_submit_in(self.conversation.id)
    }

    pub(crate) fn can_submit_in(&self, context: Uuid) -> bool {
        let profile_ready = self
            .selected_provider_profile_in(context)
            .is_some_and(|profile| profile.credential_configured);
        self[context].active_run.is_none()
            && self.occupied_run_slots() < 2
            && self
                .working_directory_in(context)
                .is_some_and(|path| !self.workspace_locked(std::path::Path::new(path)))
            && self[context]
                .selected_workspace
                .as_ref()
                .is_none_or(|workspace| workspace.status == workspace::WorkspaceStatus::Ready)
            && !self.updates.state.is_installing()
            && self.harness_manager.operating.is_none()
            && self
                .selected_probe_in(context)
                .is_some_and(|probe| probe.available && (probe.authenticated || profile_ready))
            && self.catalog_selection_is_valid_in(context)
    }

    pub(crate) fn can_queue(&self) -> bool {
        self.conversation.active_run.is_some()
            && !self.conversation.run_cancelling
            && self.conversation.active_task.is_some()
            && self.conversation.active_task == self.conversation.selected_task
    }

    pub(crate) fn selected_provider_profile(&self) -> Option<&ProviderProfile> {
        self.selected_provider_profile_in(self.conversation.id)
    }

    pub(crate) fn selected_provider_profile_in(&self, context: Uuid) -> Option<&ProviderProfile> {
        self.provider_profile_in(context, self[context].selected_harness)
    }

    pub(crate) fn provider_profile_for(&self, harness: HarnessKind) -> Option<&ProviderProfile> {
        self.provider_profile_in(self.conversation.id, harness)
    }

    pub(crate) fn provider_profile_in(
        &self,
        context: Uuid,
        harness: HarnessKind,
    ) -> Option<&ProviderProfile> {
        let profile_id = self[context].active_provider_profiles.get(&harness)?;
        self.provider_profiles
            .iter()
            .find(|profile| profile.id == *profile_id && profile.harness == harness)
    }

    pub(crate) fn configured_catalog_model_in(&self, context: Uuid) -> Option<&str> {
        self[context].model_override.as_deref().or_else(|| {
            self.selected_provider_profile_in(context)
                .and_then(|profile| profile.model.as_deref())
        })
    }

    pub(crate) fn selected_catalog_model(&self) -> Option<&ModelDescriptor> {
        self.selected_catalog_model_in(self.conversation.id)
    }

    pub(crate) fn selected_catalog_model_in(&self, context: Uuid) -> Option<&ModelDescriptor> {
        let models = self[context].model_catalog.models()?;
        if let Some(id) = self.configured_catalog_model_in(context) {
            models.iter().find(|model| model.id == id)
        } else {
            models.iter().find(|model| model.is_default)
        }
    }

    pub(crate) fn generation_settings(&self, kind: GenerationKind) -> &GenerationSettings {
        match kind {
            GenerationKind::Title => &self.title_generation,
            GenerationKind::Commit => &self.commit_message_generation,
        }
    }

    pub(crate) fn generation_catalog(&self, kind: GenerationKind) -> &ModelCatalogState {
        self.generation_catalog_in(self.conversation.id, kind)
    }

    pub(crate) fn generation_catalog_in(
        &self,
        context: Uuid,
        kind: GenerationKind,
    ) -> &ModelCatalogState {
        match kind {
            GenerationKind::Title => &self[context].title_model_catalog,
            GenerationKind::Commit => &self[context].commit_model_catalog,
        }
    }

    pub(crate) fn generation_catalog_mut(
        &mut self,
        kind: GenerationKind,
    ) -> &mut ModelCatalogState {
        self.generation_catalog_mut_in(self.conversation.id, kind)
    }

    pub(crate) fn generation_catalog_mut_in(
        &mut self,
        context: Uuid,
        kind: GenerationKind,
    ) -> &mut ModelCatalogState {
        match kind {
            GenerationKind::Title => &mut self[context].title_model_catalog,
            GenerationKind::Commit => &mut self[context].commit_model_catalog,
        }
    }

    pub(crate) fn generation_catalog_model(
        &self,
        kind: GenerationKind,
        model_override: Option<&str>,
    ) -> Option<&ModelDescriptor> {
        self.generation_catalog_model_in(self.conversation.id, kind, model_override)
    }

    pub(crate) fn generation_catalog_model_in(
        &self,
        context: Uuid,
        kind: GenerationKind,
        model_override: Option<&str>,
    ) -> Option<&ModelDescriptor> {
        let models = self.generation_catalog_in(context, kind).models()?;
        let model_id = model_override.or_else(|| {
            self.provider_profile_in(context, self.generation_settings(kind).harness)
                .and_then(|profile| profile.model.as_deref())
        });
        models.iter().find(|model| {
            model.availability.is_selectable()
                && model_id.map_or(model.is_default, |id| model.id == id)
        })
    }

    pub(crate) fn model_override_is_unavailable_in(&self, context: Uuid) -> bool {
        self[context].model_override.as_deref().is_some_and(|id| {
            !self[context]
                .model_catalog
                .can_select_model(self[context].selected_harness, id)
        })
    }

    #[cfg(test)]
    pub(crate) fn catalog_selection_is_valid(&self) -> bool {
        self.catalog_selection_is_valid_in(self.conversation.id)
    }

    pub(crate) fn catalog_selection_is_valid_in(&self, context: Uuid) -> bool {
        if self.model_override_is_unavailable_in(context) {
            return false;
        }
        self.selected_catalog_model_in(context).is_none_or(|model| {
            model.availability.is_selectable()
                && (self[context].effort.is_default()
                    || model.supports_effort(&self[context].effort))
        })
    }

    // One resolution path for the composer, Remote API, StartRun and persisted requests.
    pub(crate) fn resolved_model_selection(&self) -> ResolvedModelSelection {
        self.resolved_model_selection_in(self.conversation.id)
    }

    pub(crate) fn resolved_model_selection_in(&self, context: Uuid) -> ResolvedModelSelection {
        let model = self.configured_catalog_model_in(context).map(str::to_owned);
        let descriptor = self.selected_catalog_model_in(context);
        let effort = if descriptor.is_some_and(|model| model.supports_effort(&self[context].effort))
        {
            self[context].effort
        } else if model.is_some() && self[context].effort.is_default() {
            descriptor
                .and_then(|model| {
                    model
                        .default_reasoning_effort
                        .filter(|effort| model.supports_effort(effort))
                })
                .unwrap_or(ThinkingEffort::Default)
        } else {
            ThinkingEffort::Default
        };
        ResolvedModelSelection { model, effort }
    }
}

impl AppModel {
    pub(crate) fn issues(&self, provider: issues::IssueProvider) -> &issues::IssuesModel {
        match provider {
            issues::IssueProvider::Cnb => &self.cnb,
            issues::IssueProvider::GitHub => &self.github,
        }
    }

    pub(crate) fn issues_mut(
        &mut self,
        provider: issues::IssueProvider,
    ) -> &mut issues::IssuesModel {
        match provider {
            issues::IssueProvider::Cnb => &mut self.cnb,
            issues::IssueProvider::GitHub => &mut self.github,
        }
    }

    pub(crate) fn opened_issues(&self) -> Option<issues::IssueProvider> {
        issues::IssueProvider::ALL
            .into_iter()
            .find(|provider| self.issues(*provider).opened)
    }
}

impl std::ops::Index<Uuid> for AppModel {
    type Output = ConversationState;
    fn index(&self, id: Uuid) -> &Self::Output {
        if self.conversation.id == id {
            &self.conversation
        } else {
            &self.conversations[&id]
        }
    }
}

impl std::ops::IndexMut<Uuid> for AppModel {
    fn index_mut(&mut self, id: Uuid) -> &mut Self::Output {
        if self.conversation.id == id {
            &mut self.conversation
        } else {
            self.conversations
                .get_mut(&id)
                .expect("known conversation context")
        }
    }
}
