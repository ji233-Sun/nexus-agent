pub(crate) mod events;

use nexus_domain::RunStatus;
use nexus_protocol::{Command, ErrorCode, Event, StartRun};
use std::{collections::HashSet, path::Path, sync::Arc};
use tokio::sync::{Mutex, watch};
use uuid::Uuid;

use crate::infrastructure::{harness, process::run_harness};
use events::Emitter;

#[derive(Clone)]
struct ActiveRun {
    id: Uuid,
    cancel: watch::Sender<bool>,
}

pub(crate) struct Runner {
    active: Arc<Mutex<Option<ActiveRun>>>,
    emitter: Emitter,
}

impl Runner {
    pub(crate) fn new(emitter: Emitter) -> Self {
        Self {
            active: Arc::new(Mutex::new(None)),
            emitter,
        }
    }

    pub(crate) async fn handle(&mut self, command: Command) -> bool {
        match command {
            Command::RunnerHello => self.emitter.send(Event::RunnerReady).await,
            Command::HarnessProbe {
                harness: kind,
                executable,
            } => {
                self.emitter
                    .send(Event::HarnessDetected(
                        harness::probe(kind, &executable).await,
                    ))
                    .await;
            }
            Command::RunStart(request) => {
                start_run(request, self.active.clone(), self.emitter.clone()).await;
            }
            Command::RunCancel { run_id } => {
                cancel_run(run_id, &self.active, &self.emitter).await;
            }
            Command::RunnerShutdown => return false,
        }
        true
    }

    pub(crate) async fn shutdown(&self) {
        if let Some(run) = self.active.lock().await.as_ref() {
            let _ = run.cancel.send(true);
        }
    }
}

async fn start_run(request: StartRun, active: Arc<Mutex<Option<ActiveRun>>>, emitter: Emitter) {
    let mut guard = active.lock().await;
    if guard.is_some() {
        emitter
            .send(Event::RunFailed {
                run_id: request.run_id,
                code: ErrorCode::RunAlreadyActive,
                message: "已有 Agent 任务正在运行，请先等待或取消。".into(),
            })
            .await;
        emitter
            .send(Event::RunExited {
                run_id: request.run_id,
                status: RunStatus::Failed,
                exit_code: None,
            })
            .await;
        return;
    }

    let mut environment_names = HashSet::new();
    if request.environment.iter().any(|variable| {
        !variable.has_safe_name() || !environment_names.insert(variable.name.as_str())
    }) {
        emitter
            .send(Event::RunFailed {
                run_id: request.run_id,
                code: ErrorCode::InvalidEnvironment,
                message: "Provider Profile 包含无效或重复的环境变量。".into(),
            })
            .await;
        emitter
            .send(Event::RunExited {
                run_id: request.run_id,
                status: RunStatus::Failed,
                exit_code: None,
            })
            .await;
        return;
    }

    let cwd = match Path::new(&request.cwd).canonicalize() {
        Ok(cwd) if cwd.is_dir() => cwd,
        _ => {
            emitter
                .send(Event::RunFailed {
                    run_id: request.run_id,
                    code: ErrorCode::ProjectNotFound,
                    message: "项目目录不存在或无法访问。".into(),
                })
                .await;
            emitter
                .send(Event::RunExited {
                    run_id: request.run_id,
                    status: RunStatus::Failed,
                    exit_code: None,
                })
                .await;
            return;
        }
    };

    let (cancel, cancel_rx) = watch::channel(false);
    *guard = Some(ActiveRun {
        id: request.run_id,
        cancel,
    });
    drop(guard);

    let run_id = request.run_id;
    let active_for_task = active.clone();
    tokio::spawn(async move {
        run_harness(request, cwd, cancel_rx, emitter).await;
        let mut guard = active_for_task.lock().await;
        if guard.as_ref().is_some_and(|run| run.id == run_id) {
            *guard = None;
        }
    });
}

async fn cancel_run(run_id: Uuid, active: &Mutex<Option<ActiveRun>>, emitter: &Emitter) {
    let guard = active.lock().await;
    let Some(run) = guard.as_ref().filter(|run| run.id == run_id) else {
        return;
    };
    let first_request = !*run.cancel.borrow();
    let _ = run.cancel.send(true);
    if first_request {
        emitter
            .send(Event::RunStatusChanged {
                run_id,
                status: RunStatus::Cancelling,
                message: Some("正在停止 Agent…".into()),
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_domain::{HarnessKind, ThinkingEffort};
    use nexus_protocol::EnvironmentVariable;

    fn request(cwd: String) -> StartRun {
        StartRun {
            run_id: Uuid::new_v4(),
            task_id: Uuid::new_v4(),
            cwd,
            prompt: "test".into(),
            harness: HarnessKind::Claude,
            executable: "unused".into(),
            model: None,
            effort: ThinkingEffort::Medium,
            environment: Vec::new(),
        }
    }

    #[tokio::test]
    async fn missing_project_fails_without_occupying_run_slot() {
        let directory = tempfile::tempdir().unwrap();
        let request = request(
            directory
                .path()
                .join("missing")
                .to_string_lossy()
                .into_owned(),
        );
        let run_id = request.run_id;
        let (emitter, mut events) = Emitter::channel();
        let mut runner = Runner::new(emitter);

        assert!(runner.handle(Command::RunStart(request)).await);
        assert!(matches!(events.recv().await.unwrap().event,
            Event::RunFailed { run_id: id, code: ErrorCode::ProjectNotFound, .. } if id == run_id));
        assert!(matches!(events.recv().await.unwrap().event,
            Event::RunExited { run_id: id, status: RunStatus::Failed, exit_code: None } if id == run_id));
        assert!(runner.active.lock().await.is_none());
    }

    #[tokio::test]
    async fn unsafe_environment_fails_without_starting_a_run() {
        let directory = tempfile::tempdir().unwrap();
        let mut request = request(directory.path().to_string_lossy().into_owned());
        request.environment.push(EnvironmentVariable {
            name: "LD_PRELOAD".into(),
            value: "unsafe".into(),
        });
        let run_id = request.run_id;
        let (emitter, mut events) = Emitter::channel();
        let mut runner = Runner::new(emitter);

        runner.handle(Command::RunStart(request)).await;

        assert!(matches!(events.recv().await.unwrap().event,
            Event::RunFailed { run_id: id, code: ErrorCode::InvalidEnvironment, .. } if id == run_id));
        assert!(matches!(events.recv().await.unwrap().event,
            Event::RunExited { run_id: id, status: RunStatus::Failed, .. } if id == run_id));
        assert!(runner.active.lock().await.is_none());
    }

    #[tokio::test]
    async fn second_run_is_rejected_without_replacing_active_run() {
        let (emitter, mut events) = Emitter::channel();
        let mut runner = Runner::new(emitter);
        let active_id = Uuid::new_v4();
        let (cancel, receiver) = watch::channel(false);
        *runner.active.lock().await = Some(ActiveRun {
            id: active_id,
            cancel,
        });
        let request = request("unused".into());
        let rejected_id = request.run_id;

        runner.handle(Command::RunStart(request)).await;
        assert!(matches!(events.recv().await.unwrap().event,
            Event::RunFailed { run_id, code: ErrorCode::RunAlreadyActive, .. } if run_id == rejected_id));
        assert!(matches!(events.recv().await.unwrap().event,
            Event::RunExited { run_id, status: RunStatus::Failed, .. } if run_id == rejected_id));
        assert_eq!(runner.active.lock().await.as_ref().unwrap().id, active_id);
        assert!(!*receiver.borrow());
    }

    #[tokio::test]
    async fn cancellation_is_scoped_and_idempotent() {
        let (emitter, mut events) = Emitter::channel();
        let mut runner = Runner::new(emitter);
        let run_id = Uuid::new_v4();
        let (cancel, receiver) = watch::channel(false);
        *runner.active.lock().await = Some(ActiveRun { id: run_id, cancel });

        runner
            .handle(Command::RunCancel {
                run_id: Uuid::new_v4(),
            })
            .await;
        assert!(!*receiver.borrow());
        assert!(events.try_recv().is_err());
        runner.handle(Command::RunCancel { run_id }).await;
        runner.handle(Command::RunCancel { run_id }).await;
        assert!(*receiver.borrow());
        assert!(matches!(events.recv().await.unwrap().event,
            Event::RunStatusChanged { run_id: id, status: RunStatus::Cancelling, .. } if id == run_id));
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn shutdown_cancels_active_run() {
        let (emitter, _) = Emitter::channel();
        let mut runner = Runner::new(emitter);
        let (cancel, receiver) = watch::channel(false);
        *runner.active.lock().await = Some(ActiveRun {
            id: Uuid::new_v4(),
            cancel,
        });

        assert!(!runner.handle(Command::RunnerShutdown).await);
        runner.shutdown().await;
        assert!(*receiver.borrow());
    }
}
