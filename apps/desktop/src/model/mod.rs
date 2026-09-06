pub(crate) mod history;
pub(crate) mod tools;

use history::{HistoryMessage, ThreadSummary};
use nexus_domain::{
    ClaudeModel, HarnessKind, Message, ModelDescriptor, Project, ProviderProfile, TaskSummary,
    ThinkingEffort,
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
    },
    Ready(Vec<ModelDescriptor>),
    Empty,
    Failed(String),
}

impl ModelCatalogState {
    pub(crate) fn models(&self) -> Option<&[ModelDescriptor]> {
        match self {
            Self::Ready(models) => Some(models),
            Self::Idle | Self::Loading { .. } | Self::Empty | Self::Failed(_) => None,
        }
    }

    pub(crate) fn accepts(&self, request_id: Uuid) -> bool {
        matches!(self, Self::Loading { request_id: current } if *current == request_id)
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

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            glass: true,
            reduced_motion: false,
        }
    }
}

#[derive(Default)]
pub(crate) struct AppModel {
    pub(crate) appearance: AppearanceSettings,
    pub(crate) projects: Vec<Project>,
    pub(crate) selected_project: Option<Project>,
    pub(crate) tasks: Vec<TaskSummary>,
    pub(crate) selected_task: Option<Uuid>,
    pub(crate) messages: Vec<Message>,
    pub(crate) active_run: Option<Uuid>,
    pub(crate) active_run_elapsed_seconds: Option<u64>,
    pub(crate) active_task: Option<Uuid>,
    pub(crate) active_harness: Option<HarnessKind>,
    pub(crate) streaming_text: String,
    pub(crate) status: String,
    pub(crate) harnesses: BTreeMap<HarnessKind, HarnessProbe>,
    pub(crate) codex_threads: Vec<ThreadSummary>,
    pub(crate) selected_codex_thread: Option<String>,
    pub(crate) codex_history_messages: Vec<HistoryMessage>,
    pub(crate) codex_history_loading: bool,
    pub(crate) codex_thread_loading: bool,
    pub(crate) codex_history_error: Option<String>,
    pub(crate) selected_harness: HarnessKind,
    pub(crate) project_dirty: bool,
    pub(crate) claude_model: ClaudeModel,
    pub(crate) codex_model_override: Option<String>,
    pub(crate) codex_model_catalog: ModelCatalogState,
    pub(crate) effort: ThinkingEffort,
    pub(crate) executable: String,
    pub(crate) provider_profiles: Vec<ProviderProfile>,
    pub(crate) active_provider_profiles: BTreeMap<HarnessKind, Uuid>,
}

impl AppModel {
    pub(crate) fn selected_probe(&self) -> Option<&HarnessProbe> {
        self.harnesses.get(&self.selected_harness)
    }

    pub(crate) fn can_submit(&self) -> bool {
        let profile_ready = self
            .selected_provider_profile()
            .is_some_and(|profile| profile.credential_configured);
        self.selected_project.is_some()
            && self.active_run.is_none()
            && self
                .selected_probe()
                .is_some_and(|probe| probe.available && (probe.authenticated || profile_ready))
            && self.codex_selection_is_valid()
    }

    pub(crate) fn selected_provider_profile(&self) -> Option<&ProviderProfile> {
        let profile_id = self.active_provider_profiles.get(&self.selected_harness)?;
        self.provider_profiles
            .iter()
            .find(|profile| profile.id == *profile_id && profile.harness == self.selected_harness)
    }

    pub(crate) fn configured_codex_model(&self) -> Option<&str> {
        self.codex_model_override.as_deref().or_else(|| {
            self.selected_provider_profile()
                .and_then(|profile| profile.model.as_deref())
        })
    }

    pub(crate) fn selected_codex_catalog_model(&self) -> Option<&ModelDescriptor> {
        let models = self.codex_model_catalog.models()?;
        if let Some(id) = self.configured_codex_model() {
            models.iter().find(|model| model.id == id)
        } else {
            models.iter().find(|model| model.is_default)
        }
    }

    pub(crate) fn codex_model_override_is_unavailable(&self) -> bool {
        self.codex_model_override.is_some()
            && matches!(
                self.codex_model_catalog,
                ModelCatalogState::Ready(_) | ModelCatalogState::Empty
            )
            && self.selected_codex_catalog_model().is_none()
    }

    pub(crate) fn codex_selection_is_valid(&self) -> bool {
        if self.selected_harness != HarnessKind::Codex {
            return true;
        }
        if self.codex_model_override.is_some() && self.selected_codex_catalog_model().is_none() {
            return false;
        }
        self.effort.is_default()
            || self
                .selected_codex_catalog_model()
                .is_some_and(|model| model.supports_effort(&self.effort))
    }
}
