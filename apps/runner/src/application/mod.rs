pub(crate) mod events;
pub(crate) mod user_ask;

use nexus_domain::{RunStatus, UserAskAnswer, UserAskStatus};
use nexus_protocol::{Command, EnvironmentVariable, ErrorCode, Event, StartRun};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    sync::{Mutex, mpsc, watch},
    task::JoinHandle,
};
use uuid::Uuid;

use crate::infrastructure::{
    harness,
    process::{RunInput, SteerInput, generate_title, run_harness},
};
use events::Emitter;
use user_ask::PendingUserAsks;

#[derive(Clone)]
struct ActiveRun {
    id: Uuid,
    cancel: watch::Sender<bool>,
    input: mpsc::UnboundedSender<RunInput>,
    user_asks: PendingUserAsks,
}

struct BackgroundTask {
    cancel: watch::Sender<bool>,
    task: JoinHandle<()>,
}

pub(crate) struct Runner {
    active: Arc<Mutex<Option<ActiveRun>>>,
    catalog_task: Option<BackgroundTask>,
    title_tasks: Vec<BackgroundTask>,
    emitter: Emitter,
}

impl Runner {
    pub(crate) fn new(emitter: Emitter) -> Self {
        Self {
            active: Arc::new(Mutex::new(None)),
            catalog_task: None,
            title_tasks: Vec::new(),
            emitter,
        }
    }

    pub(crate) async fn handle(&mut self, command: Command) -> bool {
        self.reap_title_tasks().await;
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
            Command::ModelCatalogRefresh {
                request_id,
                harness: kind,
                executable,
                cwd,
                environment,
            } => {
                self.cancel_catalog_task().await;
                if !environment_is_valid(&environment) {
                    self.emitter
                        .send(Event::ModelCatalogFailed {
                            request_id,
                            harness: kind,
                            message: "Provider Profile 包含无效或重复的环境变量。".into(),
                        })
                        .await;
                } else {
                    let cwd = Path::new(&cwd).canonicalize();
                    match cwd {
                        Ok(cwd) if cwd.is_dir() => {
                            let (cancel, cancel_rx) = watch::channel(false);
                            let emitter = self.emitter.clone();
                            let task = tokio::spawn(async move {
                                match harness::discover_models(
                                    kind,
                                    &executable,
                                    &cwd,
                                    &environment,
                                    cancel_rx,
                                )
                                .await
                                {
                                    Ok(models) => {
                                        emitter
                                            .send(Event::ModelCatalogLoaded {
                                                request_id,
                                                harness: kind,
                                                models,
                                            })
                                            .await;
                                    }
                                    Err(nexus_harness_core::ModelCatalogError::Cancelled) => {}
                                    Err(nexus_harness_core::ModelCatalogError::Failed(message)) => {
                                        emitter
                                            .send(Event::ModelCatalogFailed {
                                                request_id,
                                                harness: kind,
                                                message,
                                            })
                                            .await;
                                    }
                                }
                            });
                            self.catalog_task = Some(BackgroundTask { cancel, task });
                        }
                        _ => {
                            self.emitter
                                .send(Event::ModelCatalogFailed {
                                    request_id,
                                    harness: kind,
                                    message: "项目目录不存在或无法访问，无法加载模型目录。".into(),
                                })
                                .await;
                        }
                    }
                }
            }
            Command::RunStart(request) => {
                let should_generate_title = request.session_id.is_none();
                let title_request = request.clone();
                if let Some(cwd) =
                    start_run(request, self.active.clone(), self.emitter.clone()).await
                    && should_generate_title
                {
                    self.spawn_title_generation(title_request, cwd);
                }
            }
            Command::RunSteer {
                run_id,
                message_id,
                prompt,
            } => {
                let guard = self.active.lock().await;
                let sent = !prompt.trim().is_empty()
                    && guard
                        .as_ref()
                        .filter(|run| run.id == run_id && !*run.cancel.borrow())
                        .is_some_and(|run| {
                            run.input
                                .send(RunInput::Steer(SteerInput { message_id, prompt }))
                                .is_ok()
                        });
                if !sent {
                    self.emitter
                        .send(Event::RunInputRejected {
                            run_id,
                            message_id,
                            message: "当前轮次无法接收 Steer，消息仍保留在队列中。".into(),
                        })
                        .await;
                }
            }
            Command::RunUserAskAnswer {
                run_id,
                request_id,
                answers,
            } => {
                answer_user_ask(run_id, request_id, answers, &self.active, &self.emitter).await;
            }
            Command::RunApprovalRespond {
                run_id,
                request_id,
                option,
            } => {
                let guard = self.active.lock().await;
                let sent = guard
                    .as_ref()
                    .filter(|run| run.id == run_id && !*run.cancel.borrow())
                    .is_some_and(|run| {
                        run.input
                            .send(RunInput::Approval { request_id, option })
                            .is_ok()
                    });
                if !sent {
                    self.emitter
                        .send(Event::RunApprovalRejected {
                            run_id,
                            request_id,
                            message: "当前轮次无法接收审批回复。".into(),
                        })
                        .await;
                }
            }
            Command::RunCancel { run_id } => {
                cancel_run(run_id, &self.active, &self.emitter).await;
            }
            Command::RunnerShutdown => return false,
        }
        true
    }

    async fn cancel_catalog_task(&mut self) {
        if let Some(task) = self.catalog_task.take() {
            let _ = task.cancel.send(true);
            let _ = task.task.await;
        }
    }

    fn spawn_title_generation(&mut self, request: StartRun, cwd: PathBuf) {
        let task_id = request.task_id;
        let (cancel, cancel_rx) = watch::channel(false);
        let emitter = self.emitter.clone();
        let task = tokio::spawn(async move {
            if let Some(title) = generate_title(request, cwd, cancel_rx).await {
                emitter
                    .send(Event::TaskTitleGenerated { task_id, title })
                    .await;
            }
        });
        self.title_tasks.push(BackgroundTask { cancel, task });
    }

    async fn reap_title_tasks(&mut self) {
        let mut index = 0;
        while index < self.title_tasks.len() {
            if self.title_tasks[index].task.is_finished() {
                let task = self.title_tasks.swap_remove(index);
                let _ = task.task.await;
            } else {
                index += 1;
            }
        }
    }

    pub(crate) async fn shutdown(&mut self) {
        self.cancel_catalog_task().await;
        let active_run = self.active.lock().await.clone();
        if let Some(run) = active_run {
            let _ = run.cancel.send(true);
            finish_user_asks(
                run.id,
                &run.user_asks,
                UserAskStatus::Cancelled,
                None,
                &self.emitter,
            )
            .await;
        }
        for task in &self.title_tasks {
            let _ = task.cancel.send(true);
        }
        while let Some(task) = self.title_tasks.pop() {
            let _ = task.task.await;
        }
    }
}

async fn start_run(
    request: StartRun,
    active: Arc<Mutex<Option<ActiveRun>>>,
    emitter: Emitter,
) -> Option<PathBuf> {
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
        return None;
    }

    if !environment_is_valid(&request.environment) {
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
        return None;
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
            return None;
        }
    };

    let (cancel, cancel_rx) = watch::channel(false);
    let (input, input_rx) = mpsc::unbounded_channel();
    let user_asks = PendingUserAsks::default();
    *guard = Some(ActiveRun {
        id: request.run_id,
        cancel,
        input,
        user_asks: user_asks.clone(),
    });
    drop(guard);

    let run_id = request.run_id;
    let active_for_task = active.clone();
    let title_cwd = cwd.clone();
    tokio::spawn(async move {
        let (status, exit_code) = run_harness(
            request,
            cwd,
            cancel_rx,
            input_rx,
            user_asks,
            emitter.clone(),
        )
        .await;
        let mut guard = active_for_task.lock().await;
        if guard.as_ref().is_some_and(|run| run.id == run_id) {
            *guard = None;
        }
        drop(guard);
        emitter
            .send(Event::RunExited {
                run_id,
                status,
                exit_code,
            })
            .await;
    });
    Some(title_cwd)
}

fn environment_is_valid(environment: &[EnvironmentVariable]) -> bool {
    let mut names = HashSet::new();
    environment
        .iter()
        .all(|variable| variable.has_safe_name() && names.insert(variable.name.as_str()))
}

async fn cancel_run(run_id: Uuid, active: &Mutex<Option<ActiveRun>>, emitter: &Emitter) {
    let guard = active.lock().await;
    let Some(run) = guard.as_ref().filter(|run| run.id == run_id) else {
        return;
    };
    let first_request = !*run.cancel.borrow();
    let _ = run.cancel.send(true);
    let user_asks = run.user_asks.clone();
    drop(guard);
    if first_request {
        finish_user_asks(run_id, &user_asks, UserAskStatus::Cancelled, None, emitter).await;
        emitter
            .send(Event::RunStatusChanged {
                run_id,
                status: RunStatus::Cancelling,
                message: Some("正在停止 Agent…".into()),
            })
            .await;
    }
}

async fn answer_user_ask(
    run_id: Uuid,
    request_id: Uuid,
    answers: Vec<UserAskAnswer>,
    active: &Mutex<Option<ActiveRun>>,
    emitter: &Emitter,
) {
    enum Route {
        Sent,
        Rejected(String),
        Failed(PendingUserAsks, String),
    }

    let route = {
        let guard = active.lock().await;
        let Some(run) = guard
            .as_ref()
            .filter(|run| run.id == run_id && !*run.cancel.borrow())
        else {
            drop(guard);
            emitter
                .send(Event::RunUserAskAnswerRejected {
                    run_id,
                    request_id,
                    message: "当前 Run 无法接收 User Ask 回答。".into(),
                })
                .await;
            return;
        };
        match run.user_asks.claim_answer(request_id, answers) {
            Ok(input) => {
                if run.input.send(RunInput::UserAsk(input)).is_ok() {
                    Route::Sent
                } else {
                    Route::Failed(
                        run.user_asks.clone(),
                        "Harness 回复通道已关闭，无法提交 User Ask 回答。".into(),
                    )
                }
            }
            Err(message) => Route::Rejected(message),
        }
    };

    match route {
        Route::Sent => {}
        Route::Rejected(message) => {
            emitter
                .send(Event::RunUserAskAnswerRejected {
                    run_id,
                    request_id,
                    message,
                })
                .await;
        }
        Route::Failed(user_asks, message) => {
            if user_asks.finish(request_id) {
                emitter
                    .send(Event::RunUserAskFinished {
                        run_id,
                        request_id,
                        status: UserAskStatus::Failed,
                        message: Some(message),
                    })
                    .await;
            }
        }
    }
}

pub(crate) async fn finish_user_asks(
    run_id: Uuid,
    user_asks: &PendingUserAsks,
    status: UserAskStatus,
    message: Option<String>,
    emitter: &Emitter,
) {
    for request_id in user_asks.finish_all() {
        emitter
            .send(Event::RunUserAskFinished {
                run_id,
                request_id,
                status,
                message: message.clone(),
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_domain::{
        HarnessKind, ThinkingEffort, UserAskAnswerMode, UserAskAnswerValue, UserAskOption,
        UserAskQuestion,
    };
    use nexus_protocol::EnvironmentVariable;

    fn request(cwd: String) -> StartRun {
        StartRun {
            permission_mode: nexus_domain::PermissionMode::AutoEdit,
            run_id: Uuid::new_v4(),
            task_id: Uuid::new_v4(),
            session_id: None,
            cwd,
            prompt: "test".into(),
            harness: HarnessKind::Claude,
            executable: "unused".into(),
            model: None,
            effort: ThinkingEffort::Medium,
            environment: Vec::new(),
        }
    }

    fn user_ask_questions() -> Vec<UserAskQuestion> {
        vec![UserAskQuestion {
            id: "scope".into(),
            prompt: "Which scope?".into(),
            answer_mode: UserAskAnswerMode::Choice {
                multiple: false,
                allow_custom: false,
            },
            options: vec![UserAskOption {
                id: "workspace".into(),
                label: "Workspace".into(),
                description: None,
            }],
        }]
    }

    fn user_ask_answers() -> Vec<UserAskAnswer> {
        vec![UserAskAnswer {
            question_id: "scope".into(),
            value: UserAskAnswerValue::Selected(vec!["workspace".into()]),
        }]
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
    async fn resumed_runs_do_not_regenerate_task_titles() {
        let directory = tempfile::tempdir().unwrap();
        let mut request = request(directory.path().to_string_lossy().into_owned());
        request.session_id = Some("existing-session".into());
        let (emitter, _) = Emitter::channel();
        let mut runner = Runner::new(emitter);

        runner.handle(Command::RunStart(request)).await;

        assert!(runner.title_tasks.is_empty());
        runner.shutdown().await;
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
            input: mpsc::unbounded_channel().0,
            user_asks: PendingUserAsks::default(),
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
        *runner.active.lock().await = Some(ActiveRun {
            id: run_id,
            cancel,
            input: mpsc::unbounded_channel().0,
            user_asks: PendingUserAsks::default(),
        });

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
    async fn cancellation_finishes_pending_user_ask_once_and_rejects_late_answers() {
        let (emitter, mut events) = Emitter::channel();
        let mut runner = Runner::new(emitter);
        let run_id = Uuid::new_v4();
        let (cancel, _cancel_receiver) = watch::channel(false);
        let (input, mut received) = mpsc::unbounded_channel();
        let user_asks = PendingUserAsks::default();
        let request_id = user_asks
            .register("native-ask".into(), user_ask_questions())
            .unwrap();
        *runner.active.lock().await = Some(ActiveRun {
            id: run_id,
            cancel,
            input,
            user_asks,
        });

        runner.handle(Command::RunCancel { run_id }).await;
        runner.handle(Command::RunCancel { run_id }).await;
        assert!(matches!(
            events.recv().await.unwrap().event,
            Event::RunUserAskFinished {
                request_id: id,
                status: UserAskStatus::Cancelled,
                ..
            } if id == request_id
        ));
        assert!(matches!(
            events.recv().await.unwrap().event,
            Event::RunStatusChanged { run_id: id, status: RunStatus::Cancelling, .. }
                if id == run_id
        ));
        assert!(events.try_recv().is_err());

        runner
            .handle(Command::RunUserAskAnswer {
                run_id,
                request_id,
                answers: user_ask_answers(),
            })
            .await;
        assert!(matches!(
            events.recv().await.unwrap().event,
            Event::RunUserAskAnswerRejected { request_id: id, .. } if id == request_id
        ));
        assert!(received.try_recv().is_err());
    }

    #[tokio::test]
    async fn steer_routes_only_to_the_matching_live_run() {
        let (emitter, mut events) = Emitter::channel();
        let mut runner = Runner::new(emitter);
        let run_id = Uuid::new_v4();
        let (cancel, _) = watch::channel(false);
        let (input, mut received) = mpsc::unbounded_channel();
        *runner.active.lock().await = Some(ActiveRun {
            id: run_id,
            cancel: cancel.clone(),
            input,
            user_asks: PendingUserAsks::default(),
        });
        let message_id = Uuid::new_v4();
        runner
            .handle(Command::RunSteer {
                run_id,
                message_id,
                prompt: "correction".into(),
            })
            .await;
        let RunInput::Steer(message) = received.recv().await.unwrap() else {
            panic!("expected Steer input")
        };
        assert_eq!(message.message_id, message_id);
        assert_eq!(message.prompt, "correction");
        assert!(
            events.try_recv().is_err(),
            "only the harness can acknowledge delivery"
        );
        for (id, prompt) in [(Uuid::new_v4(), "wrong run"), (run_id, " \n ")] {
            runner
                .handle(Command::RunSteer {
                    run_id: id,
                    message_id,
                    prompt: prompt.into(),
                })
                .await;
            assert!(matches!(
                events.recv().await.unwrap().event,
                Event::RunInputRejected { .. }
            ));
            assert!(received.try_recv().is_err());
        }
        cancel.send_replace(true);
        runner
            .handle(Command::RunSteer {
                run_id,
                message_id,
                prompt: "too late".into(),
            })
            .await;
        assert!(matches!(
            events.recv().await.unwrap().event,
            Event::RunInputRejected { .. }
        ));
        assert!(received.try_recv().is_err());
    }

    #[tokio::test]
    async fn user_ask_answer_routes_once_to_the_matching_live_run() {
        let (emitter, mut events) = Emitter::channel();
        let mut runner = Runner::new(emitter);
        let run_id = Uuid::new_v4();
        let user_asks = PendingUserAsks::default();
        let request_id = user_asks
            .register("native-ask".into(), user_ask_questions())
            .unwrap();
        let (cancel, _) = watch::channel(false);
        let (input, mut received) = mpsc::unbounded_channel();
        *runner.active.lock().await = Some(ActiveRun {
            id: run_id,
            cancel,
            input,
            user_asks,
        });

        for (wrong_run, wrong_request) in [(Uuid::new_v4(), request_id), (run_id, Uuid::new_v4())] {
            runner
                .handle(Command::RunUserAskAnswer {
                    run_id: wrong_run,
                    request_id: wrong_request,
                    answers: user_ask_answers(),
                })
                .await;
            assert!(matches!(
                events.recv().await.unwrap().event,
                Event::RunUserAskAnswerRejected { run_id: id, request_id: request, .. }
                    if id == wrong_run && request == wrong_request
            ));
            assert!(received.try_recv().is_err());
        }

        runner
            .handle(Command::RunUserAskAnswer {
                run_id,
                request_id,
                answers: user_ask_answers(),
            })
            .await;
        let RunInput::UserAsk(answer) = received.recv().await.unwrap() else {
            panic!("expected User Ask input")
        };
        assert_eq!(answer.request_id, request_id);
        assert_eq!(answer.native_request_id, "native-ask");
        assert!(events.try_recv().is_err());

        runner
            .handle(Command::RunUserAskAnswer {
                run_id,
                request_id,
                answers: user_ask_answers(),
            })
            .await;
        assert!(matches!(
            events.recv().await.unwrap().event,
            Event::RunUserAskAnswerRejected { request_id: id, .. } if id == request_id
        ));
        assert!(received.try_recv().is_err());
    }

    #[tokio::test]
    async fn shutdown_cancels_active_run() {
        let (emitter, mut events) = Emitter::channel();
        let mut runner = Runner::new(emitter);
        let (cancel, receiver) = watch::channel(false);
        let user_asks = PendingUserAsks::default();
        let request_id = user_asks
            .register("native-ask".into(), user_ask_questions())
            .unwrap();
        *runner.active.lock().await = Some(ActiveRun {
            id: Uuid::new_v4(),
            cancel,
            input: mpsc::unbounded_channel().0,
            user_asks,
        });

        assert!(!runner.handle(Command::RunnerShutdown).await);
        runner.shutdown().await;
        assert!(*receiver.borrow());
        assert!(matches!(
            events.recv().await.unwrap().event,
            Event::RunUserAskFinished {
                request_id: id,
                status: UserAskStatus::Cancelled,
                ..
            } if id == request_id
        ));
    }
}
