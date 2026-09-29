//! Dispatch runner events to their owning conversation without changing UI selection.
use super::Presenter;
use super::runs::{format_user_ask_request, format_user_ask_result, user_ask_status_text};
use crate::{
    i18n::{LocalizedText, probe_status},
    model::{
        GenerationKind, ModelCatalogState, PendingUserAsk, UserAskSubmissionState,
        workspace::WorkspaceStatus,
    },
};
use nexus_domain::{
    MessageKind, MessageRole, RunStatus, ToolMetadata, UserAskStatus, compact_task_title,
};
use nexus_protocol::Event;
use uuid::Uuid;

impl Presenter {
    pub(super) fn handle_event(&mut self, event: Event) {
        let refresh_tasks = matches!(
            event,
            Event::TaskTitleGenerated { .. }
                | Event::RunExited { .. }
                | Event::RunStarted { .. }
                | Event::RunStatusChanged { .. }
        );
        let target = self
            .model
            .all_conversations()
            .find(|conversation| match &event {
                Event::ModelCatalogLoaded { request_id, .. }
                | Event::ModelCatalogFailed { request_id, .. } => {
                    conversation.model_catalog.accepts(*request_id)
                        || conversation.title_model_catalog.accepts(*request_id)
                        || conversation.commit_model_catalog.accepts(*request_id)
                }
                Event::CommitMessageGenerated { request_id, .. }
                | Event::CommitMessageFailed { request_id, .. } => {
                    conversation.commit_message_request == Some(*request_id)
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
        let context = target.unwrap_or(self.model.conversation.id);
        self.handle_conversation_event(context, event);
        if refresh_tasks {
            self.reload_tasks();
        }
    }

    fn handle_conversation_event(&mut self, context: Uuid, event: Event) {
        match event {
            Event::RunnerReady => self.model.log_status(LocalizedText::new(
                "Runner 已连接，正在探测 {0}…",
                &[("0", (self.model[context].selected_harness).to_string())],
            )),
            Event::HarnessDetected(probe) => {
                let harness = probe.harness;
                let available = probe.available;
                let message = probe_status(&probe);
                self.model.harnesses.insert(harness, probe);
                if harness == self.model[context].selected_harness {
                    if self.model[context].active_run.is_none() {
                        if !available {
                            self.model[context].model_catalog =
                                ModelCatalogState::NotReady(message.clone());
                        } else if matches!(
                            self.model[context].model_catalog,
                            ModelCatalogState::NotReady(_)
                        ) {
                            self.refresh_model_catalog_in(context);
                        }
                    }
                    self.model.log_status(message);
                }
            }
            event @ (Event::ModelCatalogLoaded { .. } | Event::ModelCatalogFailed { .. }) => {
                if self.handle_catalog_event(context, event) {
                    return;
                }
            }
            event @ (Event::CommitMessageGenerated { .. } | Event::CommitMessageFailed { .. }) => {
                self.handle_commit_message_event(context, event)
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
            Event::RunStarted { run_id, .. } if self.model[context].active_run == Some(run_id) => {
                let harness = self.model[context]
                    .active_harness
                    .unwrap_or(self.model[context].selected_harness);
                self.model.set_run_status_in(
                    context,
                    LocalizedText::new(
                        "{harness} 正在执行…",
                        &[("harness", (harness).to_string())],
                    ),
                );
                let _ = self.storage.update_run_status(run_id, RunStatus::Running);
            }
            Event::RunSessionStarted { run_id, session_id }
                if self.model[context].active_run == Some(run_id) && !session_id.is_empty() =>
            {
                if let Err(error) = self.storage.save_run_session(run_id, &session_id) {
                    self.model.log_status(LocalizedText::new(
                        "无法保存会话，后续可能无法续聊：{error}",
                        &[("error", (error).to_string())],
                    ));
                }
            }
            Event::RunOutputDelta { run_id, text }
                if self.model[context].active_run == Some(run_id) =>
            {
                self.model[context].streaming_text.push_str(&text);
            }
            Event::RunApprovalRequested { run_id, request }
                if self.model[context].active_run == Some(run_id)
                    && !self.model[context].run_cancelling =>
            {
                if !self.model[context]
                    .pending_approvals
                    .iter()
                    .any(|pending| pending.request_id == request.request_id)
                {
                    self.model[context].pending_approvals.push_back(request);
                }
                self.model
                    .set_run_status_in(context, "等待授权，请在桌面弹窗中处理。".into());
            }
            Event::RunApprovalResolved { run_id, request_id }
                if self.model[context].active_run == Some(run_id) =>
            {
                self.model[context]
                    .pending_approvals
                    .retain(|request| request.request_id != request_id);
                if self.model[context].responding_approval == Some(request_id) {
                    self.model[context].responding_approval = None;
                }
                self.model.set_run_status_in(
                    context,
                    if self.model[context].pending_approvals.is_empty() {
                        "审批已处理，等待 Agent 继续…".into()
                    } else {
                        "等待授权，请在桌面弹窗中处理。".into()
                    },
                );
            }
            Event::RunApprovalRejected {
                run_id,
                request_id,
                message,
            } if self.model[context].active_run == Some(run_id) => {
                if self.model[context].responding_approval == Some(request_id) {
                    self.model[context].responding_approval = None;
                }
                self.model.set_run_status_in(context, message.into());
            }
            Event::RunInputAccepted { run_id, message_id }
                if self.model[context].active_run == Some(run_id)
                    && self.model[context].steering_message == Some(message_id) =>
            {
                self.model[context].steering_message = None;
                if let Some(index) = self.model[context]
                    .queued_messages
                    .iter()
                    .position(|message| message.id == message_id)
                    && let Some(message) = self.model[context].queued_messages.remove(index)
                {
                    self.persist_live_message(
                        context,
                        run_id,
                        MessageRole::User,
                        MessageKind::Text,
                        &message.prompt,
                        None,
                    );
                    self.model.log_status("Steer 已送达当前轮次。".into());
                }
            }
            Event::RunInputRejected {
                run_id,
                message_id,
                message,
            } if self.model[context].active_run == Some(run_id)
                && self.model[context].steering_message == Some(message_id) =>
            {
                self.model[context].steering_message = None;
                self.model.log_status(message.into());
            }
            event @ (Event::RunUserAskRequested { .. }
            | Event::RunUserAskAnswerRejected { .. }
            | Event::RunUserAskAnswerSent { .. }
            | Event::RunUserAskFinished { .. }) => self.handle_user_ask_event(context, event),
            Event::RunMessageCompleted { run_id, text }
                if self.model[context].active_run == Some(run_id) =>
            {
                self.model[context].streaming_text.clear();
                self.persist_live_message(
                    context,
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
            } if self.model[context].active_run == Some(run_id) => {
                let content = if summary.is_empty() {
                    name
                } else {
                    format!("{name}\n{summary}")
                };
                self.persist_live_message(
                    context,
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
            } if self.model[context].active_run == Some(run_id) => {
                let content = if is_error {
                    format!("工具执行失败\n{output}")
                } else {
                    output
                };
                self.persist_live_message(
                    context,
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
            } if self.model[context].active_run == Some(run_id) => {
                let _ = self.storage.update_run_status(run_id, status);
                if let Some(message) = message {
                    self.model.set_run_status_in(context, message.into());
                }
            }
            Event::RunFailed {
                run_id, message, ..
            } if self.model[context].active_run == Some(run_id) => {
                self.model
                    .set_run_status_in(context, message.clone().into());
                self.persist_live_message(
                    context,
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
            } if self.model[context].active_run == Some(run_id) => {
                self.handle_run_exit(context, run_id, status, exit_code)
            }
            _ => {}
        }
        if self.model[context]
            .pending_workspace_start
            .as_ref()
            .is_some_and(|pending| pending.context_id == self.model[context].id)
            && !self.model[context].workspace_retry
            && self.model[context]
                .selected_workspace
                .as_ref()
                .is_some_and(|workspace| workspace.status == WorkspaceStatus::Ready)
            && !matches!(
                self.model[context].model_catalog,
                ModelCatalogState::Loading { .. } | ModelCatalogState::Idle
            )
            && self.model[context].active_run.is_none()
        {
            self.retry_workspace_start_in(context);
        }
    }

    fn persist_live_message(
        &mut self,
        context: Uuid,
        run_id: Uuid,
        role: MessageRole,
        kind: MessageKind,
        content: &str,
        tool: Option<ToolMetadata>,
    ) {
        let Some(task_id) = self.model[context].active_task else {
            return;
        };
        if let Ok(message) = self
            .storage
            .append_message(task_id, run_id, role, kind, content, tool)
            && self.model[context].selected_task == Some(task_id)
        {
            self.model[context].messages.push(message);
        }
    }

    // Generation catalogs update settings only; they must not resume a conversation
    // waiting for its own catalog. Return true when that settings-only event is handled.
    fn handle_catalog_event(&mut self, context: Uuid, event: Event) -> bool {
        let generation_kind = GenerationKind::ALL.into_iter().find(|kind| match &event {
            Event::ModelCatalogLoaded {
                request_id,
                harness,
                ..
            }
            | Event::ModelCatalogFailed {
                request_id,
                harness,
                ..
            } => {
                self.model.generation_settings(*kind).harness == *harness
                    && self
                        .model
                        .generation_catalog_in(context, *kind)
                        .accepts(*request_id)
            }
            _ => false,
        });
        if let Some(kind) = generation_kind {
            match event {
                Event::ModelCatalogLoaded { models, .. } => {
                    *self.model.generation_catalog_mut_in(context, kind) = if models.is_empty() {
                        ModelCatalogState::Empty
                    } else {
                        ModelCatalogState::Ready(models)
                    };
                    self.normalize_generation_effort_in(context, kind);
                }
                Event::ModelCatalogFailed { message, .. } => self
                    .model
                    .generation_catalog_mut_in(context, kind)
                    .fail(message.into()),
                _ => unreachable!(),
            }
            return true;
        }
        match event {
            Event::ModelCatalogLoaded {
                request_id,
                harness,
                models,
            } if harness == self.model[context].selected_harness
                && self.model[context].active_run.is_none()
                && self.model[context].model_catalog.accepts(request_id) =>
            {
                self.model[context].model_catalog = if models.is_empty() {
                    ModelCatalogState::Empty
                } else {
                    ModelCatalogState::Ready(models)
                };
                if self.model[context].model_override.is_some()
                    && self.model.selected_catalog_model_in(context).is_some()
                {
                    self.remember_model_name_in(context);
                }
                let selected_unavailable = self.model.model_override_is_unavailable_in(context);
                let effort_reset =
                    !selected_unavailable && self.normalize_catalog_effort_in(context);
                let profile_model_unverified = self.model[context].model_override.is_none()
                    && self
                        .model
                        .selected_provider_profile_in(context)
                        .and_then(|profile| profile.model.as_deref())
                        .is_some()
                    && self.model.selected_catalog_model_in(context).is_none();
                // Catalog progress belongs to the model picker. Record selection
                // changes in the runtime log without changing conversation progress.
                if selected_unavailable {
                    self.model.log_status(LocalizedText::new(
                        "当前 {harness} 模型 {0} 不可用，请重新选择或跟随默认。",
                        &[
                            ("harness", (harness).to_string()),
                            (
                                "0",
                                (self.model[context]
                                    .model_override
                                    .as_deref()
                                    .unwrap_or_default())
                                .to_string(),
                            ),
                        ],
                    ));
                } else if effort_reset {
                    self.model
                        .log_status("当前模型不支持原 effort，已恢复为模型默认。".into());
                } else if profile_model_unverified {
                    self.model.log_status(
                        "Profile 默认模型不在当前目录中；仍可使用，但尚未验证可用。".into(),
                    );
                }
            }
            Event::ModelCatalogFailed {
                request_id,
                harness,
                message,
            } if harness == self.model[context].selected_harness
                && self.model[context].active_run.is_none()
                && self.model[context].model_catalog.accepts(request_id) =>
            {
                self.model[context].model_catalog.fail(message.into());
            }
            _ => {}
        }
        false
    }

    fn handle_commit_message_event(&mut self, context: Uuid, event: Event) {
        match event {
            Event::CommitMessageGenerated {
                request_id,
                message,
            } if self.model[context].commit_message_request == Some(request_id) => {
                self.model[context].commit_message_request = None;
                if !message.trim().is_empty() && !message.contains('\0') {
                    self.model[context].commit_message = message.trim().to_owned();
                    self.model[context].changes_status = None;
                } else {
                    self.model[context].changes_status =
                        Some("模型返回了空或无效的提交说明，请重试。".into());
                }
            }
            Event::CommitMessageFailed {
                request_id,
                message,
            } if self.model[context].commit_message_request == Some(request_id) => {
                self.model[context].commit_message_request = None;
                self.model[context].changes_status = Some(message.into());
            }
            _ => {}
        }
    }

    fn handle_user_ask_event(&mut self, context: Uuid, event: Event) {
        match event {
            Event::RunUserAskRequested {
                run_id,
                request_id,
                questions,
            } if self.model[context].active_run == Some(run_id)
                && !self.model[context].run_cancelling
                && !self.model[context]
                    .pending_user_asks
                    .iter()
                    .any(|request| request.request_id == request_id) =>
            {
                let history = format_user_ask_request(self.model.language, &questions);
                self.persist_live_message(
                    context,
                    run_id,
                    MessageRole::System,
                    MessageKind::Status,
                    &history,
                    None,
                );
                self.model[context]
                    .pending_user_asks
                    .push(PendingUserAsk::new(request_id, questions));
                self.model
                    .set_run_status_in(context, "Agent 正在等待你的回答。".into());
            }
            Event::RunUserAskAnswerRejected {
                run_id,
                request_id,
                message,
            } if self.model[context].active_run == Some(run_id) => {
                if let Some(request) =
                    self.model[context]
                        .pending_user_asks
                        .iter_mut()
                        .find(|request| {
                            request.request_id == request_id
                                && request.submission == UserAskSubmissionState::Submitting
                        })
                {
                    request.submission = UserAskSubmissionState::Pending;
                    request.error = Some(message.clone());
                    request.submitted_answers = None;
                    self.model.set_run_status_in(context, message.into());
                }
            }
            Event::RunUserAskAnswerSent { run_id, request_id }
                if self.model[context].active_run == Some(run_id) =>
            {
                if let Some(request) =
                    self.model[context]
                        .pending_user_asks
                        .iter_mut()
                        .find(|request| {
                            request.request_id == request_id
                                && request.submission == UserAskSubmissionState::Submitting
                        })
                {
                    request.submission = UserAskSubmissionState::Sent;
                    request.error = None;
                    self.model
                        .set_run_status_in(context, "回答已发送，等待 Agent 继续…".into());
                }
            }
            Event::RunUserAskFinished {
                run_id,
                request_id,
                status,
                message,
            } if self.model[context].active_run == Some(run_id) => {
                let request = self.model[context]
                    .pending_user_asks
                    .iter()
                    .position(|request| request.request_id == request_id)
                    .map(|index| self.model[context].pending_user_asks.remove(index));
                if let Some(request) = request {
                    let history = format_user_ask_result(
                        self.model.language,
                        &request,
                        status,
                        message.as_deref(),
                    );
                    self.persist_live_message(
                        context,
                        run_id,
                        MessageRole::System,
                        MessageKind::Status,
                        &history,
                        None,
                    );
                    self.model.set_run_status_in(
                        context,
                        if self.model[context].pending_user_asks.is_empty() {
                            message
                                .unwrap_or_else(|| {
                                    user_ask_status_text(self.model.language, status).into()
                                })
                                .into()
                        } else {
                            "Agent 正在等待你的回答。".into()
                        },
                    );
                }
            }
            _ => {}
        }
    }

    fn handle_run_exit(
        &mut self,
        context: Uuid,
        run_id: Uuid,
        status: RunStatus,
        exit_code: Option<i32>,
    ) {
        let pending_catalog_request = match self.model[context].model_catalog {
            ModelCatalogState::Loading { request_id, .. } => Some(request_id),
            _ => None,
        };
        let task_id = self.model[context].active_task;
        let cancelled = self.model[context].run_cancelling;
        let stale_user_asks = std::mem::take(&mut self.model[context].pending_user_asks);
        let stale_status = if cancelled || status == RunStatus::Cancelled {
            UserAskStatus::Cancelled
        } else {
            UserAskStatus::Expired
        };
        for request in stale_user_asks {
            let history = format_user_ask_result(self.model.language, &request, stale_status, None);
            self.persist_live_message(
                context,
                run_id,
                MessageRole::System,
                MessageKind::Status,
                &history,
                None,
            );
        }
        let _ = self.storage.finish_run(run_id, status, exit_code);
        if status == RunStatus::Completed {
            self.model[context].completed_runs.insert(run_id);
        }
        self.model[context].streaming_text.clear();
        self.model[context].active_run = None;
        self.model[context].active_checkout = None;
        self.model[context].run_cancelling = false;
        self.model[context].steering_message = None;
        self.model[context].active_run_started_at = None;
        self.model[context].active_run_elapsed_seconds = None;
        self.model[context].active_task = None;
        self.model[context].active_harness = None;
        self.model[context].active_permission_mode = None;
        self.model[context].pending_approvals.clear();
        self.model[context].responding_approval = None;
        self.model.set_run_status_in(
            context,
            match status {
                RunStatus::Completed => "任务已完成".into(),
                RunStatus::Cancelled => "任务已取消".into(),
                RunStatus::Failed => "任务执行失败".into(),
                _ => LocalizedText::new("任务状态：{status}", &[("status", (status).to_string())]),
            },
        );
        if status == RunStatus::Completed && !cancelled && self.model.sound.task_complete {
            crate::infrastructure::sound::play_task_complete();
        }
        self.reload_tasks();
        self.reload_workspaces_in(context);
        if self.model[context].selected_task != task_id
            && let Some(selected_task) = self.model[context].selected_task
        {
            self.reload_task_context(context, selected_task);
        }
        if status == RunStatus::Completed
            && !cancelled
            && let Some(message) = self.model[context]
                .queued_messages
                .iter()
                .find(|message| Some(message.task_id) == task_id)
        {
            self.send_queued_message_in(context, message.id);
        }
        if self.model[context].active_run.is_none()
            && (pending_catalog_request
                .is_some_and(|request_id| self.model[context].model_catalog.accepts(request_id))
                || self.model[context].catalog_project
                    != self.model[context]
                        .selected_project
                        .as_ref()
                        .map(|project| project.id))
        {
            // Retry an ignored response only if task restoration has not replaced the request.
            self.refresh_model_catalog_in(context);
        }
    }
}
