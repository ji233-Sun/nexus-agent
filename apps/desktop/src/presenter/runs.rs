use super::{Presenter, executable_setting_key};
use crate::i18n::{Language, LocalizedText, probe_status};
use crate::infrastructure::storage::NewTaskRun;
use crate::model::{ModelCatalogState, QueuedMessage, ResolvedModelSelection};
use nexus_domain::{HarnessKind, MessageKind, MessageRole, RunStatus, ToolMetadata};
use nexus_protocol::{Command, CommandEnvelope, Event, StartRun};
use std::time::Instant;
use uuid::Uuid;

impl Presenter {
    pub(crate) fn refresh_run_elapsed(&mut self, now: Instant) -> bool {
        let elapsed = self
            .active_run_started_at
            .map(|started| now.saturating_duration_since(started).as_secs());
        if self.model.active_run_elapsed_seconds == elapsed {
            return false;
        }
        self.model.active_run_elapsed_seconds = elapsed;
        true
    }

    pub(super) fn handle_event(&mut self, event: Event) {
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
                self.model.model_catalog = ModelCatalogState::Failed(message.clone().into());
                self.normalize_catalog_effort();
                self.model.status = LocalizedText::new(
                    "{harness} 模型目录加载失败：{message}",
                    &[
                        ("harness", (harness).to_string()),
                        ("message", (message).to_string()),
                    ],
                );
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
                let _ = self.storage.finish_run(run_id, status, exit_code);
                let task_id = self.model.active_task;
                let cancelled = self.model.run_cancelling;
                self.model.streaming_text.clear();
                self.model.active_run = None;
                self.model.run_cancelling = false;
                self.model.steering_message = None;
                self.active_run_started_at = None;
                self.model.active_run_elapsed_seconds = None;
                self.model.active_task = None;
                self.model.active_harness = None;
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
                    && (matches!(self.model.model_catalog, ModelCatalogState::Loading { .. })
                        || self.catalog_project
                            != self
                                .model
                                .selected_project
                                .as_ref()
                                .map(|project| project.id))
                {
                    // A response received during the run cannot change its selection.
                    let status = self.model.status.clone();
                    self.refresh_model_catalog();
                    self.model.status = status;
                }
            }
            _ => {}
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
            self.model.queued_messages.push_back(QueuedMessage {
                id: Uuid::new_v4(),
                task_id: self.model.active_task.unwrap(),
                prompt: prompt.trim().to_owned(),
            });
            self.model.status = "消息已排队，将在当前轮次结束后依次发送。".into();
            return true;
        }
        self.start_run(self.model.selected_task, prompt, configured_executable)
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
        if !self.start_run(Some(message.task_id), &message.prompt, &executable) {
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

    pub(super) fn start_run(
        &mut self,
        task_id: Option<Uuid>,
        prompt: &str,
        configured_executable: &str,
    ) -> bool {
        if self.model.active_run.is_some() {
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
        let environment = match self.provider_launch_configuration() {
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
        let title: String = prompt.chars().take(48).collect();
        let Ok(pending_run) = self.storage.prepare_task_run(NewTaskRun {
            task_id,
            project_id: project.id,
            title: &title,
            prompt: &prompt,
            harness,
            executable: &executable,
            model: model.as_deref(),
            effort,
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
            cwd: project.canonical_path,
            prompt: prompt.clone(),
            harness,
            executable: executable.clone(),
            model,
            effort,
            environment,
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
            self.active_run_started_at = Some(Instant::now());
            self.model.active_run_elapsed_seconds = Some(0);
            self.model.active_task = Some(task_id);
            self.model.active_harness = Some(harness);
            self.model.selected_task = Some(task_id);
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
