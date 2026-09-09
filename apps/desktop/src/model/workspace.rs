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
    #[serde(default)]
    pub(crate) external: bool,
    pub(crate) base_sha: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) merge_target: Option<String>,
    pub(crate) status: WorkspaceStatus,
    #[serde(default)]
    pub(crate) merge: Option<MergeState>,
    // Preserve historical initialization logs when saving existing workspace records.
    #[serde(default)]
    pub(crate) initialization: Option<InitializationLog>,
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
            external: false,
            base_sha: None,
            branch: None,
            merge_target: None,
            status: WorkspaceStatus::Ready,
            merge: None,
            initialization: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MergeState {
    pub(crate) source_sha: String,
    pub(crate) target_sha: String,
    pub(crate) target_branch: String,
    pub(crate) target_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct InitializationLog {
    pub(crate) command: String,
    pub(crate) output: String,
    pub(crate) running: bool,
    pub(crate) success: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceReview {
    pub(crate) workspace_id: Uuid,
    pub(crate) head: String,
    pub(crate) branch: Option<String>,
    pub(crate) committed: String,
    pub(crate) staged: String,
    pub(crate) unstaged: String,
    pub(crate) untracked: Vec<(String, String)>,
    pub(crate) dirty_paths: Vec<String>,
    pub(crate) additions: usize,
    pub(crate) deletions: usize,
    pub(crate) target_branches: Vec<String>,
    pub(crate) conflicts: Vec<String>,
    pub(crate) resolution_diff: String,
    pub(crate) resolution_staged: String,
}

#[derive(Debug, Clone)]
pub(crate) struct MergePlan {
    pub(crate) workspace_id: Uuid,
    pub(crate) source_branch: String,
    pub(crate) state: MergeState,
    pub(crate) diff: String,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceDraft {
    pub(crate) task_id: Uuid,
    pub(crate) kind: WorkspaceKind,
    pub(crate) base: String,
}

impl Default for WorkspaceDraft {
    fn default() -> Self {
        Self {
            task_id: Uuid::new_v4(),
            kind: WorkspaceKind::Local,
            base: String::new(),
        }
    }
}

pub(crate) struct PendingWorkspaceStart {
    pub(crate) context_id: Uuid,
    pub(crate) prompt: String,
    pub(crate) executable: String,
    pub(crate) permission: nexus_domain::PermissionMode,
}
