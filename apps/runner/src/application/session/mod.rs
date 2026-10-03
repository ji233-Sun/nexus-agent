mod stream;
#[cfg(test)]
mod tests;

use super::{
    events::Emitter,
    finish_user_asks,
    user_ask::{PendingUserAsks, UserAskInput},
};
use crate::infrastructure::{harness, process::command as process_command, process_tree};
use nexus_domain::{HarnessKind, RunStatus, UserAskStatus};
use nexus_harness_core::{LaunchSpec, LineDecoder};
use nexus_protocol::{ErrorCode, Event, StartRun};
use std::time::Duration;
use stream::{StreamContext, read_stdout};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    sync::{mpsc, watch},
    time::timeout,
};
use uuid::Uuid;

pub(crate) struct SteerInput {
    pub(crate) message_id: Uuid,
    pub(crate) prompt: String,
}

pub(crate) enum RunInput {
    Steer(SteerInput),
    UserAsk(UserAskInput),
    Approval {
        request_id: Uuid,
        option: Option<usize>,
    },
}

pub(crate) async fn run_harness(
    mut request: StartRun,
    cwd: std::path::PathBuf,
    cancel: watch::Receiver<bool>,
    input: mpsc::UnboundedReceiver<RunInput>,
    user_asks: PendingUserAsks,
    emitter: Emitter,
) -> (RunStatus, Option<i32>) {
    let prepared = harness::restore_session_settings(&mut request)
        .and_then(|()| harness::prepare(&request, &cwd));
    let (spec, decoder) = match prepared {
        Ok(prepared) => prepared,
        Err(message) => {
            emitter
                .send(Event::RunFailed {
                    run_id: request.run_id,
                    code: ErrorCode::LaunchFailed,
                    message,
                })
                .await;
            return (RunStatus::Failed, None);
        }
    };
    run_prepared_harness(request, spec, decoder, cancel, input, user_asks, emitter).await
}

async fn run_prepared_harness(
    request: StartRun,
    spec: LaunchSpec,
    decoder: Box<dyn LineDecoder>,
    mut cancel: watch::Receiver<bool>,
    input: mpsc::UnboundedReceiver<RunInput>,
    user_asks: PendingUserAsks,
    emitter: Emitter,
) -> (RunStatus, Option<i32>) {
    let harness = request.harness;
    let mut command = process_command(&spec, &request.environment);

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
            return (RunStatus::Failed, None);
        }
    };
    let pid = child.id().unwrap_or_default();
    emitter
        .send(Event::RunStarted {
            run_id: request.run_id,
            pid,
        })
        .await;

    let mut stdin = child.stdin.take().expect("piped harness stdin");
    if stdin.write_all(spec.stdin.as_bytes()).await.is_err() {
        let _ = process_tree::terminate(&mut child, pid).await;
        emitter
            .send(Event::RunFailed {
                run_id: request.run_id,
                code: ErrorCode::LaunchFailed,
                message: format!("无法向 {harness} 发送 Prompt。"),
            })
            .await;
        return (RunStatus::Failed, None);
    }

    // Command Code reads the prompt to EOF in print mode.
    let stdin = if harness == HarnessKind::CommandCode {
        drop(stdin);
        None
    } else {
        Some(stdin)
    };
    let stdout = child.stdout.take().expect("piped harness stdout");
    let stderr = child.stderr.take();
    let mut stdout_task = tokio::spawn(read_stdout(
        stdout,
        stdin,
        decoder,
        input,
        StreamContext {
            run_id: request.run_id,
            harness,
            user_asks: user_asks.clone(),
            cancel: cancel.clone(),
            emitter: emitter.clone(),
        },
    ));
    let mut stderr_task = tokio::spawn(async move {
        if let Some(stderr) = stderr {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(_)) = lines.next_line().await {}
        }
    });

    let (output, was_cancelled) = tokio::select! {
        result = &mut stdout_task => (result.unwrap_or_default(), false),
        _ = cancel.changed() => {
            let _ = process_tree::cancel(&mut child, pid).await;
            let output = timeout(Duration::from_secs(3), &mut stdout_task).await
                .ok().and_then(Result::ok).unwrap_or_default();
            stdout_task.abort();
            (output, true)
        }
    };
    let was_cancelled = was_cancelled || *cancel.borrow();
    let (ask_status, ask_message) = if was_cancelled {
        (UserAskStatus::Cancelled, None)
    } else if let Some(message) = &output.provider_error {
        (UserAskStatus::Failed, Some(message.clone()))
    } else {
        (UserAskStatus::Expired, None)
    };
    finish_user_asks(
        request.run_id,
        &user_asks,
        ask_status,
        ask_message,
        &emitter,
    )
    .await;
    // Interactive CLIs exit on stdin EOF. Bound cleanup after the terminal frame.
    let status = match timeout(Duration::from_secs(3), child.wait()).await {
        Ok(status) => status,
        Err(_) => process_tree::terminate(&mut child, pid).await,
    };
    if timeout(Duration::from_secs(3), &mut stderr_task)
        .await
        .is_err()
    {
        stderr_task.abort();
    }
    let exit_code = status.as_ref().ok().and_then(|status| status.code());
    let final_status = if was_cancelled {
        RunStatus::Cancelled
    } else if output.completed && output.provider_error.is_none() {
        RunStatus::Completed
    } else {
        emitter
            .send(Event::RunFailed {
                run_id: request.run_id,
                code: ErrorCode::UnexpectedExit,
                message: output.provider_error.unwrap_or_else(|| match exit_code {
                    Some(code) => {
                        format!("{harness} 异常退出（代码 {code}）。请检查登录状态或诊断日志。")
                    }
                    None => format!("{harness} 异常退出。请检查登录状态或诊断日志。"),
                }),
            })
            .await;
        RunStatus::Failed
    };
    (final_status, exit_code)
}
