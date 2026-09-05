use crate::application::events::{Emitter, emit_decoded};
use nexus_domain::{HarnessKind, RunStatus};
use nexus_harness_core::{DecodedEvent, LineDecoder};
use nexus_protocol::{ErrorCode, Event, StartRun};
#[cfg(unix)]
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use std::time::Duration;
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    process::Command as ProcessCommand,
    sync::watch,
    time::timeout,
};
use uuid::Uuid;

const CANCEL_GRACE_PERIOD: Duration = Duration::from_secs(3);

pub(crate) async fn run_harness(
    request: StartRun,
    cwd: std::path::PathBuf,
    mut cancel: watch::Receiver<bool>,
    emitter: Emitter,
) {
    let harness = request.harness;
    let (spec, decoder) = super::harness::prepare(&request, &cwd);
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
                    message: format!("无法启动 {harness}，请重新探测可执行文件。"),
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
                message: format!("无法向 {harness} 发送 Prompt。"),
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
    let stdout_task = tokio::spawn(read_stdout(
        stdout,
        request.run_id,
        harness,
        decoder,
        emitter.clone(),
    ));
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

    let provider_error = stdout_task.await.ok().flatten();
    let _ = stderr_task.await;
    let exit_code = status.as_ref().ok().and_then(|status| status.code());
    let final_status = if was_cancelled {
        RunStatus::Cancelled
    } else if provider_error.is_none() && status.as_ref().is_ok_and(|status| status.success()) {
        RunStatus::Completed
    } else {
        emitter
            .send(Event::RunFailed {
                run_id: request.run_id,
                code: ErrorCode::UnexpectedExit,
                message: provider_error.unwrap_or_else(|| match exit_code {
                    Some(code) => {
                        format!("{harness} 异常退出（代码 {code}）。请检查登录状态或诊断日志。")
                    }
                    None => format!("{harness} 异常退出。请检查登录状态或诊断日志。"),
                }),
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

async fn read_stdout(
    stdout: Option<tokio::process::ChildStdout>,
    run_id: Uuid,
    harness: HarnessKind,
    mut decoder: Box<dyn LineDecoder>,
    emitter: Emitter,
) -> Option<String> {
    let stdout = stdout?;
    let mut lines = BufReader::new(stdout).lines();
    let mut provider_error = None;
    while let Ok(Some(line)) = lines.next_line().await {
        match decoder.decode_line(&line) {
            Ok(events) => {
                for event in events {
                    if let DecodedEvent::Error(message) = &event {
                        provider_error = Some(message.clone());
                    }
                    emit_decoded(run_id, event, &emitter).await;
                }
            }
            Err(_) => {
                emitter
                    .send(Event::RunStatusChanged {
                        run_id,
                        status: RunStatus::Running,
                        message: Some(format!("已忽略一条无法解析的 {harness} 输出。")),
                    })
                    .await;
            }
        }
    }
    provider_error
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
