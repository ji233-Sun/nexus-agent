use super::{Presenter, executable_setting_key};
use crate::i18n::{Language, LocalizedText};
use crate::infrastructure::git;
use crate::infrastructure::storage::NewTaskRun;
use crate::model::workspace::WorkspaceKind;
use crate::model::{
    GenerationKind, PendingUserAsk, QueuedMessage, ResolvedModelSelection, UserAskSubmissionState,
};
use nexus_domain::{
    PermissionMode, RunStatus, UserAskAnswer, UserAskAnswerMode, UserAskAnswerValue,
    UserAskQuestion, UserAskStatus, compact_task_title,
};
use nexus_protocol::{Command, CommandEnvelope, StartRun};
use std::time::Instant;
use uuid::Uuid;

impl Presenter {
    pub(crate) fn begin_attachment_import(
        &mut self,
        count: usize,
    ) -> Option<(Uuid, std::path::PathBuf)> {
        if self.model.conversation.attachments_loading || count == 0 {
            return None;
        }
        if self.model.conversation.attachments.len() + count > nexus_domain::Attachment::MAX_COUNT {
            self.report_attachment_error("每条消息最多包含 8 个附件。".into());
            return None;
        }
        self.model.conversation.attachments_loading = true;
        self.model.conversation.attachment_error = None;
        Some((
            self.model.conversation.id,
            self.storage.attachment_directory(),
        ))
    }

    pub(crate) fn finish_attachment_import(
        &mut self,
        conversation_id: Uuid,
        results: Vec<Result<nexus_domain::Attachment, String>>,
    ) {
        let conversation = if self.model.conversation.id == conversation_id {
            Some(&mut self.model.conversation)
        } else {
            self.model.conversations.get_mut(&conversation_id)
        };
        let Some(conversation) = conversation else {
            return;
        };
        conversation.attachments_loading = false;
        let mut errors = Vec::new();
        for result in results {
            match result {
                Ok(attachment) => conversation.attachments.push(attachment),
                Err(error) => errors.push(error),
            }
        }
        conversation.attachment_error = (!errors.is_empty()).then(|| errors.join("\n").into());
    }

    pub(crate) fn report_attachment_error(&mut self, error: String) {
        self.report_attachment_error_in(self.model.conversation.id, error)
    }

    pub(crate) fn report_attachment_error_in(&mut self, context: Uuid, error: String) {
        self.model[context].attachment_error = Some(error.clone().into());
        self.model.log_status(error.into());
    }
    pub(crate) fn attach_pdf_capture(
        &mut self,
        name: &str,
        page: u32,
        bytes: &[u8],
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.model.conversation.attachments_loading,
            "正在添加附件，请稍候。"
        );
        anyhow::ensure!(
            self.model.conversation.attachments.len() < nexus_domain::Attachment::MAX_COUNT,
            "每条消息最多包含 8 个附件。"
        );
        let image = crate::infrastructure::attachments::save_pdf_capture(
            &self.storage.attachment_directory(),
            name,
            page,
            bytes,
        )?;
        self.model.conversation.attachments.push(image);
        self.model.conversation.attachment_error = None;
        Ok(())
    }

    pub(crate) fn remove_attachment(&mut self, index: usize) {
        if index < self.model.conversation.attachments.len() {
            self.model.conversation.attachments.remove(index);
        }
        self.model.conversation.attachment_error = None;
    }

    pub(crate) fn restore_attachments(&mut self, images: &[nexus_domain::Attachment]) -> bool {
        if self.model.conversation.attachments_loading {
            return false;
        }
        if self.model.conversation.attachments.len() + images.len()
            > nexus_domain::Attachment::MAX_COUNT
        {
            self.model.conversation.attachment_error = Some("每条消息最多包含 8 个附件。".into());
            return false;
        }
        self.model
            .conversation
            .attachments
            .extend_from_slice(images);
        self.model.conversation.attachment_error = None;
        true
    }

    pub(crate) fn refresh_run_elapsed(&mut self, now: Instant) -> bool {
        let elapsed = self
            .model
            .conversation
            .active_run_started_at
            .map(|started| now.saturating_duration_since(started).as_secs());
        if self.model.conversation.active_run_elapsed_seconds == elapsed {
            return false;
        }
        self.model.conversation.active_run_elapsed_seconds = elapsed;
        true
    }

    pub(crate) fn submit(&mut self, prompt: &str, configured_executable: &str) -> bool {
        let attachments = self.model.conversation.attachments.clone();
        if self.model.conversation.attachments_loading {
            return false;
        }
        if let Err(error) = nexus_harness_core::validate_attachments(
            &attachments,
            self.model.conversation.selected_harness,
        ) {
            self.report_attachment_error(error);
            return false;
        }
        let prompt = if prompt.trim().is_empty() && !attachments.is_empty() {
            self.model.language.text("请查看所附文件。")
        } else {
            prompt
        };
        if self.model.conversation.active_run.is_some() {
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
                    attachments,
                    permission_mode: self.model.conversation.permission_mode,
                });
            self.model.conversation.attachments.clear();
            self.model
                .log_status("消息已排队，将在当前轮次结束后依次发送。".into());
            return true;
        }
        let started = self.start_run_with_attachments(
            self.model.conversation.selected_task,
            prompt,
            configured_executable,
            self.model.conversation.permission_mode,
            &attachments,
        );
        if started {
            self.model.conversation.attachments.clear();
            self.model.conversation.attachment_error = None;
        }
        started
    }

    pub(crate) fn send_queued_message(&mut self, message_id: Uuid) -> bool {
        self.send_queued_message_in(self.model.conversation.id, message_id)
    }

    pub(crate) fn send_queued_message_in(&mut self, context: Uuid, message_id: Uuid) -> bool {
        let Some(message) = self.model[context]
            .queued_messages
            .iter()
            .find(|message| message.id == message_id)
            .cloned()
        else {
            return false;
        };
        if self.model[context].active_run.is_some()
            || self.model[context].selected_task != Some(message.task_id)
        {
            return false;
        }
        let executable = self.model[context].executable.clone();
        if !self.start_run_with_attachments_in(
            context,
            Some(message.task_id),
            &message.prompt,
            &executable,
            message.permission_mode,
            &message.attachments,
        ) {
            return false;
        }
        self.model[context]
            .queued_messages
            .retain(|queued| queued.id != message_id);
        true
    }

    pub(crate) fn remove_queued_message(&mut self, message_id: Uuid) {
        if self.model.conversation.steering_message == Some(message_id) {
            return;
        }
        self.model
            .conversation
            .queued_messages
            .retain(|message| message.id != message_id);
    }

    pub(crate) fn steer_queued_message(&mut self, message_id: Uuid) -> bool {
        if !self.model.can_queue() || self.model.conversation.steering_message.is_some() {
            return false;
        }
        let Some(index) = self
            .model
            .conversation
            .queued_messages
            .iter()
            .position(|message| {
                message.id == message_id
                    && Some(message.task_id) == self.model.conversation.active_task
            })
        else {
            return false;
        };
        let run_id = self.model.conversation.active_run.unwrap();
        if !self.model.conversation.queued_messages[index]
            .attachments
            .is_empty()
        {
            self.model.log_status("含附件的消息将在下一轮发送。".into());
            return false;
        }
        if self.model.conversation.active_permission_mode
            != Some(self.model.conversation.queued_messages[index].permission_mode)
        {
            self.model
                .log_status("排队消息的权限与当前轮次不同，请等待下一轮发送。".into());
            return false;
        }
        let command = CommandEnvelope::new(Command::RunSteer {
            run_id,
            message_id,
            prompt: self.model.conversation.queued_messages[index]
                .prompt
                .clone(),
        });
        if !self
            .runner
            .as_ref()
            .is_some_and(|runner| runner.send(command).is_ok())
        {
            self.model
                .log_status("Runner 不可用，消息仍保留在队列中。".into());
            return false;
        }
        // A Steer that races with turn completion becomes the next queued message.
        let message = self
            .model
            .conversation
            .queued_messages
            .remove(index)
            .unwrap();
        self.model.conversation.queued_messages.push_front(message);
        self.model.conversation.steering_message = Some(message_id);
        self.model.log_status("等待工具执行结束后介入…".into());
        true
    }

    pub(crate) fn set_user_ask_question(&mut self, request_id: Uuid, index: usize) -> bool {
        let Some(request) = self
            .model
            .conversation
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
            .conversation
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
        let Some(request) = self
            .model
            .conversation
            .pending_user_asks
            .iter_mut()
            .find(|request| {
                request.request_id == request_id
                    && request.submission == UserAskSubmissionState::Pending
            })
        else {
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
        let Some(request) = self
            .model
            .conversation
            .pending_user_asks
            .iter_mut()
            .find(|request| {
                request.request_id == request_id
                    && request.submission == UserAskSubmissionState::Pending
            })
        else {
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
        self.model.conversation.active_run.is_some()
            && !self.model.conversation.run_cancelling
            && self
                .model
                .conversation
                .pending_user_asks
                .iter()
                .any(|request| {
                    request.request_id == request_id
                        && request.submission == UserAskSubmissionState::Pending
                        && request.answers().is_some()
                })
    }

    pub(crate) fn submit_user_ask(&mut self, request_id: Uuid) -> bool {
        let Some(answers) = self
            .model
            .conversation
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
        let Some(run_id) = self
            .model
            .conversation
            .active_run
            .filter(|_| !self.model.conversation.run_cancelling)
        else {
            return false;
        };
        let Some(index) = self
            .model
            .conversation
            .pending_user_asks
            .iter()
            .position(|request| {
                request.request_id == request_id
                    && request.submission == UserAskSubmissionState::Pending
            })
        else {
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
                let request = &mut self.model.conversation.pending_user_asks[index];
                request.submission = UserAskSubmissionState::Submitting;
                request.error = None;
                request.submitted_answers = Some(submitted_answers);
                self.model.set_run_status("正在提交回答…".into());
                true
            }
            Err(error) => {
                let message = format!("无法提交 User Ask 回答：{error}");
                self.model.conversation.pending_user_asks[index].error = Some(message.clone());
                self.model.set_run_status(message.into());
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
        self.start_run_with_attachments(
            task_id,
            prompt,
            configured_executable,
            permission_mode,
            &[],
        )
    }

    pub(super) fn start_run_with_attachments(
        &mut self,
        task_id: Option<Uuid>,
        prompt: &str,
        configured_executable: &str,
        permission_mode: PermissionMode,
        attachments: &[nexus_domain::Attachment],
    ) -> bool {
        self.start_run_with_attachments_in(
            self.model.conversation.id,
            task_id,
            prompt,
            configured_executable,
            permission_mode,
            attachments,
        )
    }

    pub(super) fn start_run_with_attachments_in(
        &mut self,
        context: Uuid,
        task_id: Option<Uuid>,
        prompt: &str,
        configured_executable: &str,
        permission_mode: PermissionMode,
        attachments: &[nexus_domain::Attachment],
    ) -> bool {
        if let Err(error) = nexus_harness_core::validate_attachments(
            attachments,
            self.model[context].selected_harness,
        ) {
            self.report_attachment_error_in(context, error);
            return false;
        }
        if self.model.updates.state.is_installing() {
            self.model
                .log_status("正在安装应用更新，重启后可继续任务。".into());
            return false;
        }
        if self.model[context].active_run.is_some()
            || self.model.harness_manager.operating.is_some()
            || self.model.occupied_run_slots() >= 2
        {
            return false;
        }
        let project_id = self.model[context]
            .selected_project
            .as_ref()
            .map(|project| project.id);
        let prompt = prompt.trim().to_owned();
        if prompt.is_empty() {
            self.model.log_status("Prompt 不能为空。".into());
            return false;
        }
        let configured_executable = configured_executable.trim().to_owned();
        if configured_executable.is_empty() {
            self.model.log_status(LocalizedText::new(
                "{0} 可执行文件不能为空。",
                &[("0", (self.model[context].selected_harness).to_string())],
            ));
            return false;
        }
        let profile_ready = self
            .model
            .selected_provider_profile_in(context)
            .is_some_and(|profile| profile.credential_configured);
        let Some(probe) = self
            .model
            .selected_probe_in(context)
            .filter(|probe| probe.available && (probe.authenticated || profile_ready))
        else {
            self.model.log_status(LocalizedText::new(
                "{0} 尚未就绪，请先完成探测和登录。",
                &[("0", (self.model[context].selected_harness).to_string())],
            ));
            return false;
        };
        let executable = probe.executable.clone();
        let harness_version = probe.version.clone();
        let harness = self.model[context].selected_harness;
        let session_id = if let Some(task_id) = task_id {
            let config = match self.storage.conversation_config(task_id) {
                Ok(Some(config)) => config,
                _ => {
                    self.model.log_status("无法读取当前任务的会话配置。".into());
                    return false;
                }
            };
            if config.harness != harness {
                self.model.log_status(LocalizedText::new(
                    "当前会话使用 {0}，请切回该 Harness 继续对话，或新建任务。",
                    &[("0", (config.harness).to_string())],
                ));
                return false;
            }
            let Some(session_id) = config.session_id.filter(|id| !id.is_empty()) else {
                self.model
                    .log_status("当前任务未保存可恢复的会话，无法继续对话；请新建任务。".into());
                return false;
            };
            if config.executable != executable {
                self.model.log_status(
                    "当前探测结果与任务保存的可执行文件不一致，请重新选择任务并完成探测。".into(),
                );
                return false;
            }
            Some(session_id)
        } else {
            None
        };
        if !self.model.catalog_selection_is_valid_in(context) {
            self.model.log_status(LocalizedText::new(
                "当前 {harness} 模型或 effort 未通过目录验证，请调整选择后重试。",
                &[("harness", (harness).to_string())],
            ));
            return false;
        }
        let environment = match self.provider_launch_configuration_in(context, harness) {
            Ok(configuration) => configuration,
            Err(error) => {
                self.model.log_status(LocalizedText::new(
                    "无法读取 Provider Profile：{error}",
                    &[("error", (error).to_string())],
                ));
                return false;
            }
        };
        let ResolvedModelSelection { model, effort } =
            self.model.resolved_model_selection_in(context);
        let title_generation = session_id
            .is_none()
            .then(|| {
                self.generation_configuration_in(context, GenerationKind::Title)
                    .ok()
            })
            .flatten();
        let title = compact_task_title(&prompt).unwrap_or_else(|| "新任务".into());
        if task_id.is_none()
            && self.model[context].selected_workspace.is_none()
            && self.model[context].workspace_draft.kind == WorkspaceKind::Worktree
        {
            let started =
                self.begin_worktree_in(context, &prompt, &configured_executable, permission_mode);
            if started && let Some(pending) = &mut self.model[context].pending_workspace_start {
                pending.attachments = attachments.to_vec();
            }
            return started;
        }
        let workspace = if let Some(task_id) = task_id {
            self.storage.task_workspace(task_id)
        } else if let Some(workspace) = &self.model[context].selected_workspace {
            Ok(Some(workspace.clone()))
        } else if let Some(project_id) = project_id {
            self.storage.workspace(project_id)
        } else {
            Ok(None)
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
                self.model.log_status(error.to_string().into());
                return false;
            }
        };
        let transport = self.harness_transport(harness);
        let Ok(pending_run) = self.storage.prepare_task_run(NewTaskRun {
            workspace_id: Some(workspace.id),
            task_id,
            project_id,
            title: &title,
            prompt: &prompt,
            attachments,
            harness,
            executable: &executable,
            model: model.as_deref(),
            effort,
            permission_mode,
            harness_version: harness_version.as_deref(),
        }) else {
            self.model.log_status("无法保存任务运行。".into());
            return false;
        };
        let task_id = pending_run.task_id;
        let run_id = pending_run.run_id;
        // Keep the user's message and initial title unchanged in the local history.
        let run_prompt = if session_id.is_none()
            && workspace.managed
            && workspace.kind == WorkspaceKind::Worktree
        {
            format!(
                "{prompt}\n\n<nexus_worktree_context>\n\
                 Nexus has already created a dedicated worktree for this task. \
                 After understanding the user's task, if the current branch is still `{}`, \
                 rename it once to a meaningful task-specific name with `git branch -m <name>`, \
                 following the repository's branch naming rules. \
                 Choose a different name if it already exists; never force-overwrite a branch. \
                 Keep working in this directory; do not create another worktree or switch branches.\n\
                 </nexus_worktree_context>",
                workspace.branch.as_deref().unwrap_or_default(),
            )
        } else {
            prompt.clone()
        };
        let command = CommandEnvelope::new(Command::RunStart(StartRun {
            transport,
            run_id,
            task_id,
            session_id,
            cwd: workspace.path.clone(),
            prompt: run_prompt,
            attachments: attachments.to_vec(),
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
                self.model
                    .log_status("无法保存任务运行，已请求停止 Runner。".into());
                return false;
            }
            self.model[context].active_run = Some(run_id);
            self.model[context].active_checkout = Some(checkout);
            self.model[context].active_run_started_at = Some(Instant::now());
            self.model[context].active_run_elapsed_seconds = Some(0);
            self.model[context].active_task = Some(task_id);
            self.model[context].active_harness = Some(harness);
            self.model[context].active_permission_mode = Some(permission_mode);
            self.model[context].selected_task = Some(task_id);
            self.model[context].selected_workspace = self
                .storage
                .task_workspace(task_id)
                .ok()
                .flatten()
                .or(Some(workspace));
            self.model[context].messages = self.storage.messages(task_id).unwrap_or_default();
            self.model[context].completed_runs =
                self.storage.completed_runs(task_id).unwrap_or_default();
            self.model.set_run_status_in(
                context,
                LocalizedText::translated(|language| {
                    let effort_label = match language {
                        Language::Chinese => effort.to_string(),
                        Language::English => language.effort(effort).to_owned(),
                    };
                    language.format(
                        "正在启动 {harness} · {effort}",
                        &[("harness", harness.to_string()), ("effort", effort_label)],
                    )
                }),
            );
            let _ = self
                .storage
                .set_setting(executable_setting_key(harness), &configured_executable);

            self.reload_tasks();
            true
        } else {
            self.model.log_status("Runner 不可用，任务未启动。".into());
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
        if self.model.conversation.active_run != Some(run_id)
            || self.model.conversation.run_cancelling
            || self.model.conversation.responding_approval.is_some()
            || !self
                .model
                .conversation
                .pending_approvals
                .front()
                .is_some_and(|request| {
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
            self.model
                .set_run_status("审批回复发送失败，请重试或停止任务。".into());
            return false;
        }
        self.model.conversation.responding_approval = Some(request_id);
        self.model.set_run_status("正在发送审批回复…".into());
        true
    }

    pub(super) fn request_cancel(&mut self) -> Result<(), String> {
        let Some(run_id) = self.model.conversation.active_run else {
            return Err("没有运行中的任务".into());
        };
        let runner = self
            .runner
            .as_ref()
            .ok_or_else(|| "Runner 不可用".to_owned())?;
        runner
            .send(CommandEnvelope::new(Command::RunCancel { run_id }))
            .map_err(|error| error.to_string())?;
        self.model.conversation.run_cancelling = true;
        self.model.conversation.pending_approvals.clear();
        self.model.conversation.responding_approval = None;
        let _ = self
            .storage
            .update_run_status(run_id, RunStatus::Cancelling);
        let harness = self
            .model
            .conversation
            .active_harness
            .unwrap_or(self.model.conversation.selected_harness);
        self.model.set_run_status(LocalizedText::new(
            "正在停止 {harness}…",
            &[("harness", (harness).to_string())],
        ));
        Ok(())
    }
}

pub(super) fn format_user_ask_request(language: Language, questions: &[UserAskQuestion]) -> String {
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

pub(super) fn format_user_ask_result(
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

pub(super) fn user_ask_status_text(language: Language, status: UserAskStatus) -> &'static str {
    language.text(match status {
        UserAskStatus::Answered => "User Ask 已回答",
        UserAskStatus::Cancelled => "User Ask 已取消",
        UserAskStatus::Expired => "User Ask 已失效",
        UserAskStatus::Failed => "User Ask 回答失败",
    })
}
