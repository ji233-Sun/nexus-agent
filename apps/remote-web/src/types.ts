export type RunStatus =
  | "starting"
  | "running"
  | "cancelling"
  | "completed"
  | "failed"
  | "cancelled"
  | "interrupted";

export type MessageRole = "user" | "assistant" | "tool" | "system";

export type MessageKind =
  | "text"
  | "tool_call"
  | "tool_result"
  | "status"
  | "error";

export interface RemoteProject {
  id: string;
  display_name: string;
}

export interface TaskSummary {
  id: string;
  project_id: string;
  title: string;
  status: RunStatus;
  created_at: string;
}

export interface Message {
  id: string;
  task_id: string;
  run_id: string;
  sequence: number;
  role: MessageRole;
  kind: MessageKind;
  content: string;
  created_at: string;
}

export interface RemoteState {
  projects: RemoteProject[];
  tasks: TaskSummary[];
  selected_project_id: string | null;
  selected_task_id: string | null;
  active_run_id: string | null;
  active_task_id: string | null;
  streaming_text: string;
  status: string;
  harness: "claude" | "codex" | "omp";
  model: string | null;
  effort: "low" | "medium" | "high" | "xhigh" | "max";
  harness_ready: boolean;
}

export interface ConnectionConfig {
  endpoint: string;
  token: string;
}
