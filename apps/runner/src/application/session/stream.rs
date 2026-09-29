use super::{Emitter, PendingUserAsks, RunInput};
use crate::application::events::emit_decoded;
use nexus_domain::{RunStatus, UserAskStatus};
use nexus_harness_core::{ApprovalPrompt, DecodedEvent, InputFrame, LineDecoder};
use nexus_protocol::{ApprovalRequest, Event};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    process::{ChildStdin, ChildStdout},
    sync::{mpsc, watch},
    time::{Instant, sleep_until, timeout},
};
use uuid::Uuid;

const USER_ASK_WRITE_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) struct StreamContext {
    pub(super) run_id: Uuid,
    pub(super) harness: nexus_domain::HarnessKind,
    pub(super) user_asks: PendingUserAsks,
    pub(super) cancel: watch::Receiver<bool>,
    pub(super) emitter: Emitter,
}

struct PendingApproval {
    prompt: ApprovalPrompt,
    deadline: Option<Instant>,
}

#[derive(Default)]
pub(super) struct SessionOutput {
    pub(super) completed: bool,
    pub(super) provider_error: Option<String>,
}

pub(super) async fn read_stdout(
    stdout: ChildStdout,
    mut stdin: Option<ChildStdin>,
    mut decoder: Box<dyn LineDecoder>,
    mut input: mpsc::UnboundedReceiver<RunInput>,
    context: StreamContext,
) -> SessionOutput {
    let StreamContext {
        run_id,
        harness,
        user_asks,
        mut cancel,
        emitter,
    } = context;
    let mut lines = BufReader::new(stdout).lines();
    let mut output = SessionOutput::default();
    let mut tools = HashSet::new();
    let mut queued = VecDeque::new();
    let mut awaiting_receipt = HashSet::new();
    let mut input_open = true;
    let mut session_started = false;
    let mut approvals = HashMap::<Uuid, PendingApproval>::new();
    let mut ask_deadlines = HashMap::<Uuid, Instant>::new();
    let mut asks_resolved_on_send = HashSet::new();
    'stream: loop {
        let deadline = approvals
            .values()
            .filter_map(|pending| pending.deadline)
            .chain(ask_deadlines.values().copied())
            .min();
        let line = tokio::select! {
            biased;
            _ = cancel.changed() => break,
            _ = sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
                let expired: Vec<_> = approvals.iter().filter_map(|(id, pending)|
                    pending.deadline.is_some_and(|deadline| deadline <= Instant::now()).then_some(*id)).collect();
                for request_id in expired {
                    let pending = approvals.remove(&request_id).unwrap();
                    if write_frame(&mut stdin, &pending.prompt.cancel).await.is_err() {
                        output.provider_error = Some(format!("无法向 {harness} 写入审批回复。"));
                        break 'stream;
                    }
                    emitter.send(Event::RunApprovalResolved { run_id, request_id }).await;
                }
                let expired: Vec<_> = ask_deadlines.iter().filter_map(|(id, deadline)|
                    (*deadline <= Instant::now()).then_some(*id)).collect();
                for request_id in expired {
                    ask_deadlines.remove(&request_id);
                    asks_resolved_on_send.remove(&request_id);
                    if user_asks.finish(request_id) {
                        emitter.send(Event::RunUserAskFinished {
                            run_id, request_id, status: UserAskStatus::Expired, message: None,
                        }).await;
                    }
                }
                continue;
            }
            message = input.recv(), if input_open => {
                match message {
                    Some(RunInput::Steer(message)) => queued.push_back(message),
                    Some(RunInput::UserAsk(answer)) => {
                        if !user_asks.is_answer_queued(answer.request_id) {
                            continue;
                        }
                        let Some(frame) = decoder
                            .answer_user_ask(&answer.native_request_id, &answer.answers)
                        else {
                            let message = format!("{harness} 无法编码 User Ask 回答。");
                            if user_asks.finish(answer.request_id) {
                                emitter
                                    .send(Event::RunUserAskFinished {
                                        run_id,
                                        request_id: answer.request_id,
                                        status: UserAskStatus::Failed,
                                        message: Some(message.clone()),
                                    })
                                    .await;
                            }
                            output.provider_error = Some(message);
                            break 'stream;
                        };
                        if *cancel.borrow() || !user_asks.is_answer_queued(answer.request_id) {
                            continue;
                        }
                        if !matches!(
                            timeout(USER_ASK_WRITE_TIMEOUT, write_frame(&mut stdin, &frame)).await,
                            Ok(Ok(()))
                        ) {
                            let message = format!("无法向 {harness} 写入 User Ask 回答。");
                            if user_asks.finish(answer.request_id) {
                                emitter
                                    .send(Event::RunUserAskFinished {
                                        run_id,
                                        request_id: answer.request_id,
                                        status: UserAskStatus::Failed,
                                        message: Some(message.clone()),
                                    })
                                    .await;
                            }
                            output.provider_error = Some(message);
                            break 'stream;
                        }
                        if user_asks.mark_answer_sent(answer.request_id) {
                            ask_deadlines.remove(&answer.request_id);
                            emitter
                                .send(Event::RunUserAskAnswerSent {
                                    run_id,
                                    request_id: answer.request_id,
                                })
                                .await;
                            if asks_resolved_on_send.remove(&answer.request_id)
                                && user_asks.finish(answer.request_id)
                            {
                                emitter.send(Event::RunUserAskFinished {
                                    run_id,
                                    request_id: answer.request_id,
                                    status: UserAskStatus::Answered,
                                    message: None,
                                }).await;
                            }
                        }
                    }
                    Some(RunInput::Approval { request_id, option }) => {
                        let response = approvals.get(&request_id).and_then(|pending| {
                            if *cancel.borrow() { return None; }
                            match option {
                                Some(index) => pending.prompt.options.get(index).map(|option| &option.response),
                                None => Some(&pending.prompt.cancel),
                            }
                        });
                        if let Some(response) = response {
                            if write_frame(&mut stdin, response).await.is_err() {
                                output.provider_error = Some(format!("无法向 {harness} 写入审批回复。"));
                                break 'stream;
                            }
                            approvals.remove(&request_id);
                            emitter.send(Event::RunApprovalResolved { run_id, request_id }).await;
                        } else {
                            emitter.send(Event::RunApprovalRejected { run_id, request_id,
                                message: "审批请求已失效或选项无效。".into() }).await;
                        }
                    }
                    None => input_open = false,
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
                        DecodedEvent::ApprovalRequested(prompt) => {
                            if *cancel.borrow() {
                                break 'stream;
                            }
                            // A replay cannot create a second actionable copy of one native request.
                            if approvals
                                .values()
                                .any(|pending| pending.prompt.id == prompt.id)
                            {
                                continue;
                            }
                            let request_id = Uuid::new_v4();
                            let request = ApprovalRequest {
                                request_id,
                                title: prompt.title.clone(),
                                details: prompt.details.clone(),
                                options: prompt
                                    .options
                                    .iter()
                                    .map(|option| option.label.clone())
                                    .collect(),
                            };
                            approvals.insert(
                                request_id,
                                PendingApproval {
                                    prompt: prompt.clone(),
                                    deadline: prompt.timeout_ms.and_then(|ms| {
                                        Instant::now().checked_add(Duration::from_millis(ms))
                                    }),
                                },
                            );
                            emitter
                                .send(Event::RunApprovalRequested { run_id, request })
                                .await;
                            continue;
                        }
                        DecodedEvent::ApprovalResolved(id) => {
                            if let Some(request_id) =
                                approvals.iter().find_map(|(request_id, pending)| {
                                    (&pending.prompt.id == id).then_some(*request_id)
                                })
                            {
                                approvals.remove(&request_id);
                                emitter
                                    .send(Event::RunApprovalResolved { run_id, request_id })
                                    .await;
                            }
                            continue;
                        }
                        DecodedEvent::WriteStdin(frame) => {
                            if write_frame(&mut stdin, frame).await.is_err() {
                                output.provider_error =
                                    Some(format!("无法向 {harness} 写入交互消息。"));
                                break 'stream;
                            }
                            continue;
                        }
                        DecodedEvent::UserAskRequested(request) => {
                            let questions = request.questions.clone();
                            match user_asks
                                .register(request.native_request_id.clone(), questions.clone())
                            {
                                Ok(request_id) => {
                                    if let Some(deadline) = request.timeout_ms.and_then(|ms| {
                                        Instant::now().checked_add(Duration::from_millis(ms))
                                    }) {
                                        ask_deadlines.insert(request_id, deadline);
                                    }
                                    if request.resolve_on_send {
                                        asks_resolved_on_send.insert(request_id);
                                    }
                                    emitter
                                        .send(Event::RunUserAskRequested {
                                            run_id,
                                            request_id,
                                            questions,
                                        })
                                        .await;
                                }
                                Err(message) => {
                                    output.provider_error = Some(message);
                                    break 'stream;
                                }
                            }
                            continue;
                        }
                        DecodedEvent::UserAskFinished {
                            native_request_id,
                            status,
                            message,
                        } => {
                            if let Some(request_id) =
                                user_asks.finish_native(native_request_id, *status)
                            {
                                ask_deadlines.remove(&request_id);
                                asks_resolved_on_send.remove(&request_id);
                                emitter
                                    .send(Event::RunUserAskFinished {
                                        run_id,
                                        request_id,
                                        status: *status,
                                        message: message.clone(),
                                    })
                                    .await;
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
        match message {
            RunInput::Steer(message) => queued.push_back(message),
            RunInput::UserAsk(_) => {}
            RunInput::Approval { request_id, .. } => {
                emitter
                    .send(Event::RunApprovalRejected {
                        run_id,
                        request_id,
                        message: "当前轮次已结束，审批请求已失效。".into(),
                    })
                    .await;
            }
        }
    }
    for request_id in approvals.into_keys() {
        emitter
            .send(Event::RunApprovalResolved { run_id, request_id })
            .await;
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

async fn write_frame(stdin: &mut Option<ChildStdin>, frame: &InputFrame) -> std::io::Result<()> {
    let stdin = stdin.as_mut().ok_or(std::io::ErrorKind::BrokenPipe)?;
    let mut encoded = frame.0.to_string();
    encoded.push('\n');
    stdin.write_all(encoded.as_bytes()).await?;
    stdin.flush().await
}
