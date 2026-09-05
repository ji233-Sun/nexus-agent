pub(crate) mod history;

use history::{HistoryMessage, ThreadSummary};
use nexus_domain::{ClaudeModel, HarnessKind, Message, Project, TaskSummary, ThinkingEffort};
use nexus_protocol::HarnessProbe;
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Default)]
pub(crate) struct AppModel {
    pub(crate) projects: Vec<Project>,
    pub(crate) selected_project: Option<Project>,
    pub(crate) tasks: Vec<TaskSummary>,
    pub(crate) selected_task: Option<Uuid>,
    pub(crate) messages: Vec<Message>,
    pub(crate) active_run: Option<Uuid>,
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
    pub(crate) effort: ThinkingEffort,
    pub(crate) executable: String,
}

impl AppModel {
    pub(crate) fn selected_probe(&self) -> Option<&HarnessProbe> {
        self.harnesses.get(&self.selected_harness)
    }

    pub(crate) fn can_submit(&self) -> bool {
        self.selected_project.is_some()
            && self.active_run.is_none()
            && self
                .selected_probe()
                .is_some_and(|probe| probe.available && probe.authenticated)
    }
}
