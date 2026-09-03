use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context as _, Result};
use nexus_domain::RunStatus;
use nexus_harness_claude::{DecodedEvent, EventDecoder, build_launch_spec, probe};
use nexus_protocol::{
    Command, CommandEnvelope, ErrorCode, Event, EventEnvelope, PROTOCOL_VERSION, StartRun,
};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    process::Command as ProcessCommand,
    sync::{Mutex, mpsc, watch},
    time::timeout,
};
use uuid::Uuid;

const CANCEL_GRACE_PERIOD: Duration = Duration::from_secs(3);

#[derive(Clone)]
struct Emitter {
    tx: mpsc::Sender<EventEnvelope>,
    sequence: Arc<AtomicU64>,
}

impl Emitter {
    async fn send(&self, event: Event) {
        let envelope = EventEnvelope {
            protocol_version: PROTOCOL_VERSION,
            id: Uuid::new_v4(),
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            event,
        };
        let _ = self.tx.send(envelope).await;
    }
}

#[derive(Clone)]
struct ActiveRun {
    id: Uuid,
    cancel: watch::Sender<bool>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let (event_tx, mut event_rx) = mpsc::channel::<EventEnvelope>(256);
    let emitter = Emitter {
        tx: event_tx,
        sequence: Arc::new(AtomicU64::new(1)),
    };
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(event) = event_rx.recv().await {
            let mut frame = serde_json::to_vec(&event).context("serialize runner event")?;
            frame.push(b'\n');
            stdout
                .write_all(&frame)
                .await
                .context("write runner event")?;
            stdout.flush().await.context("flush runner event")?;
        }
        Ok::<(), anyhow::Error>(())
    });

    let active = Arc::new(Mutex::new(None::<ActiveRun>));
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    while let Some(line) = lines.next_line().await.context("read command")? {
        let command: CommandEnvelope = match serde_json::from_str(&line) {
            Ok(command) => command,
            Err(error) => {
                eprintln!("invalid command frame: {error}");
                continue;
            }
        };
        if command.protocol_version != PROTOCOL_VERSION {
            eprintln!(
                "protocol version mismatch: expected {PROTOCOL_VERSION}, got {}",
                command.protocol_version
            );
            continue;
        }

        match command.command {
            Command::RunnerHello => emitter.send(Event::RunnerReady).await,
            Command::HarnessProbe { executable } => {
                let result = probe(&executable).await;
                emitter.send(Event::HarnessDetected(result)).await;
            }
            Command::RunStart(request) => {
                start_run(request, active.clone(), emitter.clone()).await;
            }
            Command::RunCancel { run_id } => {
                cancel_run(run_id, &active, &emitter).await;
            }
            Command::RunnerShutdown => {
                if let Some(run) = active.lock().await.as_ref() {
                    let _ = run.cancel.send(true);
                }
                break;
            }
        }
    }

    if let Some(run) = active.lock().await.as_ref() {
        let _ = run.cancel.send(true);
    }
    drop(emitter);
    writer.await.context("join event writer")??;
    Ok(())
}

async fn start_run(request: StartRun, active: Arc<Mutex<Option<ActiveRun>>>, emitter: Emitter) {
    let mut guard = active.lock().await;
    if guard.is_some() {
        emitter
            .send(Event::RunFailed {
                run_id: request.run_id,
                code: ErrorCode::RunAlreadyActive,
                message: "已有 Claude Code 任务正在运行，请先等待或取消。".into(),
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
        run_claude(request, cwd, cancel_rx, emitter).await;
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
                message: Some("正在停止 Claude Code…".into()),
            })
            .await;
    }
}

async fn run_claude(
    request: StartRun,
    cwd: std::path::PathBuf,
    mut cancel: watch::Receiver<bool>,
    emitter: Emitter,
) {
    let spec = build_launch_spec(
        &request.executable,
        &cwd,
        &request.prompt,
        request.model,
        request.effort,
    );
    let mut command = ProcessCommand::new(&spec.executable);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            emitter
                .send(Event::RunFailed {
                    run_id: request.run_id,
                    code: ErrorCode::LaunchFailed,
                    message: "无法启动 Claude Code，请重新探测可执行文件。".into(),
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
    let pid = child.id().unwrap_or_default();
    emitter
        .send(Event::RunStarted {
            run_id: request.run_id,
            pid,
        })
        .await;

    if let Some(mut stdin) = child.stdin.take()
        && stdin.write_all(spec.stdin.as_bytes()).await.is_err()
    {
        let _ = child.start_kill();
        emitter
            .send(Event::RunFailed {
                run_id: request.run_id,
                code: ErrorCode::LaunchFailed,
                message: "无法向 Claude Code 发送 Prompt。".into(),
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

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_task = tokio::spawn(read_stdout(stdout, request.run_id, emitter.clone()));
    let stderr_task = tokio::spawn(async move {
        let mut captured = String::new();
        if let Some(stderr) = stderr {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if captured.len() < 2_048 {
                    captured.push_str(&line);
                    captured.push('\n');
                }
            }
        }
        captured
    });

    let (status, was_cancelled) = tokio::select! {
        status = child.wait() => (status, false),
        changed = cancel.changed() => {
            let cancelled = changed.is_ok() && *cancel.borrow();
            if cancelled {
                interrupt_process_group(pid);
                match timeout(CANCEL_GRACE_PERIOD, child.wait()).await {
                    Ok(status) => (status, true),
                    Err(_) => {
                        kill_process_group(pid);
                        (child.wait().await, true)
                    }
                }
            } else {
                (child.wait().await, false)
            }
        }
    };

    let _ = stdout_task.await;
    let _ = stderr_task.await;
    let exit_code = status.as_ref().ok().and_then(|status| status.code());
    let final_status = if was_cancelled {
        RunStatus::Cancelled
    } else if status.as_ref().is_ok_and(|status| status.success()) {
        RunStatus::Completed
    } else {
        emitter
            .send(Event::RunFailed {
                run_id: request.run_id,
                code: ErrorCode::UnexpectedExit,
                message: match exit_code {
                    Some(code) => {
                        format!("Claude Code 异常退出（代码 {code}）。请检查登录状态或诊断日志。")
                    }
                    None => "Claude Code 异常退出。请检查登录状态或诊断日志。".into(),
                },
            })
            .await;
        RunStatus::Failed
    };
    emitter
        .send(Event::RunExited {
            run_id: request.run_id,
            status: final_status,
            exit_code,
        })
        .await;
}

async fn read_stdout(stdout: Option<tokio::process::ChildStdout>, run_id: Uuid, emitter: Emitter) {
    let Some(stdout) = stdout else {
        return;
    };
    let mut lines = BufReader::new(stdout).lines();
    let mut decoder = EventDecoder;
    while let Ok(Some(line)) = lines.next_line().await {
        match decoder.decode_line(&line) {
            Ok(events) => {
                for event in events {
                    emit_decoded(run_id, event, &emitter).await;
                }
            }
            Err(_) => {
                emitter
                    .send(Event::RunStatusChanged {
                        run_id,
                        status: RunStatus::Running,
                        message: Some("已忽略一条无法解析的 Claude Code 输出。".into()),
                    })
                    .await;
            }
        }
    }
}

async fn emit_decoded(run_id: Uuid, decoded: DecodedEvent, emitter: &Emitter) {
    let event = match decoded {
        DecodedEvent::TextDelta(text) => Event::RunOutputDelta { run_id, text },
        DecodedEvent::MessageCompleted(text) => Event::RunMessageCompleted { run_id, text },
        DecodedEvent::ToolStarted { id, name, summary } => Event::RunToolStarted {
            run_id,
            tool_id: id,
            name,
            summary,
        },
        DecodedEvent::ToolCompleted {
            id,
            output,
            is_error,
        } => Event::RunToolCompleted {
            run_id,
            tool_id: id,
            output,
            is_error,
        },
        DecodedEvent::Status(message) => Event::RunStatusChanged {
            run_id,
            status: RunStatus::Running,
            message: Some(message),
        },
    };
    emitter.send(event).await;
}

#[cfg(unix)]
fn interrupt_process_group(pid: u32) {
    if pid > 0 {
        let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGINT);
    }
}

#[cfg(not(unix))]
fn interrupt_process_group(_pid: u32) {}

#[cfg(unix)]
fn kill_process_group(pid: u32) {
    if pid > 0 {
        let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_process_group(_pid: u32) {}
