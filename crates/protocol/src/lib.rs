use nexus_domain::{HarnessKind, ModelDescriptor, RunStatus, ThinkingEffort};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

pub const PROTOCOL_VERSION: u32 = 8;

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
    #[serde(rename = "model.catalog.refresh")]
    ModelCatalogRefresh {
        request_id: Uuid,
        harness: HarnessKind,
        executable: String,
        cwd: String,
        #[serde(default)]
        environment: Vec<EnvironmentVariable>,
    },
    #[serde(rename = "run.start")]
    RunStart(StartRun),
    #[serde(rename = "run.steer")]
    RunSteer {
        run_id: Uuid,
        message_id: Uuid,
        prompt: String,
    },
    #[serde(rename = "run.cancel")]
    RunCancel { run_id: Uuid },
    #[serde(rename = "runner.shutdown")]
    RunnerShutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartRun {
    pub run_id: Uuid,
    pub task_id: Uuid,
    pub session_id: Option<String>,
    pub cwd: String,
    pub prompt: String,
    pub harness: HarnessKind,
    pub executable: String,
    pub model: Option<String>,
    pub effort: ThinkingEffort,
    #[serde(default)]
    pub environment: Vec<EnvironmentVariable>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentVariable {
    pub name: String,
    pub value: String,
}

impl EnvironmentVariable {
    pub fn has_safe_name(&self) -> bool {
        let mut characters = self.name.chars();
        let valid_identifier = characters
            .next()
            .is_some_and(|character| character == '_' || character.is_ascii_uppercase())
            && characters.all(|character| {
                character == '_' || character.is_ascii_uppercase() || character.is_ascii_digit()
            });
        valid_identifier
            && !matches!(
                self.name.as_str(),
                "HOME"
                    | "PATH"
                    | "PATHEXT"
                    | "SHELL"
                    | "COMSPEC"
                    | "LD_PRELOAD"
                    | "LD_LIBRARY_PATH"
            )
            && !self.name.starts_with("DYLD_")
            && !self.name.starts_with("NEXUS_")
    }
}

impl fmt::Debug for EnvironmentVariable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EnvironmentVariable")
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .finish()
    }
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
    #[serde(rename = "model.catalog.loaded")]
    ModelCatalogLoaded {
        request_id: Uuid,
        harness: HarnessKind,
        models: Vec<ModelDescriptor>,
    },
    #[serde(rename = "model.catalog.failed")]
    ModelCatalogFailed {
        request_id: Uuid,
        harness: HarnessKind,
        message: String,
    },
    #[serde(rename = "run.started")]
    RunStarted { run_id: Uuid, pid: u32 },
    #[serde(rename = "run.session.started")]
    RunSessionStarted { run_id: Uuid, session_id: String },
    #[serde(rename = "run.input.accepted")]
    RunInputAccepted { run_id: Uuid, message_id: Uuid },
    #[serde(rename = "run.input.rejected")]
    RunInputRejected {
        run_id: Uuid,
        message_id: Uuid,
        message: String,
    },
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
    #[serde(rename = "task.title.generated")]
    TaskTitleGenerated { task_id: Uuid, title: String },
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
    InvalidEnvironment,
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
    fn steer_round_trip_preserves_run_and_message_identity() {
        let run_id = Uuid::new_v4();
        let message_id = Uuid::new_v4();
        let command = CommandEnvelope::new(Command::RunSteer {
            run_id,
            message_id,
            prompt: "update\n指令".into(),
        });
        let encoded = serde_json::to_string(&command).unwrap();
        let decoded: CommandEnvelope = serde_json::from_str(&encoded).unwrap();
        assert!(
            matches!(decoded.command, Command::RunSteer { run_id: run, message_id: message, prompt }
            if run == run_id && message == message_id && prompt == "update\n指令")
        );
        for event in [
            Event::RunInputAccepted { run_id, message_id },
            Event::RunInputRejected {
                run_id,
                message_id,
                message: "ended".into(),
            },
        ] {
            let encoded = serde_json::to_value(&event).unwrap();
            let decoded: Event = serde_json::from_value(encoded.clone()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
        }
    }

    #[test]
    fn protocol_round_trip_preserves_harness_model_and_effort() {
        let command = CommandEnvelope::new(Command::RunStart(StartRun {
            run_id: Uuid::new_v4(),
            task_id: Uuid::new_v4(),
            session_id: Some("existing-session".into()),
            cwd: "/tmp/project".into(),
            prompt: "fix it".into(),
            harness: HarnessKind::Codex,
            executable: "codex".into(),
            model: Some("gpt-test".into()),
            effort: ThinkingEffort::XHigh,
            environment: vec![EnvironmentVariable {
                name: "OPENAI_API_KEY".into(),
                value: "secret-value".into(),
            }],
        }));
        let json = serde_json::to_string(&command).unwrap();
        assert!(json.contains(r#""kind":"run.start""#));
        assert!(!format!("{command:?}").contains("secret-value"));
        let decoded: CommandEnvelope = serde_json::from_str(&json).unwrap();
        let Command::RunStart(request) = decoded.command else {
            panic!("expected run.start")
        };
        assert_eq!(request.harness, HarnessKind::Codex);
        assert_eq!(request.session_id.as_deref(), Some("existing-session"));
        assert_eq!(request.model.as_deref(), Some("gpt-test"));
        assert_eq!(request.effort, ThinkingEffort::XHigh);
        assert_eq!(request.environment[0].name, "OPENAI_API_KEY");
        assert_eq!(request.environment[0].value, "secret-value");
    }

    #[test]
    fn protocol_round_trip_preserves_model_catalog_request_context() {
        let request_id = Uuid::new_v4();
        let command = CommandEnvelope::new(Command::ModelCatalogRefresh {
            request_id,
            harness: HarnessKind::Codex,
            executable: "/usr/local/bin/codex".into(),
            cwd: "/tmp/project".into(),
            environment: vec![EnvironmentVariable {
                name: "CODEX_API_KEY".into(),
                value: "secret-value".into(),
            }],
        });

        let json = serde_json::to_string(&command).unwrap();
        assert!(json.contains(r#""kind":"model.catalog.refresh""#));
        assert!(!format!("{command:?}").contains("secret-value"));
        let decoded: CommandEnvelope = serde_json::from_str(&json).unwrap();
        let Command::ModelCatalogRefresh {
            request_id: decoded_id,
            harness,
            executable,
            cwd,
            environment,
        } = decoded.command
        else {
            panic!("expected model.catalog.refresh")
        };
        assert_eq!(decoded_id, request_id);
        assert_eq!(harness, HarnessKind::Codex);
        assert_eq!(executable, "/usr/local/bin/codex");
        assert_eq!(cwd, "/tmp/project");
        assert_eq!(environment[0].value, "secret-value");
    }

    #[test]
    fn protocol_round_trip_preserves_model_provider_metadata() {
        let request_id = Uuid::new_v4();
        let event = EventEnvelope {
            protocol_version: PROTOCOL_VERSION,
            id: Uuid::new_v4(),
            sequence: 1,
            event: Event::ModelCatalogLoaded {
                request_id,
                harness: HarnessKind::Omp,
                models: vec![ModelDescriptor {
                    id: "provider/model".into(),
                    display_name: "Model".into(),
                    source: nexus_domain::ModelSource::OmpCli,
                    availability: nexus_domain::ModelAvailability::Unavailable {
                        reason: "Provider disabled".into(),
                    },
                    provider: Some("provider".into()),
                    is_default: false,
                    supported_reasoning_efforts: Vec::new(),
                    default_reasoning_effort: None,
                }],
            },
        };

        let json = serde_json::to_string(&event).unwrap();
        let decoded: EventEnvelope = serde_json::from_str(&json).unwrap();
        let Event::ModelCatalogLoaded { models, .. } = decoded.event else {
            panic!("expected model catalog")
        };
        assert_eq!(models[0].provider.as_deref(), Some("provider"));
        assert_eq!(models[0].id, "provider/model");
        assert_eq!(models[0].source.harness(), HarnessKind::Omp);
        assert_eq!(
            models[0].availability,
            nexus_domain::ModelAvailability::Unavailable {
                reason: "Provider disabled".into()
            }
        );
        assert!(!models[0].availability.is_selectable());
    }

    #[test]
    fn environment_names_reject_process_control_variables() {
        for name in ["OPENAI_API_KEY", "ANTHROPIC_BASE_URL", "_PRIVATE_TOKEN"] {
            assert!(
                EnvironmentVariable {
                    name: name.into(),
                    value: "secret".into(),
                }
                .has_safe_name()
            );
        }
        for name in ["PATH", "LD_PRELOAD", "DYLD_INSERT_LIBRARIES", "lowercase"] {
            assert!(
                !EnvironmentVariable {
                    name: name.into(),
                    value: "secret".into(),
                }
                .has_safe_name()
            );
        }
    }

    #[test]
    fn protocol_round_trip_preserves_generated_task_title() {
        let task_id = Uuid::new_v4();
        let envelope = EventEnvelope {
            protocol_version: PROTOCOL_VERSION,
            id: Uuid::new_v4(),
            sequence: 1,
            event: Event::TaskTitleGenerated {
                task_id,
                title: "修复登录流程".into(),
            },
        };

        let json = serde_json::to_string(&envelope).unwrap();
        assert!(json.contains(r#""kind":"task.title.generated""#));
        let decoded: EventEnvelope = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            decoded.event,
            Event::TaskTitleGenerated { task_id: id, title }
                if id == task_id && title == "修复登录流程"
        ));
    }
}
