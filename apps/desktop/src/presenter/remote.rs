use super::{Presenter, RemoteCommand};
use crate::remote_control::{RemoteControl, RemoteProject, RemoteState};
use nexus_domain::HarnessKind;

impl Presenter {
    pub(super) fn handle_remote_command(&mut self, command: RemoteCommand) -> bool {
        match command {
            RemoteCommand::GetState { reply } => {
                let _ = reply.send(self.remote_state());
                false
            }
            RemoteCommand::GetMessages { task_id, reply } => {
                let _ = reply.send(self.storage.messages(task_id).unwrap_or_default());
                false
            }
            RemoteCommand::StartRun {
                project_id,
                prompt,
                reply,
            } => {
                if reply.is_closed() {
                    return false;
                }
                let result = if self.model.active_run.is_some() {
                    Err("已有任务正在执行".into())
                } else if let Some(project) = self
                    .model
                    .projects
                    .iter()
                    .find(|project| project.id == project_id)
                    .cloned()
                {
                    if self
                        .model
                        .selected_project
                        .as_ref()
                        .map(|project| project.id)
                        != Some(project_id)
                    {
                        self.select_project(project);
                    }
                    let executable = self.model.executable.clone();
                    if self.submit(&prompt, &executable) {
                        Ok(())
                    } else {
                        Err(self.model.status.clone())
                    }
                } else {
                    Err("项目不存在，请先在 Nexus 中打开项目".into())
                };
                let _ = reply.send(result);
                true
            }
            RemoteCommand::CancelRun { reply } => {
                let result = self.request_cancel();
                let _ = reply.send(result);
                true
            }
        }
    }

    fn remote_state(&self) -> RemoteState {
        let mut tasks = self
            .model
            .projects
            .iter()
            .flat_map(|project| self.storage.tasks(project.id).unwrap_or_default())
            .collect::<Vec<_>>();
        tasks.sort_by_key(|task| std::cmp::Reverse(task.created_at));
        RemoteState {
            projects: self
                .model
                .projects
                .iter()
                .map(RemoteProject::from)
                .collect(),
            tasks,
            selected_project_id: self
                .model
                .selected_project
                .as_ref()
                .map(|project| project.id),
            selected_task_id: self.model.selected_task,
            active_run_id: self.model.active_run,
            active_task_id: self.model.active_task,
            streaming_text: self.model.streaming_text.clone(),
            status: self.model.status.clone(),
            harness: self.model.selected_harness,
            model: self
                .model
                .selected_provider_profile()
                .and_then(|profile| profile.model.clone())
                .or_else(|| {
                    (self.model.selected_harness == HarnessKind::Claude)
                        .then(|| self.model.claude_model.to_string())
                }),
            effort: self.model.effort,
            harness_ready: self.model.selected_probe().is_some_and(|probe| {
                let profile_ready = self
                    .model
                    .selected_provider_profile()
                    .is_some_and(|profile| profile.credential_configured);
                probe.available && (probe.authenticated || profile_ready)
            }),
        }
    }

    pub(crate) fn notify_remote_changed(&self) {
        if let Some(remote_control) = &self.remote_control {
            remote_control.notify_changed();
        }
    }

    pub(crate) fn remote_endpoint(&self) -> Option<String> {
        self.remote_control.as_ref().map(RemoteControl::endpoint)
    }

    pub(crate) fn remote_token(&self) -> Option<&str> {
        self.remote_control.as_ref().map(RemoteControl::token)
    }

    pub(crate) fn remote_control_error(&self) -> Option<&str> {
        self.remote_control_error.as_deref()
    }

    pub(crate) fn copyable_remote_link(&mut self) -> Option<String> {
        let remote_control = self.remote_control.as_ref()?;
        let link = format!(
            "{}/#token={}",
            remote_control.endpoint(),
            remote_control.token()
        );
        self.model.status = "远程控制链接已复制。".into();
        self.notify_remote_changed();
        Some(link)
    }

    pub(crate) fn copyable_remote_token(&mut self) -> Option<String> {
        let token = self.remote_control.as_ref()?.token().to_owned();
        self.model.status = "远程控制访问令牌已复制。".into();
        self.notify_remote_changed();
        Some(token)
    }
}
