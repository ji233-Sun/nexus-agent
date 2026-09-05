use super::{Presenter, executable_setting_key};
use crate::infrastructure::storage::NewTaskRun;
use nexus_domain::{HarnessKind, MessageKind, MessageRole, RunStatus};
use nexus_protocol::{Command, CommandEnvelope, Event, StartRun};
use uuid::Uuid;

impl Presenter {
    pub(super) fn handle_event(&mut self, event: Event) {
        match event {
            Event::RunnerReady => {
                self.model.status =
                    format!("Runner 已连接，正在探测 {}…", self.model.selected_harness)
            }
            Event::HarnessDetected(probe) => {
                let harness = probe.harness;
                let message = probe.message.clone();
                let history_executable =
                    (harness == HarnessKind::Codex).then(|| probe.executable.clone());
                self.model.harnesses.insert(harness, probe);
                if harness == self.model.selected_harness {
                    self.model.status = message;
                }
                if let Some(executable) = history_executable {
                    self.connect_codex_history(executable);
                }
            }
            Event::RunStarted { run_id, .. } if self.model.active_run == Some(run_id) => {
                let harness = self
                    .model
                    .active_harness
                    .unwrap_or(self.model.selected_harness);
                self.model.status = format!("{harness} 正在执行…");
                let _ = self.storage.update_run_status(run_id, RunStatus::Running);
            }
            Event::RunOutputDelta { run_id, text } if self.model.active_run == Some(run_id) => {
                self.model.streaming_text.push_str(&text);
            }
            Event::RunMessageCompleted { run_id, text }
                if self.model.active_run == Some(run_id) =>
            {
                self.model.streaming_text.clear();
                self.persist_live_message(run_id, MessageRole::Assistant, MessageKind::Text, &text);
            }
            Event::RunToolStarted {
                run_id,
                name,
                summary,
                ..
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
                );
            }
            Event::RunToolCompleted {
                run_id,
                output,
                is_error,
                ..
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
                );
            }
            Event::RunStatusChanged {
                run_id,
                status,
                message,
            } if self.model.active_run == Some(run_id) => {
                let _ = self.storage.update_run_status(run_id, status);
                if let Some(message) = message {
                    self.model.status = message;
                }
            }
            Event::RunFailed {
                run_id, message, ..
            } if self.model.active_run == Some(run_id) => {
                self.model.status = message.clone();
                self.persist_live_message(
                    run_id,
                    MessageRole::System,
                    MessageKind::Error,
                    &message,
                );
            }
            Event::RunExited {
                run_id,
                status,
                exit_code,
            } if self.model.active_run == Some(run_id) => {
                let _ = self.storage.finish_run(run_id, status, exit_code);
                self.model.streaming_text.clear();
                self.model.active_run = None;
                self.model.active_task = None;
                self.model.active_harness = None;
                self.model.status = match status {
                    RunStatus::Completed => "任务已完成".into(),
                    RunStatus::Cancelled => "任务已取消".into(),
                    RunStatus::Failed => "任务执行失败".into(),
                    _ => format!("任务状态：{status}"),
                };
                self.reload_tasks();
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
    ) {
        let Some(task_id) = self.model.active_task else {
            return;
        };
        if let Ok(message) = self
            .storage
            .append_message(task_id, run_id, role, kind, content)
        {
            self.model.messages.push(message);
        }
    }

    pub(crate) fn submit(&mut self, prompt: &str, configured_executable: &str) -> bool {
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
            self.model.status = format!("{} 可执行文件不能为空。", self.model.selected_harness);
            return false;
        }
        let Some(probe) = self
            .model
            .selected_probe()
            .filter(|probe| probe.available && probe.authenticated)
        else {
            self.model.status = format!(
                "{} 尚未就绪，请先完成探测和登录。",
                self.model.selected_harness
            );
            return false;
        };
        let executable = probe.executable.clone();
        let harness_version = probe.version.clone();
        let harness = self.model.selected_harness;
        let model = match harness {
            HarnessKind::Claude => self.model.claude_model.cli_value().map(str::to_owned),
            HarnessKind::Codex => None,
        };
        let title: String = prompt.chars().take(48).collect();
        let created = self.storage.create_task_run(NewTaskRun {
            project_id: project.id,
            title: &title,
            prompt: &prompt,
            harness,
            executable: &executable,
            model: model.as_deref(),
            effort: self.model.effort,
            harness_version: harness_version.as_deref(),
        });
        let Ok((task_id, run_id)) = created else {
            self.model.status = "无法保存新任务。".into();
            return false;
        };
        let command = CommandEnvelope::new(Command::RunStart(StartRun {
            run_id,
            task_id,
            cwd: project.canonical_path,
            prompt: prompt.clone(),
            harness,
            executable: executable.clone(),
            model,
            effort: self.model.effort,
        }));
        if let Some(runner) = &self.runner
            && runner.send(command).is_ok()
        {
            self.model.active_run = Some(run_id);
            self.model.active_task = Some(task_id);
            self.model.active_harness = Some(harness);
            self.model.selected_task = Some(task_id);
            self.model.selected_codex_thread = None;
            self.model.codex_history_messages.clear();
            self.model.codex_thread_loading = false;
            self.model.messages = self.storage.messages(task_id).unwrap_or_default();
            self.model.status = format!("正在启动 {harness} · {}", self.model.effort);
            let _ = self
                .storage
                .set_setting(executable_setting_key(harness), &configured_executable);

            self.reload_tasks();
            true
        } else {
            let _ = self.storage.finish_run(run_id, RunStatus::Failed, None);
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
        let _ = self
            .storage
            .update_run_status(run_id, RunStatus::Cancelling);
        let harness = self
            .model
            .active_harness
            .unwrap_or(self.model.selected_harness);
        self.model.status = format!("正在停止 {harness}…");
        Ok(())
    }
}
