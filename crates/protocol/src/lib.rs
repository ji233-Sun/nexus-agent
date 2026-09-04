use nexus_domain::{HarnessKind, RunStatus, ThinkingEffort};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PROTOCOL_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandEnvelope {
    pub protocol_version: u32,
    pub id: Uuid,
    #[serde(flatten)]
    pub command: Command,
}

impl CommandEnvelope {
    pub fn new(command: Command) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            id: Uuid::new_v4(),
            command,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum Command {
    #[serde(rename = "runner.hello")]
    RunnerHello,
    #[serde(rename = "harness.probe")]
    HarnessProbe {
        harness: HarnessKind,
        executable: String,
    },
    #[serde(rename = "run.start")]
    RunStart(StartRun),
    #[serde(rename = "run.cancel")]
    RunCancel { run_id: Uuid },
    #[serde(rename = "runner.shutdown")]
    RunnerShutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartRun {
    pub run_id: Uuid,
    pub task_id: Uuid,
    pub cwd: String,
    pub prompt: String,
    pub harness: HarnessKind,
    pub executable: String,
    pub model: Option<String>,
    pub effort: ThinkingEffort,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub protocol_version: u32,
    pub id: Uuid,
    pub sequence: u64,
    #[serde(flatten)]
    pub event: Event,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum Event {
    #[serde(rename = "runner.ready")]
    RunnerReady,
    #[serde(rename = "harness.detected")]
    HarnessDetected(HarnessProbe),
    #[serde(rename = "run.started")]
    RunStarted { run_id: Uuid, pid: u32 },
    #[serde(rename = "run.output.delta")]
    RunOutputDelta { run_id: Uuid, text: String },
    #[serde(rename = "run.message.completed")]
    RunMessageCompleted { run_id: Uuid, text: String },
    #[serde(rename = "run.tool.started")]
    RunToolStarted {
        run_id: Uuid,
        tool_id: String,
        name: String,
        summary: String,
    },
    #[serde(rename = "run.tool.completed")]
    RunToolCompleted {
        run_id: Uuid,
        tool_id: String,
        output: String,
        is_error: bool,
    },
    #[serde(rename = "run.status.changed")]
    RunStatusChanged {
        run_id: Uuid,
        status: RunStatus,
        message: Option<String>,
    },
    #[serde(rename = "run.failed")]
    RunFailed {
        run_id: Uuid,
        code: ErrorCode,
        message: String,
    },
    #[serde(rename = "run.exited")]
    RunExited {
        run_id: Uuid,
        status: RunStatus,
        exit_code: Option<i32>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessProbe {
    pub harness: HarnessKind,
    pub available: bool,
    pub authenticated: bool,
    pub executable: String,
    pub version: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    HarnessNotFound,
    HarnessNotExecutable,
    HarnessNotAuthenticated,
    ProjectNotFound,
    ProjectPermissionDenied,
    ProtocolVersionMismatch,
    RunAlreadyActive,
    LaunchFailed,
    MalformedHarnessOutput,
    CancellationTimeout,
    UnexpectedExit,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_round_trip_preserves_harness_model_and_effort() {
        let command = CommandEnvelope::new(Command::RunStart(StartRun {
            run_id: Uuid::new_v4(),
            task_id: Uuid::new_v4(),
            cwd: "/tmp/project".into(),
            prompt: "fix it".into(),
            harness: HarnessKind::Codex,
            executable: "codex".into(),
            model: Some("gpt-test".into()),
            effort: ThinkingEffort::XHigh,
        }));
        let json = serde_json::to_string(&command).unwrap();
        assert!(json.contains(r#""kind":"run.start""#));
        let decoded: CommandEnvelope = serde_json::from_str(&json).unwrap();
        let Command::RunStart(request) = decoded.command else {
            panic!("expected run.start")
        };
        assert_eq!(request.harness, HarnessKind::Codex);
        assert_eq!(request.model.as_deref(), Some("gpt-test"));
        assert_eq!(request.effort, ThinkingEffort::XHigh);
    }
}
