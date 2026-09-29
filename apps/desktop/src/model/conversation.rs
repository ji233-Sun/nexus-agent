use super::ModelCatalogState;
use super::workspace;
use crate::i18n::LocalizedText;
use nexus_domain::{
    HarnessKind, Message, PermissionMode, Project, TaskSummary, ThinkingEffort, UserAskAnswer,
    UserAskAnswerMode, UserAskAnswerValue, UserAskQuestion,
};
use nexus_protocol::ApprovalRequest;
use std::collections::{BTreeMap, HashSet, VecDeque};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub(crate) struct QueuedMessage {
    pub(crate) id: Uuid,
    pub(crate) task_id: Uuid,
    pub(crate) prompt: String,
    pub(crate) attachments: Vec<nexus_domain::Attachment>,
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

// Each task owns its configuration, live output, queue and approval state. The
// selected conversation lives here; switching moves it into the keyed collection.
#[derive(Default)]
pub(crate) struct ConversationState {
    pub(crate) attachments: Vec<nexus_domain::Attachment>,
    pub(crate) attachment_error: Option<LocalizedText>,
    pub(crate) attachments_loading: bool,
    pub(crate) changes_sidebar_open: bool,
    pub(crate) changes_files_expanded: bool,
    pub(crate) commit_editor_open: bool,
    pub(crate) changes_status: Option<LocalizedText>,
    pub(crate) commit_message: String,
    pub(crate) commit_message_request: Option<Uuid>,
    pub(crate) commit_model_catalog: ModelCatalogState,
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
    pub(crate) completed_runs: HashSet<Uuid>,
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
    pub(crate) run_status: LocalizedText,
    pub(crate) selected_harness: HarnessKind,
    pub(crate) model_override: Option<String>,
    pub(crate) model_override_name: Option<String>,
    pub(crate) model_catalog: ModelCatalogState,
    pub(crate) effort: ThinkingEffort,
    pub(crate) permission_mode: PermissionMode,
    pub(crate) executable: String,
    pub(crate) active_provider_profiles: BTreeMap<HarnessKind, Uuid>,
}
