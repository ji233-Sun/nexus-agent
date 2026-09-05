mod history;
mod runs;

#[cfg(test)]
mod tests;

use crate::{
    infrastructure::{
        codex_history::Client as CodexHistoryClient, git::is_git_dirty, storage::Storage,
    },
    model::AppModel,
};
use anyhow::Result;
use nexus_domain::{ClaudeModel, HarnessKind, Project, ThinkingEffort};
use nexus_protocol::{Command, CommandEnvelope, EventEnvelope};
use std::{path::Path, str::FromStr as _};
use uuid::Uuid;

pub(crate) trait RunnerPort {
    fn send(&self, command: CommandEnvelope) -> Result<()>;
    fn drain_events(&self) -> Vec<EventEnvelope>;
}

pub(crate) struct Presenter {
    model: AppModel,
    storage: Storage,
    runner: Option<Box<dyn RunnerPort>>,
    codex_history_client: Option<CodexHistoryClient>,
    codex_history_executable: Option<String>,
}

impl Presenter {
    pub(crate) fn new(
        storage: Storage,
        runner: Result<Box<dyn RunnerPort>>,
        storage_error: Option<String>,
    ) -> Self {
        let projects = storage.projects().unwrap_or_default();
        let selected_harness = storage
            .setting("default_harness")
            .ok()
            .flatten()
            .and_then(|value| HarnessKind::from_str(&value).ok())
            .unwrap_or_default();
        let model = storage
            .setting("claude_model")
            .ok()
            .flatten()
            .and_then(|value| ClaudeModel::from_str(&value).ok())
            .unwrap_or_default();
        let effort = storage
            .setting("thinking_effort")
            .ok()
            .flatten()
            .and_then(|value| ThinkingEffort::from_str(&value).ok())
            .unwrap_or_default();
        let executable = storage
            .setting(executable_setting_key(selected_harness))
            .ok()
            .flatten()
            .unwrap_or_else(|| selected_harness.default_executable().into());

        let has_storage_error = storage_error.is_some();
        let (runner, runner_error) = match runner {
            Ok(runner) => (Some(runner), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let mut presenter = Self {
            storage,
            runner,
            model: AppModel {
                projects,
                selected_harness,
                claude_model: model,
                effort,
                executable,
                status: storage_error.unwrap_or_else(|| "正在连接本地 Runner…".into()),
                ..AppModel::default()
            },
            codex_history_client: None,
            codex_history_executable: None,
        };
        if let Some(runner) = &presenter.runner {
            let _ = runner.send(CommandEnvelope::new(Command::RunnerHello));
            for harness in HarnessKind::ALL {
                let executable = presenter
                    .storage
                    .setting(executable_setting_key(harness))
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| harness.default_executable().into());
                let _ = runner.send(CommandEnvelope::new(Command::HarnessProbe {
                    harness,
                    executable,
                }));
            }
        } else if !has_storage_error {
            presenter.model.status = runner_error.unwrap_or_default();
        }
        presenter
    }

    pub(crate) fn model(&self) -> &AppModel {
        &self.model
    }

    pub(crate) fn drain_events(&mut self) -> bool {
        let runner_events = self
            .runner
            .as_ref()
            .map(|runner| runner.drain_events())
            .unwrap_or_default();
        let history_events = self
            .codex_history_client
            .as_ref()
            .map(CodexHistoryClient::drain_events)
            .unwrap_or_default();
        let changed = !runner_events.is_empty() || !history_events.is_empty();
        for envelope in runner_events {
            self.handle_event(envelope.event);
        }
        for event in history_events {
            self.handle_codex_history_event(event);
        }
        changed
    }

    pub(crate) fn open_project(&mut self, path: &Path) {
        match self.storage.open_project(path) {
            Ok(project) => {
                self.select_project(project);
                self.model.projects = self.storage.projects().unwrap_or_default();
            }
            Err(error) => self.model.status = format!("无法打开项目：{error}"),
        }
    }

    pub(crate) fn new_task(&mut self) {
        if self.model.active_run.is_some() || self.model.selected_project.is_none() {
            return;
        }
        self.model.selected_task = None;
        self.model.selected_codex_thread = None;
        self.model.messages.clear();
        self.model.codex_history_messages.clear();
        self.model.codex_thread_loading = false;
        self.model.streaming_text.clear();
        self.model.status = "已准备好新任务。".into();
    }

    pub(crate) fn select_project(&mut self, project: Project) {
        self.model.project_dirty = is_git_dirty(Path::new(&project.canonical_path));
        self.model.selected_project = Some(project);
        self.model.selected_task = None;
        self.model.selected_codex_thread = None;
        self.model.messages.clear();
        self.model.codex_history_messages.clear();
        self.model.codex_thread_loading = false;
        self.model.streaming_text.clear();
        self.reload_tasks();
    }

    fn reload_tasks(&mut self) {
        self.model.tasks = self
            .model
            .selected_project
            .as_ref()
            .and_then(|project| self.storage.tasks(project.id).ok())
            .unwrap_or_default();
    }

    pub(crate) fn select_task(&mut self, task_id: Uuid) {
        self.model.selected_task = Some(task_id);
        self.model.selected_codex_thread = None;
        self.model.codex_history_messages.clear();
        self.model.codex_thread_loading = false;
        self.model.messages = self.storage.messages(task_id).unwrap_or_default();
        if let Ok(Some(config)) = self.storage.conversation_config(task_id) {
            self.model.selected_harness = config.harness;
            self.model.effort = config.effort;
            if config.harness == HarnessKind::Claude {
                self.model.claude_model = ClaudeModel::from_str(&config.model).unwrap_or_default();
            }
            let executable = if config.executable.is_empty() {
                self.storage
                    .setting(executable_setting_key(config.harness))
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| config.harness.default_executable().into())
            } else {
                config.executable
            };
            self.model.executable = executable;
        }
    }

    pub(crate) fn probe(&mut self, executable: &str) {
        let executable = executable.trim().to_owned();
        let harness = self.model.selected_harness;
        if let Some(runner) = &self.runner {
            let _ = runner.send(CommandEnvelope::new(Command::HarnessProbe {
                harness,
                executable: executable.clone(),
            }));
            let _ = self
                .storage
                .set_setting(executable_setting_key(harness), &executable);
            self.model.status = format!("正在探测 {harness}…");
        }
    }

    pub(crate) fn select_harness(
        &mut self,
        harness: HarnessKind,
        current_executable: &str,
    ) -> bool {
        if self.model.active_run.is_some() || self.model.selected_harness == harness {
            return false;
        }
        let current_executable = current_executable.trim().to_owned();
        if !current_executable.is_empty() {
            let _ = self.storage.set_setting(
                executable_setting_key(self.model.selected_harness),
                &current_executable,
            );
        }

        self.model.selected_harness = harness;
        let _ = self
            .storage
            .set_setting("default_harness", self.model.selected_harness.as_str());
        let executable = self
            .storage
            .setting(executable_setting_key(self.model.selected_harness))
            .ok()
            .flatten()
            .unwrap_or_else(|| self.model.selected_harness.default_executable().into());
        self.model.executable = executable.clone();
        if let Some(runner) = &self.runner {
            let _ = runner.send(CommandEnvelope::new(Command::HarnessProbe {
                harness: self.model.selected_harness,
                executable,
            }));
            self.model.status = format!("正在探测 {}…", self.model.selected_harness);
        }
        true
    }

    pub(crate) fn select_model(&mut self, model: ClaudeModel) {
        if self.model.active_run.is_some()
            || self.model.selected_harness != HarnessKind::Claude
            || self.model.claude_model == model
        {
            return;
        }
        self.model.claude_model = model;
        let _ = self
            .storage
            .set_setting("claude_model", self.model.claude_model.as_str());
    }

    pub(crate) fn select_effort(&mut self, effort: ThinkingEffort) {
        if self.model.active_run.is_some() || self.model.effort == effort {
            return;
        }
        self.model.effort = effort;
        let _ = self
            .storage
            .set_setting("thinking_effort", self.model.effort.as_str());
    }
}

fn executable_setting_key(harness: HarnessKind) -> &'static str {
    match harness {
        HarnessKind::Claude => "claude_executable",
        HarnessKind::Codex => "codex_executable",
    }
}
