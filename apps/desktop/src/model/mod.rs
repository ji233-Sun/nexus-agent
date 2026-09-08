pub(crate) mod history;
pub(crate) mod tools;
pub(crate) mod updates;

use crate::i18n::{Language, LocalizedText};
use history::{HistoryMessage, ThreadSummary};
use nexus_domain::{
    HarnessKind, Message, ModelDescriptor, PermissionMode, Project, ProviderProfile, TaskSummary,
    ThinkingEffort, UserAskAnswer, UserAskAnswerMode, UserAskAnswerValue, UserAskQuestion,
};
use nexus_protocol::{ApprovalRequest, HarnessProbe};
use std::collections::{BTreeMap, VecDeque};
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

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TitleGenerationSettings {
    pub(crate) harness: HarnessKind,
    pub(crate) model: Option<String>,
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

#[derive(Debug, Clone)]
pub(crate) struct QueuedMessage {
    pub(crate) id: Uuid,
    pub(crate) task_id: Uuid,
    pub(crate) prompt: String,
    pub(crate) permission_mode: PermissionMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum UserAskSubmissionState {
    Pending,
    Submitting,
    Sent,
}

#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct PendingUserAsk {
    pub(crate) request_id: Uuid,
    pub(crate) questions: Vec<UserAskQuestion>,
    pub(crate) submission: UserAskSubmissionState,
    pub(crate) error: Option<String>,
    pub(crate) active_question: usize,
    pub(crate) collapsed: bool,
    pub(crate) drafts: BTreeMap<String, UserAskAnswerValue>,
    pub(crate) submitted_answers: Option<Vec<UserAskAnswer>>,
}

impl PendingUserAsk {
    pub(crate) fn new(request_id: Uuid, questions: Vec<UserAskQuestion>) -> Self {
        let drafts = questions
            .iter()
            .map(|question| {
                let value = match question.answer_mode {
                    UserAskAnswerMode::Text => UserAskAnswerValue::Text(String::new()),
                    UserAskAnswerMode::Choice { .. } => UserAskAnswerValue::Selected(Vec::new()),
                };
                (question.id.clone(), value)
            })
            .collect();
        Self {
            request_id,
            questions,
            submission: UserAskSubmissionState::Pending,
            error: None,
            active_question: 0,
            collapsed: false,
            drafts,
            submitted_answers: None,
        }
    }

    pub(crate) fn answers(&self) -> Option<Vec<UserAskAnswer>> {
        if self.questions.is_empty() {
            return None;
        }
        self.questions
            .iter()
            .map(|question| {
                let value = self.drafts.get(&question.id)?.clone();
                let valid = match (&question.answer_mode, &value) {
                    (UserAskAnswerMode::Text, UserAskAnswerValue::Text(text)) => {
                        !text.trim().is_empty()
                    }
                    (
                        UserAskAnswerMode::Choice { multiple, .. },
                        UserAskAnswerValue::Selected(option_ids),
                    ) => {
                        !option_ids.is_empty()
                            && (*multiple || option_ids.len() == 1)
                            && option_ids.iter().all(|option_id| {
                                question
                                    .options
                                    .iter()
                                    .any(|option| option.id == *option_id)
                            })
                    }
                    (
                        UserAskAnswerMode::Choice {
                            allow_custom: true, ..
                        },
                        UserAskAnswerValue::Text(text),
                    ) => !text.trim().is_empty(),
                    _ => false,
                };
                valid.then_some(UserAskAnswer {
                    question_id: question.id.clone(),
                    value,
                })
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedModelSelection {
    pub(crate) model: Option<String>,
    pub(crate) effort: ThinkingEffort,
}

#[derive(Default)]
pub(crate) struct AppModel {
    pub(crate) language: Language,
    pub(crate) appearance: AppearanceSettings,
    pub(crate) title_generation: TitleGenerationSettings,
    pub(crate) title_model_catalog: ModelCatalogState,
    pub(crate) updates: updates::UpdateModel,
    pub(crate) projects: Vec<Project>,
    pub(crate) selected_project: Option<Project>,
    pub(crate) tasks: Vec<TaskSummary>,
    pub(crate) archived_tasks: Vec<TaskSummary>,
    pub(crate) selected_task: Option<Uuid>,
    pub(crate) messages: Vec<Message>,
    pub(crate) active_run: Option<Uuid>,
    pub(crate) run_cancelling: bool,
    pub(crate) queued_messages: VecDeque<QueuedMessage>,
    pub(crate) steering_message: Option<Uuid>,
    pub(crate) pending_user_asks: Vec<PendingUserAsk>,
    pub(crate) active_run_elapsed_seconds: Option<u64>,
    pub(crate) active_task: Option<Uuid>,
    pub(crate) active_harness: Option<HarnessKind>,
    pub(crate) active_permission_mode: Option<PermissionMode>,
    pub(crate) pending_approvals: VecDeque<ApprovalRequest>,
    pub(crate) responding_approval: Option<Uuid>,
    pub(crate) streaming_text: String,
    pub(crate) status: LocalizedText,
    pub(crate) harnesses: BTreeMap<HarnessKind, HarnessProbe>,
    pub(crate) codex_threads: Vec<ThreadSummary>,
    pub(crate) selected_codex_thread: Option<String>,
    pub(crate) codex_history_messages: Vec<HistoryMessage>,
    pub(crate) codex_history_loading: bool,
    pub(crate) codex_thread_loading: bool,
    pub(crate) codex_history_error: Option<LocalizedText>,
    pub(crate) selected_harness: HarnessKind,
    pub(crate) project_dirty: bool,
    pub(crate) model_override: Option<String>,
    pub(crate) model_override_name: Option<String>,
    pub(crate) model_catalog: ModelCatalogState,
    pub(crate) effort: ThinkingEffort,
    pub(crate) permission_mode: PermissionMode,
    pub(crate) executable: String,
    pub(crate) provider_profiles: Vec<ProviderProfile>,
    pub(crate) active_provider_profiles: BTreeMap<HarnessKind, Uuid>,
}

impl AppModel {
    pub(crate) fn working_directory(&self) -> Option<&str> {
        let cwd = if let Some(thread_id) = &self.selected_codex_thread {
            self.codex_threads
                .iter()
                .find(|thread| &thread.id == thread_id)
                .map(|thread| thread.cwd.as_str())
        } else {
            self.selected_project
                .as_ref()
                .map(|project| project.canonical_path.as_str())
        };
        cwd.filter(|path| !path.is_empty())
    }

    pub(crate) fn status_text(&self) -> &str {
        self.status.render(self.language)
    }

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
            && self.catalog_selection_is_valid()
    }

    pub(crate) fn can_queue(&self) -> bool {
        self.active_run.is_some()
            && !self.run_cancelling
            && self.active_task.is_some()
            && self.active_task == self.selected_task
            && self.selected_codex_thread.is_none()
    }

    pub(crate) fn selected_provider_profile(&self) -> Option<&ProviderProfile> {
        self.provider_profile_for(self.selected_harness)
    }

    pub(crate) fn provider_profile_for(&self, harness: HarnessKind) -> Option<&ProviderProfile> {
        let profile_id = self.active_provider_profiles.get(&harness)?;
        self.provider_profiles
            .iter()
            .find(|profile| profile.id == *profile_id && profile.harness == harness)
    }

    pub(crate) fn configured_catalog_model(&self) -> Option<&str> {
        self.model_override.as_deref().or_else(|| {
            self.selected_provider_profile()
                .and_then(|profile| profile.model.as_deref())
        })
    }

    pub(crate) fn selected_catalog_model(&self) -> Option<&ModelDescriptor> {
        let models = self.model_catalog.models()?;
        if let Some(id) = self.configured_catalog_model() {
            models.iter().find(|model| model.id == id)
        } else {
            models.iter().find(|model| model.is_default)
        }
    }

    pub(crate) fn model_override_is_unavailable(&self) -> bool {
        self.model_override.is_some()
            && self
                .selected_catalog_model()
                .is_none_or(|model| !model.availability.is_selectable())
    }

    pub(crate) fn catalog_selection_is_valid(&self) -> bool {
        if self.model_override_is_unavailable() {
            return false;
        }
        self.selected_catalog_model().is_none_or(|model| {
            model.availability.is_selectable()
                && (self.effort.is_default() || model.supports_effort(&self.effort))
        })
    }

    // One resolution path for the composer, Remote API, StartRun and persisted requests.
    pub(crate) fn resolved_model_selection(&self) -> ResolvedModelSelection {
        let model = self.configured_catalog_model().map(str::to_owned);
        let descriptor = self.selected_catalog_model();
        let effort = if descriptor.is_some_and(|model| model.supports_effort(&self.effort)) {
            self.effort
        } else if model.is_some() && self.effort.is_default() {
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
