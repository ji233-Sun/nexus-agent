pub(crate) mod harness_installation;
pub(crate) mod history;
pub(crate) mod tools;
pub(crate) mod updates;
pub(crate) mod workspace;

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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct TitleGenerationSettings {
    pub(crate) harness: HarnessKind,
    pub(crate) model: Option<String>,
    pub(crate) effort: ThinkingEffort,
}

impl Default for TitleGenerationSettings {
    fn default() -> Self {
        Self {
            harness: HarnessKind::default(),
            model: None,
            effort: ThinkingEffort::Default,
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
    pub(crate) updates: updates::UpdateModel,
    pub(crate) harness_manager: harness_installation::HarnessManager,
    pub(crate) projects: Vec<Project>,
    pub(crate) archived_tasks: Vec<TaskSummary>,
    pub(crate) workspace_busy: bool,
    pub(crate) workspace_operation_context: Option<Uuid>,
    pub(crate) workspace_operation_paths: Vec<std::path::PathBuf>,
    pub(crate) harnesses: BTreeMap<HarnessKind, HarnessProbe>,
    pub(crate) codex_threads: Vec<ThreadSummary>,
    pub(crate) codex_history_loading: bool,
    pub(crate) codex_history_error: Option<LocalizedText>,
    pub(crate) provider_profiles: Vec<ProviderProfile>,
    pub(crate) conversation: ConversationState,
    pub(crate) conversations: BTreeMap<Uuid, ConversationState>,
}

// Each task owns its configuration, live output, queue and approval state. The
// selected conversation lives here; switching moves it into the keyed collection.
#[derive(Default)]
pub(crate) struct ConversationState {
    pub(crate) workspace_retry: bool,
    pub(crate) pending_workspace_start: Option<workspace::PendingWorkspaceStart>,
    pub(crate) workspace_review: Option<workspace::WorkspaceReview>,
    pub(crate) merge_plan: Option<workspace::MergePlan>,
    pub(crate) selected_changes: std::collections::BTreeSet<String>,
    pub(crate) id: Uuid,
    pub(crate) active_run_started_at: Option<std::time::Instant>,
    pub(crate) active_checkout: Option<std::path::PathBuf>,
    pub(crate) catalog_project: Option<Uuid>,
    pub(crate) title_model_catalog: ModelCatalogState,
    pub(crate) selected_project: Option<Project>,
    pub(crate) tasks: Vec<TaskSummary>,
    pub(crate) selected_task: Option<Uuid>,
    pub(crate) selected_workspace: Option<workspace::Workspace>,
    pub(crate) workspace_draft: workspace::WorkspaceDraft,
    pub(crate) workspaces: Vec<workspace::Workspace>,
    pub(crate) project_is_git: bool,
    pub(crate) workspace_branch: Option<String>,
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
    pub(crate) selected_codex_thread: Option<String>,
    pub(crate) codex_history_messages: Vec<HistoryMessage>,
    pub(crate) codex_thread_loading: bool,
    pub(crate) selected_harness: HarnessKind,
    pub(crate) project_dirty: bool,
    pub(crate) model_override: Option<String>,
    pub(crate) model_override_name: Option<String>,
    pub(crate) model_catalog: ModelCatalogState,
    pub(crate) effort: ThinkingEffort,
    pub(crate) permission_mode: PermissionMode,
    pub(crate) executable: String,
    pub(crate) active_provider_profiles: BTreeMap<HarnessKind, Uuid>,
}

impl std::ops::Deref for AppModel {
    type Target = ConversationState;
    fn deref(&self) -> &Self::Target {
        &self.conversation
    }
}

impl std::ops::DerefMut for AppModel {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.conversation
    }
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
        let next = ConversationState {
            id: Uuid::new_v4(),
            selected_project: self.selected_project.clone(),
            selected_harness: self.selected_harness,
            permission_mode: self.permission_mode,
            model_override: self.model_override.clone(),
            model_override_name: self.model_override_name.clone(),
            model_catalog: match &self.model_catalog {
                ModelCatalogState::Loading { models, .. } => {
                    ModelCatalogState::Ready(models.clone())
                }
                catalog => catalog.clone(),
            },
            effort: self.effort,
            executable: self.executable.clone(),
            active_provider_profiles: self.active_provider_profiles.clone(),
            tasks: self.tasks.clone(),
            ..ConversationState::default()
        };
        let previous = std::mem::replace(&mut self.conversation, next);
        if previous.selected_task.is_some()
            || previous.active_run.is_some()
            || previous.pending_workspace_start.is_some()
            || self.workspace_operation_context == Some(previous.id)
        {
            self.conversations.insert(previous.id, previous);
        }
    }

    pub(crate) fn working_directory(&self) -> Option<&str> {
        let cwd = if let Some(thread_id) = &self.selected_codex_thread {
            self.codex_threads
                .iter()
                .find(|thread| &thread.id == thread_id)
                .map(|thread| thread.cwd.as_str())
        } else if let Some(workspace) = &self.selected_workspace {
            Some(workspace.path.as_str())
        } else if self.selected_task.is_some() {
            None
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
            && self.occupied_run_slots() < 2
            && self
                .working_directory()
                .is_none_or(|path| !self.workspace_locked(std::path::Path::new(path)))
            && self
                .selected_workspace
                .as_ref()
                .is_none_or(|workspace| workspace.status == workspace::WorkspaceStatus::Ready)
            && !self.updates.state.is_installing()
            && self.harness_manager.operating.is_none()
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

    pub(crate) fn title_catalog_model(
        &self,
        model_override: Option<&str>,
    ) -> Option<&ModelDescriptor> {
        let models = self.title_model_catalog.models()?;
        let model_id = model_override.or_else(|| {
            self.provider_profile_for(self.title_generation.harness)
                .and_then(|profile| profile.model.as_deref())
        });
        models.iter().find(|model| {
            model.availability.is_selectable()
                && model_id.map_or(model.is_default, |id| model.id == id)
        })
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
