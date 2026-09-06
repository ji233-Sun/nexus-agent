use crate::application::events::{Emitter, emit_decoded};
use nexus_domain::RunStatus;
use nexus_harness_core::{DecodedEvent, InputFrame, LineDecoder};
use nexus_protocol::{ErrorCode, Event, StartRun};
use std::{
    collections::{HashSet, VecDeque},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    process::{ChildStdin, ChildStdout, Command as ProcessCommand},
    sync::{mpsc, watch},
    time::timeout,
};
use uuid::Uuid;

use super::process_tree;

pub(crate) struct SteerInput {
    pub(crate) message_id: Uuid,
    pub(crate) prompt: String,
}

#[derive(Default)]
struct SessionOutput {
    completed: bool,
    provider_error: Option<String>,
}

pub(crate) async fn run_harness(
    request: StartRun,
    cwd: std::path::PathBuf,
    mut cancel: watch::Receiver<bool>,
    input: mpsc::UnboundedReceiver<SteerInput>,
    emitter: Emitter,
) -> (RunStatus, Option<i32>) {
    let harness = request.harness;
    let (spec, decoder) = super::harness::prepare(&request, &cwd);
    let mut command = ProcessCommand::new(&spec.executable);
    command
        .args(&spec.args)
        .envs(
            request
                .environment
                .iter()
                .map(|variable| (&variable.name, &variable.value)),
        )
        .current_dir(&spec.cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    process_tree::configure(&mut command);

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
        process_tree::terminate(&mut child, pid).await;
        let _ = child.wait().await;
        emitter
            .send(Event::RunFailed {
                run_id: request.run_id,
                code: ErrorCode::LaunchFailed,
                message: format!("无法向 {harness} 发送 Prompt。"),
            })
            .await;
        return (RunStatus::Failed, None);
    }

    let stdout = child.stdout.take().expect("piped harness stdout");
    let stderr = child.stderr.take();
    let mut stdout_task = tokio::spawn(read_stdout(
        stdout,
        stdin,
        request.clone(),
        decoder,
        input,
        cancel.clone(),
        emitter.clone(),
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
    // Interactive CLIs exit on stdin EOF. Bound cleanup after the terminal frame.
    let status = match timeout(Duration::from_secs(3), child.wait()).await {
        Ok(status) => status,
        Err(_) => {
            process_tree::terminate(&mut child, pid).await;
            child.wait().await
        }
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

async fn read_stdout(
    stdout: ChildStdout,
    mut stdin: ChildStdin,
    request: StartRun,
    mut decoder: Box<dyn LineDecoder>,
    mut input: mpsc::UnboundedReceiver<SteerInput>,
    cancel: watch::Receiver<bool>,
    emitter: Emitter,
) -> SessionOutput {
    let run_id = request.run_id;
    let harness = request.harness;
    let mut lines = BufReader::new(stdout).lines();
    let mut output = SessionOutput::default();
    let mut tools = HashSet::new();
    let mut queued = VecDeque::new();
    let mut awaiting_receipt = HashSet::new();
    let mut input_open = true;
    let mut session_started = false;
    'stream: loop {
        let line = tokio::select! {
            biased;
            message = input.recv(), if input_open => {
                if let Some(message) = message {
                    queued.push_back(message);
                } else {
                    input_open = false;
                }
                continue;
            }
            line = lines.next_line() => match line {
                Ok(Some(line)) => line,
                _ => break,
            }
        };
        let mut tool_completed = false;
        match decoder.decode_line(&line) {
            Ok(events) => {
                for event in events {
                    match &event {
                        DecodedEvent::WriteStdin(frame) => {
                            if write_frame(&mut stdin, frame).await.is_err() {
                                output.provider_error =
                                    Some(format!("无法向 {harness} 写入交互消息。"));
                                break 'stream;
                            }
                            continue;
                        }
                        DecodedEvent::SessionStarted(_) => session_started = true,
                        DecodedEvent::ToolStarted { id, .. } => {
                            tools.insert(id.clone());
                        }
                        DecodedEvent::ToolCompleted { id, .. } => {
                            tools.remove(id);
                            tool_completed = true;
                        }
                        DecodedEvent::InputAccepted(id) => {
                            if let Ok(message_id) = Uuid::parse_str(id)
                                && awaiting_receipt.remove(&message_id)
                            {
                                emitter
                                    .send(Event::RunInputAccepted { run_id, message_id })
                                    .await;
                            }
                            continue;
                        }
                        DecodedEvent::InputRejected { id, message } => {
                            if let Ok(message_id) = Uuid::parse_str(id)
                                && awaiting_receipt.remove(&message_id)
                            {
                                emitter
                                    .send(Event::RunInputRejected {
                                        run_id,
                                        message_id,
                                        message: message.clone(),
                                    })
                                    .await;
                            }
                            continue;
                        }
                        DecodedEvent::TurnCompleted => output.completed = true,
                        DecodedEvent::Error(message) => {
                            output.provider_error = Some(message.clone())
                        }
                        _ => {}
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
        if output.completed {
            break;
        }
        // Decode the entire frame first: Claude can start several tools in one frame.
        if session_started && tool_completed && tools.is_empty() && !*cancel.borrow() {
            while let Some(message) = queued.front() {
                let Some(frame) = decoder.steer(&message.message_id.to_string(), &message.prompt)
                else {
                    break;
                };
                awaiting_receipt.insert(message.message_id);
                queued.pop_front();
                if write_frame(&mut stdin, &frame).await.is_err() {
                    output.provider_error = Some(format!("无法向 {harness} 发送 Steer。"));
                    break 'stream;
                }
            }
        }
    }
    // Close the receiver before announcing exit so late requests are explicitly rejected.
    input.close();
    while let Ok(message) = input.try_recv() {
        queued.push_back(message);
    }
    for message in queued {
        emitter
            .send(Event::RunInputRejected {
                run_id,
                message_id: message.message_id,
                message: "当前轮次已结束，消息仍保留在队列中。".into(),
            })
            .await;
    }
    for message_id in awaiting_receipt {
        let message = "Steer 送达结果未确认，已暂停队列；请检查会话后再重试。".to_owned();
        output.provider_error = Some(message.clone());
        emitter
            .send(Event::RunInputRejected {
                run_id,
                message_id,
                message,
            })
            .await;
    }
    output
}

async fn write_frame(stdin: &mut ChildStdin, frame: &InputFrame) -> std::io::Result<()> {
    let mut encoded = frame.0.to_string();
    encoded.push('\n');
    stdin.write_all(encoded.as_bytes()).await?;
    stdin.flush().await
}
