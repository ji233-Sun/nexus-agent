use super::{Presenter, executable_setting_key};
use crate::i18n::{Language, LocalizedText, probe_status};
use crate::infrastructure::git;
use crate::infrastructure::storage::NewTaskRun;
use crate::model::workspace::{WorkspaceKind, WorkspaceStatus};
use crate::model::{
    ModelCatalogState, PendingUserAsk, QueuedMessage, ResolvedModelSelection,
    UserAskSubmissionState,
};
use nexus_domain::{
    HarnessKind, MessageKind, MessageRole, PermissionMode, RunStatus, ToolMetadata, UserAskAnswer,
    UserAskAnswerMode, UserAskAnswerValue, UserAskQuestion, UserAskStatus, compact_task_title,
};
use nexus_protocol::{Command, CommandEnvelope, Event, StartRun};
use std::time::Instant;
use uuid::Uuid;

impl Presenter {
    pub(crate) fn refresh_run_elapsed(&mut self, now: Instant) -> bool {
        let elapsed = self
            .model
            .active_run_started_at
            .map(|started| now.saturating_duration_since(started).as_secs());
        if self.model.active_run_elapsed_seconds == elapsed {
            return false;
        }
        self.model.active_run_elapsed_seconds = elapsed;
        true
    }

    pub(super) fn handle_event(&mut self, event: Event) {
        let refresh_tasks = matches!(
            event,
            Event::TaskTitleGenerated { .. }
                | Event::RunExited { .. }
                | Event::RunStarted { .. }
                | Event::RunStatusChanged { .. }
        );
        let selected = self.model.conversation.id;
        let target = self
            .model
            .all_conversations()
            .find(|conversation| match &event {
                Event::ModelCatalogLoaded { request_id, .. }
                | Event::ModelCatalogFailed { request_id, .. } => {
                    conversation.model_catalog.accepts(*request_id)
                        || conversation.title_model_catalog.accepts(*request_id)
                }
                Event::RunStarted { run_id, .. }
                | Event::RunSessionStarted { run_id, .. }
                | Event::RunOutputDelta { run_id, .. }
                | Event::RunMessageCompleted { run_id, .. }
                | Event::RunApprovalRequested { run_id, .. }
                | Event::RunApprovalResolved { run_id, .. }
                | Event::RunApprovalRejected { run_id, .. }
                | Event::RunInputAccepted { run_id, .. }
                | Event::RunInputRejected { run_id, .. }
                | Event::RunUserAskRequested { run_id, .. }
                | Event::RunUserAskAnswerRejected { run_id, .. }
                | Event::RunUserAskAnswerSent { run_id, .. }
                | Event::RunUserAskFinished { run_id, .. }
                | Event::RunToolStarted { run_id, .. }
                | Event::RunToolCompleted { run_id, .. }
                | Event::RunStatusChanged { run_id, .. }
                | Event::RunFailed { run_id, .. }
                | Event::RunExited { run_id, .. } => conversation.active_run == Some(*run_id),
                _ => false,
            })
            .map(|conversation| conversation.id);
        if let Some(target) = target {
            self.model.activate_conversation(target);
        }
        // Route through the owning task's state, then restore the visible task before
        // notifying the view. Queue continuation therefore uses its own configuration.
        self.handle_conversation_event(event);
        self.model.activate_conversation(selected);
        if refresh_tasks {
            self.reload_tasks();
        }
    }

    fn handle_conversation_event(&mut self, event: Event) {
        match event {
            Event::RunnerReady => {
                self.model.status = LocalizedText::new(
                    "Runner 已连接，正在探测 {0}…",
                    &[("0", (self.model.selected_harness).to_string())],
                )
            }
            Event::HarnessDetected(probe) => {
                let harness = probe.harness;
                let available = probe.available;
                let message = probe_status(&probe);
                let history_executable =
                    (harness == HarnessKind::Codex).then(|| probe.executable.clone());
                self.model.harnesses.insert(harness, probe);
                if harness == self.model.selected_harness {
                    if self.model.active_run.is_none() {
                        if !available {
                            self.model.model_catalog = ModelCatalogState::NotReady(message.clone());
                        } else if matches!(self.model.model_catalog, ModelCatalogState::NotReady(_))
                        {
                            self.refresh_model_catalog();
                        }
                    }
                    self.model.status = message;
                }
                if let Some(executable) = history_executable {
                    self.connect_codex_history(executable);
                }
            }
            Event::ModelCatalogLoaded {
                request_id,
                harness,
                models,
            } if harness == self.model.title_generation.harness
                && self.model.title_model_catalog.accepts(request_id) =>
            {
                self.model.title_model_catalog = if models.is_empty() {
                    ModelCatalogState::Empty
                } else {
                    ModelCatalogState::Ready(models)
                };
                self.normalize_title_effort();
            }
            Event::ModelCatalogFailed {
                request_id,
                harness,
                message,
            } if harness == self.model.title_generation.harness
                && self.model.title_model_catalog.accepts(request_id) =>
            {
                self.model.title_model_catalog.fail(message.into());
            }
            Event::ModelCatalogLoaded {
                request_id,
                harness,
                models,
            } if harness == self.model.selected_harness
                && self.model.active_run.is_none()
                && self.model.model_catalog.accepts(request_id) =>
            {
                let count = models.len();
                self.model.model_catalog = if models.is_empty() {
                    ModelCatalogState::Empty
                } else {
                    ModelCatalogState::Ready(models)
                };
                if self.model.model_override.is_some()
                    && self.model.selected_catalog_model().is_some()
                {
                    self.remember_model_name();
                }
                let selected_unavailable = self.model.model_override_is_unavailable();
                let effort_reset = !selected_unavailable && self.normalize_catalog_effort();
                let profile_model_unverified = self.model.model_override.is_none()
                    && self
                        .model
                        .selected_provider_profile()
                        .and_then(|profile| profile.model.as_deref())
                        .is_some()
                    && self.model.selected_catalog_model().is_none();
                self.model.status = if selected_unavailable {
                    LocalizedText::new(
                        "当前 {harness} 模型 {0} 不可用，请重新选择或跟随默认。",
                        &[
                            ("harness", (harness).to_string()),
                            (
                                "0",
                                (self.model.model_override.as_deref().unwrap_or_default())
                                    .to_string(),
                            ),
                        ],
                    )
                } else if effort_reset {
                    "当前模型不支持原 effort，已恢复为模型默认。".into()
                } else if profile_model_unverified {
                    "Profile 默认模型不在当前目录中；仍可使用，但尚未验证可用。".into()
                } else if count == 0 {
                    LocalizedText::new(
                        "{harness} 模型目录为空；仍可跟随 CLI 默认。",
                        &[("harness", (harness).to_string())],
                    )
                } else {
                    LocalizedText::new(
                        "已加载 {count} 个 {harness} 模型。",
                        &[
                            ("count", (count).to_string()),
                            ("harness", (harness).to_string()),
                        ],
                    )
                };
            }
            Event::ModelCatalogFailed {
                request_id,
                harness,
                message,
            } if harness == self.model.selected_harness
                && self.model.active_run.is_none()
                && self.model.model_catalog.accepts(request_id) =>
            {
                self.model.model_catalog.fail(message.clone().into());
                self.model.status = LocalizedText::new(
                    "{harness} 模型目录加载失败：{message}",
                    &[
                        ("harness", (harness).to_string()),
                        ("message", (message).to_string()),
                    ],
                );
            }
            Event::TaskTitleGenerated { task_id, title } => {
                if let Some(title) = compact_task_title(&title)
                    && self
                        .storage
                        .update_task_title(task_id, &title)
                        .unwrap_or(false)
                {
                    self.reload_tasks();
                }
            }
            Event::RunStarted { run_id, .. } if self.model.active_run == Some(run_id) => {
                let harness = self
                    .model
                    .active_harness
                    .unwrap_or(self.model.selected_harness);
                self.model.status = LocalizedText::new(
                    "{harness} 正在执行…",
                    &[("harness", (harness).to_string())],
                );
                let _ = self.storage.update_run_status(run_id, RunStatus::Running);
            }
            Event::RunSessionStarted { run_id, session_id }
                if self.model.active_run == Some(run_id) && !session_id.is_empty() =>
            {
                if let Err(error) = self.storage.save_run_session(run_id, &session_id) {
                    self.model.status = LocalizedText::new(
                        "无法保存会话，后续可能无法续聊：{error}",
                        &[("error", (error).to_string())],
                    );
                }
            }
            Event::RunOutputDelta { run_id, text } if self.model.active_run == Some(run_id) => {
                self.model.streaming_text.push_str(&text);
            }
            Event::RunApprovalRequested { run_id, request }
                if self.model.active_run == Some(run_id) && !self.model.run_cancelling =>
            {
                if !self
                    .model
                    .pending_approvals
                    .iter()
                    .any(|pending| pending.request_id == request.request_id)
                {
                    self.model.pending_approvals.push_back(request);
                }
                self.model.status = "等待授权，请在桌面弹窗中处理。".into();
            }
            Event::RunApprovalResolved { run_id, request_id }
                if self.model.active_run == Some(run_id) =>
            {
                self.model
                    .pending_approvals
                    .retain(|request| request.request_id != request_id);
                if self.model.responding_approval == Some(request_id) {
                    self.model.responding_approval = None;
                }
                self.model.status = if self.model.pending_approvals.is_empty() {
                    "审批已处理，等待 Agent 继续…".into()
                } else {
                    "等待授权，请在桌面弹窗中处理。".into()
                };
            }
            Event::RunApprovalRejected {
                run_id,
                request_id,
                message,
            } if self.model.active_run == Some(run_id) => {
                if self.model.responding_approval == Some(request_id) {
                    self.model.responding_approval = None;
                }
                self.model.status = message.into();
            }
            Event::RunInputAccepted { run_id, message_id }
                if self.model.active_run == Some(run_id)
                    && self.model.steering_message == Some(message_id) =>
            {
                self.model.steering_message = None;
                if let Some(index) = self
                    .model
                    .queued_messages
                    .iter()
                    .position(|message| message.id == message_id)
                    && let Some(message) = self.model.queued_messages.remove(index)
                {
                    self.persist_live_message(
                        run_id,
                        MessageRole::User,
                        MessageKind::Text,
                        &message.prompt,
                        None,
                    );
                    self.model.status = "Steer 已送达当前轮次。".into();
                }
            }
            Event::RunInputRejected {
                run_id,
                message_id,
                message,
            } if self.model.active_run == Some(run_id)
                && self.model.steering_message == Some(message_id) =>
            {
                self.model.steering_message = None;
                self.model.status = message.into();
            }
            Event::RunUserAskRequested {
                run_id,
                request_id,
                questions,
            } if self.model.active_run == Some(run_id)
                && !self.model.run_cancelling
                && !self
                    .model
                    .pending_user_asks
                    .iter()
                    .any(|request| request.request_id == request_id) =>
            {
                let history = format_user_ask_request(self.model.language, &questions);
                self.persist_live_message(
                    run_id,
                    MessageRole::System,
                    MessageKind::Status,
                    &history,
                    None,
                );
                self.model
                    .pending_user_asks
                    .push(PendingUserAsk::new(request_id, questions));
                self.model.status = "Agent 正在等待你的回答。".into();
            }
            Event::RunUserAskAnswerRejected {
                run_id,
                request_id,
                message,
            } if self.model.active_run == Some(run_id) => {
                if let Some(request) = self.model.pending_user_asks.iter_mut().find(|request| {
                    request.request_id == request_id
                        && request.submission == UserAskSubmissionState::Submitting
                }) {
                    request.submission = UserAskSubmissionState::Pending;
                    request.error = Some(message.clone());
                    request.submitted_answers = None;
                    self.model.status = message.into();
                }
            }
            Event::RunUserAskAnswerSent { run_id, request_id }
                if self.model.active_run == Some(run_id) =>
            {
                if let Some(request) = self.model.pending_user_asks.iter_mut().find(|request| {
                    request.request_id == request_id
                        && request.submission == UserAskSubmissionState::Submitting
                }) {
                    request.submission = UserAskSubmissionState::Sent;
                    request.error = None;
                    self.model.status = "回答已发送，等待 Agent 继续…".into();
                }
            }
            Event::RunUserAskFinished {
                run_id,
                request_id,
                status,
                message,
            } if self.model.active_run == Some(run_id) => {
                let request = self
                    .model
                    .pending_user_asks
                    .iter()
                    .position(|request| request.request_id == request_id)
                    .map(|index| self.model.pending_user_asks.remove(index));
                if let Some(request) = request {
                    let history = format_user_ask_result(
                        self.model.language,
                        &request,
                        status,
                        message.as_deref(),
                    );
                    self.persist_live_message(
                        run_id,
                        MessageRole::System,
                        MessageKind::Status,
                        &history,
                        None,
                    );
                    self.model.status = if self.model.pending_user_asks.is_empty() {
                        message
                            .unwrap_or_else(|| {
                                user_ask_status_text(self.model.language, status).into()
                            })
                            .into()
                    } else {
                        "Agent 正在等待你的回答。".into()
                    };
                }
            }
            Event::RunMessageCompleted { run_id, text }
                if self.model.active_run == Some(run_id) =>
            {
                self.model.streaming_text.clear();
                self.persist_live_message(
                    run_id,
                    MessageRole::Assistant,
                    MessageKind::Text,
                    &text,
                    None,
                );
            }
            Event::RunToolStarted {
                run_id,
                tool_id,
                name,
                summary,
            } if self.model.active_run == Some(run_id) => {
                let content = if summary.is_empty() {
                    name
                } else {
                    format!("{name}\n{summary}")
                };
                self.persist_live_message(
                    run_id,
                    MessageRole::Tool,
                    MessageKind::ToolCall,
                    &content,
                    Some(ToolMetadata {
                        id: tool_id,
                        is_error: false,
                    }),
                );
            }
            Event::RunToolCompleted {
                run_id,
                tool_id,
                output,
                is_error,
            } if self.model.active_run == Some(run_id) => {
                let content = if is_error {
                    format!("工具执行失败\n{output}")
                } else {
                    output
                };
                self.persist_live_message(
                    run_id,
                    MessageRole::Tool,
                    MessageKind::ToolResult,
                    &content,
                    Some(ToolMetadata {
                        id: tool_id,
                        is_error,
                    }),
                );
            }
            Event::RunStatusChanged {
                run_id,
                status,
                message,
            } if self.model.active_run == Some(run_id) => {
                let _ = self.storage.update_run_status(run_id, status);
                if let Some(message) = message {
                    self.model.status = message.into();
                }
            }
            Event::RunFailed {
                run_id, message, ..
            } if self.model.active_run == Some(run_id) => {
                self.model.status = message.clone().into();
                self.persist_live_message(
                    run_id,
                    MessageRole::System,
                    MessageKind::Error,
                    &message,
                    None,
                );
            }
            Event::RunExited {
                run_id,
                status,
                exit_code,
            } if self.model.active_run == Some(run_id) => {
                let pending_catalog_request = match self.model.model_catalog {
                    ModelCatalogState::Loading { request_id, .. } => Some(request_id),
                    _ => None,
                };
                let task_id = self.model.active_task;
                let cancelled = self.model.run_cancelling;
                let stale_user_asks = std::mem::take(&mut self.model.pending_user_asks);
                let stale_status = if cancelled || status == RunStatus::Cancelled {
                    UserAskStatus::Cancelled
                } else {
                    UserAskStatus::Expired
                };
                for request in stale_user_asks {
                    let history =
                        format_user_ask_result(self.model.language, &request, stale_status, None);
                    self.persist_live_message(
                        run_id,
                        MessageRole::System,
                        MessageKind::Status,
                        &history,
                        None,
                    );
                }
                let _ = self.storage.finish_run(run_id, status, exit_code);
                self.model.streaming_text.clear();
                self.model.active_run = None;
                self.model.active_checkout = None;
                self.model.run_cancelling = false;
                self.model.steering_message = None;
                self.model.active_run_started_at = None;
                self.model.active_run_elapsed_seconds = None;
                self.model.active_task = None;
                self.model.active_harness = None;
                self.model.active_permission_mode = None;
                self.model.pending_approvals.clear();
                self.model.responding_approval = None;
                self.model.status = match status {
                    RunStatus::Completed => "任务已完成".into(),
                    RunStatus::Cancelled => "任务已取消".into(),
                    RunStatus::Failed => "任务执行失败".into(),
                    _ => LocalizedText::new(
                        "任务状态：{status}",
                        &[("status", (status).to_string())],
                    ),
                };
                self.reload_tasks();
                self.refresh_workspace_branch();
                if self.model.selected_task != task_id
                    && let Some(selected_task) = self.model.selected_task
                {
                    self.select_task(selected_task);
                }
                if status == RunStatus::Completed
                    && !cancelled
                    && let Some(message) = self
                        .model
                        .queued_messages
                        .iter()
                        .find(|message| Some(message.task_id) == task_id)
                {
                    self.send_queued_message(message.id);
                }
                if self.model.active_run.is_none()
                    && (pending_catalog_request
                        .is_some_and(|request_id| self.model.model_catalog.accepts(request_id))
                        || self.model.catalog_project
                            != self
                                .model
                                .selected_project
                                .as_ref()
                                .map(|project| project.id))
                {
                    // Retry an ignored response only if task restoration has not replaced the request.
                    let status = self.model.status.clone();
                    self.refresh_model_catalog();
                    self.model.status = status;
                }
            }
            _ => {}
        }
        if self
            .model
            .pending_workspace_start
            .as_ref()
            .is_some_and(|pending| pending.context_id == self.model.conversation.id)
            && !self.model.workspace_retry
            && self
                .model
                .selected_workspace
                .as_ref()
                .is_some_and(|workspace| workspace.status == WorkspaceStatus::Ready)
            && !matches!(
                self.model.model_catalog,
                ModelCatalogState::Loading { .. } | ModelCatalogState::Idle
            )
            && self.model.active_run.is_none()
        {
            self.retry_workspace_start();
        }
    }

    fn persist_live_message(
        &mut self,
        run_id: Uuid,
        role: MessageRole,
        kind: MessageKind,
        content: &str,
        tool: Option<ToolMetadata>,
    ) {
        let Some(task_id) = self.model.active_task else {
            return;
        };
        if let Ok(message) = self
            .storage
            .append_message(task_id, run_id, role, kind, content, tool)
            && self.model.selected_task == Some(task_id)
        {
            self.model.messages.push(message);
        }
    }

    pub(crate) fn submit(&mut self, prompt: &str, configured_executable: &str) -> bool {
        if self.model.active_run.is_some() {
            if !self.model.can_queue() || prompt.trim().is_empty() {
                return false;
            }
            self.model
                .conversation
                .queued_messages
                .push_back(QueuedMessage {
                    id: Uuid::new_v4(),
                    task_id: self.model.conversation.active_task.unwrap(),
                    prompt: prompt.trim().to_owned(),
                    permission_mode: self.model.conversation.permission_mode,
                });
            self.model.status = "消息已排队，将在当前轮次结束后依次发送。".into();
            return true;
        }
        self.start_run(
            self.model.selected_task,
            prompt,
            configured_executable,
            self.model.permission_mode,
        )
    }

    pub(crate) fn send_queued_message(&mut self, message_id: Uuid) -> bool {
        let Some(message) = self
            .model
            .queued_messages
            .iter()
            .find(|message| message.id == message_id)
            .cloned()
        else {
            return false;
        };
        if self.model.active_run.is_some() || self.model.selected_task != Some(message.task_id) {
            return false;
        }
        let executable = self.model.executable.clone();
        if !self.start_run(
            Some(message.task_id),
            &message.prompt,
            &executable,
            message.permission_mode,
        ) {
            return false;
        }
        self.model
            .queued_messages
            .retain(|queued| queued.id != message_id);
        true
    }

    pub(crate) fn remove_queued_message(&mut self, message_id: Uuid) {
        if self.model.steering_message == Some(message_id) {
            return;
        }
        self.model
            .queued_messages
            .retain(|message| message.id != message_id);
    }

    pub(crate) fn steer_queued_message(&mut self, message_id: Uuid) -> bool {
        if !self.model.can_queue() || self.model.steering_message.is_some() {
            return false;
        }
        let Some(index) = self.model.queued_messages.iter().position(|message| {
            message.id == message_id && Some(message.task_id) == self.model.active_task
        }) else {
            return false;
        };
        let run_id = self.model.active_run.unwrap();
        if self.model.active_permission_mode
            != Some(self.model.queued_messages[index].permission_mode)
        {
            self.model.status = "排队消息的权限与当前轮次不同，请等待下一轮发送。".into();
            return false;
        }
        let command = CommandEnvelope::new(Command::RunSteer {
            run_id,
            message_id,
            prompt: self.model.queued_messages[index].prompt.clone(),
        });
        if !self
            .runner
            .as_ref()
            .is_some_and(|runner| runner.send(command).is_ok())
        {
            self.model.status = "Runner 不可用，消息仍保留在队列中。".into();
            return false;
        }
        // A Steer that races with turn completion becomes the next queued message.
        let message = self.model.queued_messages.remove(index).unwrap();
        self.model.queued_messages.push_front(message);
        self.model.steering_message = Some(message_id);
        self.model.status = "等待工具执行结束后介入…".into();
        true
    }

    pub(crate) fn set_user_ask_question(&mut self, request_id: Uuid, index: usize) -> bool {
        let Some(request) = self
            .model
            .pending_user_asks
            .iter_mut()
            .find(|request| request.request_id == request_id)
        else {
            return false;
        };
        if index >= request.questions.len() {
            return false;
        }
        request.active_question = index;
        true
    }

    pub(crate) fn toggle_user_ask_collapsed(&mut self, request_id: Uuid) -> bool {
        let Some(request) = self
            .model
            .pending_user_asks
            .iter_mut()
            .find(|request| request.request_id == request_id)
        else {
            return false;
        };
        request.collapsed = !request.collapsed;
        true
    }

    pub(crate) fn set_user_ask_text(
        &mut self,
        request_id: Uuid,
        question_id: &str,
        text: String,
    ) -> bool {
        let Some(request) = self.model.pending_user_asks.iter_mut().find(|request| {
            request.request_id == request_id
                && request.submission == UserAskSubmissionState::Pending
        }) else {
            return false;
        };
        let Some(question) = request
            .questions
            .iter()
            .find(|question| question.id == question_id)
        else {
            return false;
        };
        match question.answer_mode {
            UserAskAnswerMode::Text => {
                request
                    .drafts
                    .insert(question.id.clone(), UserAskAnswerValue::Text(text));
            }
            UserAskAnswerMode::Choice {
                allow_custom: true, ..
            } => {
                if text.trim().is_empty()
                    && matches!(
                        request.drafts.get(question_id),
                        Some(UserAskAnswerValue::Selected(_))
                    )
                {
                    return true;
                }
                let value = if text.trim().is_empty() {
                    UserAskAnswerValue::Selected(Vec::new())
                } else {
                    UserAskAnswerValue::Text(text)
                };
                request.drafts.insert(question.id.clone(), value);
            }
            UserAskAnswerMode::Choice {
                allow_custom: false,
                ..
            } => return false,
        }
        request.error = None;
        true
    }

    pub(crate) fn set_user_ask_option(
        &mut self,
        request_id: Uuid,
        question_id: &str,
        option_id: &str,
        checked: bool,
    ) -> bool {
        let Some(request) = self.model.pending_user_asks.iter_mut().find(|request| {
            request.request_id == request_id
                && request.submission == UserAskSubmissionState::Pending
        }) else {
            return false;
        };
        let Some(question) = request
            .questions
            .iter()
            .find(|question| question.id == question_id)
        else {
            return false;
        };
        let UserAskAnswerMode::Choice { multiple, .. } = question.answer_mode else {
            return false;
        };
        if !question.options.iter().any(|option| option.id == option_id) {
            return false;
        }
        let selected = request
            .drafts
            .entry(question.id.clone())
            .or_insert_with(|| UserAskAnswerValue::Selected(Vec::new()));
        if !matches!(selected, UserAskAnswerValue::Selected(_)) {
            *selected = UserAskAnswerValue::Selected(Vec::new());
        }
        let UserAskAnswerValue::Selected(selected) = selected else {
            unreachable!()
        };
        if multiple {
            if checked && !selected.iter().any(|id| id == option_id) {
                selected.push(option_id.to_owned());
            } else if !checked {
                selected.retain(|id| id != option_id);
            }
        } else if checked {
            selected.clear();
            selected.push(option_id.to_owned());
        }
        request.error = None;
        true
    }

    pub(crate) fn can_submit_user_ask(&self, request_id: Uuid) -> bool {
        self.model.active_run.is_some()
            && !self.model.run_cancelling
            && self.model.pending_user_asks.iter().any(|request| {
                request.request_id == request_id
                    && request.submission == UserAskSubmissionState::Pending
                    && request.answers().is_some()
            })
    }

    pub(crate) fn submit_user_ask(&mut self, request_id: Uuid) -> bool {
        let Some(answers) = self
            .model
            .pending_user_asks
            .iter()
            .find(|request| request.request_id == request_id)
            .and_then(PendingUserAsk::answers)
        else {
            return false;
        };
        self.answer_user_ask(request_id, answers)
    }

    pub(crate) fn answer_user_ask(
        &mut self,
        request_id: Uuid,
        answers: Vec<UserAskAnswer>,
    ) -> bool {
        let Some(run_id) = self.model.active_run.filter(|_| !self.model.run_cancelling) else {
            return false;
        };
        let Some(index) = self.model.pending_user_asks.iter().position(|request| {
            request.request_id == request_id
                && request.submission == UserAskSubmissionState::Pending
        }) else {
            return false;
        };
        let submitted_answers = answers.clone();
        let result = self
            .runner
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Runner 不可用"))
            .and_then(|runner| runner.answer_user_ask(run_id, request_id, answers));
        match result {
            Ok(()) => {
                let request = &mut self.model.pending_user_asks[index];
                request.submission = UserAskSubmissionState::Submitting;
                request.error = None;
                request.submitted_answers = Some(submitted_answers);
                self.model.status = "正在提交回答…".into();
                true
            }
            Err(error) => {
                let message = format!("无法提交 User Ask 回答：{error}");
                self.model.pending_user_asks[index].error = Some(message.clone());
                self.model.status = message.into();
                false
            }
        }
    }

    pub(super) fn start_run(
        &mut self,
        task_id: Option<Uuid>,
        prompt: &str,
        configured_executable: &str,
        permission_mode: PermissionMode,
    ) -> bool {
        if self.model.updates.state.is_installing() {
            self.model.status = "正在安装应用更新，重启后可继续任务。".into();
            return false;
        }
        if self.model.active_run.is_some()
            || self.model.harness_manager.operating.is_some()
            || self.model.occupied_run_slots() >= 2
        {
            return false;
        }
        let Some(project) = self.model.selected_project.clone() else {
            self.model.status = "请先选择项目目录。".into();
            return false;
        };
        let prompt = prompt.trim().to_owned();
        if prompt.is_empty() {
            self.model.status = "Prompt 不能为空。".into();
            return false;
        }
        let configured_executable = configured_executable.trim().to_owned();
        if configured_executable.is_empty() {
            self.model.status = LocalizedText::new(
                "{0} 可执行文件不能为空。",
                &[("0", (self.model.selected_harness).to_string())],
            );
            return false;
        }
        let profile_ready = self
            .model
            .selected_provider_profile()
            .is_some_and(|profile| profile.credential_configured);
        let Some(probe) = self
            .model
            .selected_probe()
            .filter(|probe| probe.available && (probe.authenticated || profile_ready))
        else {
            self.model.status = LocalizedText::new(
                "{0} 尚未就绪，请先完成探测和登录。",
                &[("0", (self.model.selected_harness).to_string())],
            );
            return false;
        };
        let executable = probe.executable.clone();
        let harness_version = probe.version.clone();
        let harness = self.model.selected_harness;
        let session_id = if let Some(task_id) = task_id {
            let config = match self.storage.conversation_config(task_id) {
                Ok(Some(config)) => config,
                _ => {
                    self.model.status = "无法读取当前任务的会话配置。".into();
                    return false;
                }
            };
            if config.harness != harness {
                self.model.status = LocalizedText::new(
                    "当前会话使用 {0}，请切回该 Harness 继续对话，或新建任务。",
                    &[("0", (config.harness).to_string())],
                );
                return false;
            }
            let Some(session_id) = config.session_id.filter(|id| !id.is_empty()) else {
                self.model.status = "当前任务未保存可恢复的会话，无法继续对话；请新建任务。".into();
                return false;
            };
            if config.executable != executable {
                self.model.status =
                    "当前探测结果与任务保存的可执行文件不一致，请重新选择任务并完成探测。".into();
                return false;
            }
            Some(session_id)
        } else {
            None
        };
        if !self.model.catalog_selection_is_valid() {
            self.model.status = LocalizedText::new(
                "当前 {harness} 模型或 effort 未通过目录验证，请调整选择后重试。",
                &[("harness", (harness).to_string())],
            );
            return false;
        }
        let environment = match self.provider_launch_configuration(harness) {
            Ok(configuration) => configuration,
            Err(error) => {
                self.model.status = LocalizedText::new(
                    "无法读取 Provider Profile：{error}",
                    &[("error", (error).to_string())],
                );
                return false;
            }
        };
        let ResolvedModelSelection { model, effort } = self.model.resolved_model_selection();
        let title_generation = session_id
            .is_none()
            .then(|| self.title_generation_configuration().ok())
            .flatten();
        let title = compact_task_title(&prompt).unwrap_or_else(|| "新任务".into());
        if task_id.is_none()
            && self.model.selected_workspace.is_none()
            && self.model.workspace_draft.kind == WorkspaceKind::Worktree
        {
            return self.begin_worktree(&prompt, &configured_executable, permission_mode);
        }
        let workspace = if let Some(task_id) = task_id {
            self.storage.task_workspace(task_id)
        } else if let Some(workspace) = &self.model.selected_workspace {
            Ok(Some(workspace.clone()))
        } else {
            self.storage.workspace(project.id)
        };
        let (workspace, checkout) = match workspace.and_then(|workspace| {
            let workspace = workspace.ok_or_else(|| anyhow::anyhow!("任务缺少绑定目录"))?;
            git::validate_workspace(&workspace)?;
            let checkout = git::checkout_path(std::path::Path::new(&workspace.path))?;
            anyhow::ensure!(
                !self.model.workspace_locked(&checkout),
                "此目录正在执行 Worktree 操作，请等待完成"
            );
            anyhow::ensure!(
                !self.model.checkout_running(&checkout),
                "此 checkout 已有任务运行，请等待结束或使用独立 Worktree"
            );
            Ok((workspace, checkout))
        }) {
            Ok(workspace) => workspace,
            Err(error) => {
                self.model.status = error.to_string().into();
                return false;
            }
        };
        let Ok(pending_run) = self.storage.prepare_task_run(NewTaskRun {
            workspace_id: Some(workspace.id),
            task_id,
            project_id: project.id,
            title: &title,
            prompt: &prompt,
            harness,
            executable: &executable,
            model: model.as_deref(),
            effort,
            permission_mode,
            harness_version: harness_version.as_deref(),
        }) else {
            self.model.status = "无法保存任务运行。".into();
            return false;
        };
        let task_id = pending_run.task_id;
        let run_id = pending_run.run_id;
        let command = CommandEnvelope::new(Command::RunStart(StartRun {
            run_id,
            task_id,
            session_id,
            cwd: workspace.path.clone(),
            prompt: prompt.clone(),
            harness,
            executable: executable.clone(),
            model,
            effort,
            permission_mode,
            environment,
            title_generation,
        }));
        if let Some(runner) = &self.runner
            && runner.send(command).is_ok()
        {
            if pending_run.commit().is_err() {
                let _ = runner.send(CommandEnvelope::new(Command::RunCancel { run_id }));
                self.model.status = "无法保存任务运行，已请求停止 Runner。".into();
                return false;
            }
            self.model.active_run = Some(run_id);
            self.model.active_checkout = Some(checkout);
            self.model.active_run_started_at = Some(Instant::now());
            self.model.active_run_elapsed_seconds = Some(0);
            self.model.active_task = Some(task_id);
            self.model.active_harness = Some(harness);
            self.model.active_permission_mode = Some(permission_mode);
            self.model.selected_task = Some(task_id);
            self.model.selected_workspace = self
                .storage
                .task_workspace(task_id)
                .ok()
                .flatten()
                .or(Some(workspace));
            self.model.selected_codex_thread = None;
            self.model.codex_history_messages.clear();
            self.model.codex_thread_loading = false;
            self.model.messages = self.storage.messages(task_id).unwrap_or_default();
            self.model.status = LocalizedText::translated(|language| {
                let effort_label = match language {
                    Language::Chinese => effort.to_string(),
                    Language::English => language.effort(effort).to_owned(),
                };
                language.format(
                    "正在启动 {harness} · {effort}",
                    &[("harness", harness.to_string()), ("effort", effort_label)],
                )
            });
            let _ = self
                .storage
                .set_setting(executable_setting_key(harness), &configured_executable);

            self.reload_tasks();
            true
        } else {
            self.model.status = "Runner 不可用，任务未启动。".into();
            false
        }
    }

    pub(crate) fn cancel(&mut self) {
        let _ = self.request_cancel();
    }

    pub(crate) fn respond_approval(
        &mut self,
        run_id: Uuid,
        request_id: Uuid,
        option: Option<usize>,
    ) -> bool {
        if self.model.active_run != Some(run_id)
            || self.model.run_cancelling
            || self.model.responding_approval.is_some()
            || !self.model.pending_approvals.front().is_some_and(|request| {
                request.request_id == request_id
                    && option.is_none_or(|index| index < request.options.len())
            })
        {
            return false;
        }
        let command = CommandEnvelope::new(Command::RunApprovalRespond {
            run_id,
            request_id,
            option,
        });
        if !self
            .runner
            .as_ref()
            .is_some_and(|runner| runner.send(command).is_ok())
        {
            self.model.status = "审批回复发送失败，请重试或停止任务。".into();
            return false;
        }
        self.model.responding_approval = Some(request_id);
        self.model.status = "正在发送审批回复…".into();
        true
    }

    pub(super) fn request_cancel(&mut self) -> Result<(), String> {
        let Some(run_id) = self.model.active_run else {
            return Err("没有运行中的任务".into());
        };
        let runner = self
            .runner
            .as_ref()
            .ok_or_else(|| "Runner 不可用".to_owned())?;
        runner
            .send(CommandEnvelope::new(Command::RunCancel { run_id }))
            .map_err(|error| error.to_string())?;
        self.model.run_cancelling = true;
        self.model.pending_approvals.clear();
        self.model.responding_approval = None;
        let _ = self
            .storage
            .update_run_status(run_id, RunStatus::Cancelling);
        let harness = self
            .model
            .active_harness
            .unwrap_or(self.model.selected_harness);
        self.model.status =
            LocalizedText::new("正在停止 {harness}…", &[("harness", (harness).to_string())]);
        Ok(())
    }
}

fn format_user_ask_request(language: Language, questions: &[UserAskQuestion]) -> String {
    let mut lines = vec![language.text("Agent 提问").to_owned()];
    for (index, question) in questions.iter().enumerate() {
        lines.push(format!("{}. {}", index + 1, question.prompt));
        for option in &question.options {
            let mut line = format!("   - {}", option.label);
            if let Some(description) = option
                .description
                .as_deref()
                .filter(|text| !text.is_empty())
            {
                line.push_str(": ");
                line.push_str(description);
            }
            lines.push(line);
        }
    }
    lines.join("\n")
}

fn format_user_ask_result(
    language: Language,
    request: &PendingUserAsk,
    status: UserAskStatus,
    message: Option<&str>,
) -> String {
    let mut lines = vec![user_ask_status_text(language, status).to_owned()];
    for (index, question) in request.questions.iter().enumerate() {
        let answer = request
            .submitted_answers
            .as_deref()
            .and_then(|answers| {
                answers
                    .iter()
                    .find(|answer| answer.question_id == question.id)
            })
            .map(|answer| match &answer.value {
                UserAskAnswerValue::Text(text) => text.clone(),
                UserAskAnswerValue::Selected(option_ids) => option_ids
                    .iter()
                    .map(|option_id| {
                        question
                            .options
                            .iter()
                            .find(|option| option.id == *option_id)
                            .map(|option| option.label.clone())
                            .unwrap_or_else(|| option_id.clone())
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            })
            .unwrap_or_else(|| language.text("未记录回答").to_owned());
        lines.push(format!("{}. {}", index + 1, question.prompt));
        lines.push(format!("   {}", answer));
    }
    if let Some(message) = message.filter(|message| !message.trim().is_empty()) {
        lines.push(message.to_owned());
    }
    lines.join("\n")
}

fn user_ask_status_text(language: Language, status: UserAskStatus) -> &'static str {
    language.text(match status {
        UserAskStatus::Answered => "User Ask 已回答",
        UserAskStatus::Cancelled => "User Ask 已取消",
        UserAskStatus::Expired => "User Ask 已失效",
        UserAskStatus::Failed => "User Ask 回答失败",
    })
}
