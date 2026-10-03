//! HTTP payloads shared by the desktop server and remote client contract checks.
use nexus_domain::{HarnessKind, Project, TaskSummary, ThinkingEffort};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteProject {
    pub id: Uuid,
    pub display_name: String,
}

impl From<&Project> for RemoteProject {
    fn from(project: &Project) -> Self {
        Self {
            id: project.id,
            display_name: project.display_name.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteState {
    pub projects: Vec<RemoteProject>,
    pub tasks: Vec<TaskSummary>,
    pub selected_project_id: Option<Uuid>,
    pub selected_task_id: Option<Uuid>,
    pub active_run_id: Option<Uuid>,
    pub active_task_id: Option<Uuid>,
    pub streaming_text: String,
    pub status: String,
    pub harness: HarnessKind,
    pub model: Option<String>,
    pub effort: ThinkingEffort,
    pub harness_ready: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartRunRequest {
    pub project_id: Uuid,
    pub prompt: String,
}
