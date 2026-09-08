use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkspaceKind {
    #[default]
    Local,
    Worktree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkspaceStatus {
    Creating,
    Ready,
    Missing,
    Removed,
}

// This record outlives the chat. Only managed workspaces grant directory ownership.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Workspace {
    pub(crate) id: Uuid,
    pub(crate) project_id: Uuid,
    pub(crate) task_id: Option<Uuid>,
    pub(crate) path: String,
    pub(crate) repository: Option<String>,
    pub(crate) kind: WorkspaceKind,
    pub(crate) managed: bool,
    pub(crate) base_sha: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) merge_target: Option<String>,
    pub(crate) status: WorkspaceStatus,
}

impl Workspace {
    pub(crate) fn local(project: &nexus_domain::Project) -> Self {
        Self {
            id: project.id,
            project_id: project.id,
            task_id: None,
            path: project.canonical_path.clone(),
            repository: None,
            kind: WorkspaceKind::Local,
            managed: false,
            base_sha: None,
            branch: None,
            merge_target: None,
            status: WorkspaceStatus::Ready,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceDraft {
    pub(crate) task_id: Uuid,
    pub(crate) kind: WorkspaceKind,
    pub(crate) base: String,
    pub(crate) branch: String,
}

impl Default for WorkspaceDraft {
    fn default() -> Self {
        let task_id = Uuid::new_v4();
        Self {
            task_id,
            kind: WorkspaceKind::Local,
            base: "HEAD".into(),
            branch: format!("feat/nx-{}", &task_id.simple().to_string()[..8]),
        }
    }
}
