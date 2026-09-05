use std::{cell::RefCell, collections::HashMap, rc::Rc};

use super::*;
use crate::{
    infrastructure::{codex_history::Event as HistoryEvent, credentials::CredentialStore},
    model::history::HistoryMessage,
};
use nexus_domain::{MessageKind, MessageRole, RunStatus};
use nexus_protocol::{ErrorCode, Event, HarnessProbe, PROTOCOL_VERSION};

#[derive(Clone, Default)]
struct FakeRunner(Rc<RefCell<FakeRunnerState>>);

#[derive(Default)]
struct FakeRunnerState {
    commands: Vec<CommandEnvelope>,
    events: Vec<EventEnvelope>,
    fail_send: bool,
}

#[derive(Clone, Default)]
struct FakeCredentialStore(Rc<RefCell<HashMap<Uuid, String>>>);

impl CredentialStore for FakeCredentialStore {
    fn set_api_key(&self, profile_id: Uuid, api_key: &str) -> Result<()> {
        self.0.borrow_mut().insert(profile_id, api_key.to_owned());
        Ok(())
    }

    fn api_key(&self, profile_id: Uuid) -> Result<Option<String>> {
        Ok(self.0.borrow().get(&profile_id).cloned())
    }

    fn delete_api_key(&self, profile_id: Uuid) -> Result<()> {
        self.0.borrow_mut().remove(&profile_id);
        Ok(())
    }
}

impl RunnerPort for FakeRunner {
    fn send(&self, command: CommandEnvelope) -> Result<()> {
        let mut state = self.0.borrow_mut();
        if state.fail_send {
            anyhow::bail!("runner disconnected");
        }
        state.commands.push(command);
        Ok(())
    }

    fn drain_events(&self) -> Vec<EventEnvelope> {
        std::mem::take(&mut self.0.borrow_mut().events)
    }
}

impl FakeRunner {
    fn emit(&self, event: Event) {
        self.0.borrow_mut().events.push(EventEnvelope {
            protocol_version: PROTOCOL_VERSION,
            id: Uuid::new_v4(),
            sequence: 1,
            event,
        });
    }
}

fn fixture() -> (Presenter, FakeRunner, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    let runner = FakeRunner::default();
    let mut presenter = Presenter::new(storage, Ok(Box::new(runner.clone())), None);
    presenter.open_project(directory.path());
    presenter
        .model
        .harnesses
        .insert(HarnessKind::Claude, ready_probe(HarnessKind::Claude));
    runner.0.borrow_mut().commands.clear();
    (presenter, runner, directory)
}

fn ready_probe(harness: HarnessKind) -> HarnessProbe {
    HarnessProbe {
        harness,
        available: true,
        authenticated: true,
        executable: format!("/fake/{harness}"),
        version: Some("1.2.3".into()),
        message: "ready".into(),
    }
}

fn profile_draft(id: Option<Uuid>, name: &str, api_key: &str) -> ProviderProfileDraft {
    ProviderProfileDraft {
        id,
        name: name.into(),
        api_key_env: "DEEPSEEK_API_KEY".into(),
        api_key: api_key.into(),
        base_url_env: String::new(),
        base_url: String::new(),
        model: "deepseek/deepseek-v4-pro".into(),
    }
}

#[test]
fn startup_restores_preferences_and_probes_all_harnesses() {
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    for (key, value) in [
        ("default_harness", "codex"),
        ("claude_model", "opus"),
        ("thinking_effort", "high"),
        ("codex_executable", "/custom/codex"),
    ] {
        storage.set_setting(key, value).unwrap();
    }
    let runner = FakeRunner::default();
    let presenter = Presenter::new(storage, Ok(Box::new(runner.clone())), None);

    assert_eq!(presenter.model().selected_harness, HarnessKind::Codex);
    assert_eq!(presenter.model().claude_model, ClaudeModel::Opus);
    assert_eq!(presenter.model().effort, ThinkingEffort::High);
    assert_eq!(presenter.model().executable, "/custom/codex");
    let state = runner.0.borrow();
    assert_eq!(state.commands.len(), 4);
    assert!(matches!(state.commands[0].command, Command::RunnerHello));
    assert!(state.commands.iter().any(|command| matches!(&command.command,
        Command::HarnessProbe { harness: HarnessKind::Codex, executable } if executable == "/custom/codex")));
    assert!(!presenter.model().can_submit());
}

#[test]
fn startup_reads_profile_credential_state_from_the_system_store() {
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    let configured_id = Uuid::new_v4();
    let missing_id = Uuid::new_v4();
    storage
        .set_provider_profiles(&[
            ProviderProfile {
                id: configured_id,
                name: "Configured".into(),
                harness: HarnessKind::Codex,
                api_key_env: "CODEX_API_KEY".into(),
                base_url_env: None,
                base_url: None,
                model: None,
                credential_configured: false,
            },
            ProviderProfile {
                id: missing_id,
                name: "Missing".into(),
                harness: HarnessKind::Codex,
                api_key_env: "CODEX_API_KEY".into(),
                base_url_env: None,
                base_url: None,
                model: None,
                credential_configured: true,
            },
        ])
        .unwrap();
    let credentials = FakeCredentialStore::default();
    credentials
        .0
        .borrow_mut()
        .insert(configured_id, "stored-key".into());

    let presenter = Presenter::new_with_credentials(
        storage,
        Err(anyhow::anyhow!("runner unavailable")),
        None,
        Box::new(credentials),
    );

    assert!(
        presenter
            .model()
            .provider_profiles
            .iter()
            .find(|profile| profile.id == configured_id)
            .unwrap()
            .credential_configured
    );
    assert!(
        !presenter
            .model()
            .provider_profiles
            .iter()
            .find(|profile| profile.id == missing_id)
            .unwrap()
            .credential_configured
    );
}

#[test]
fn invalid_submissions_never_create_tasks_or_send_commands() {
    let (mut presenter, runner, _directory) = fixture();
    let project = presenter.model.selected_project.take().unwrap();
    assert!(!presenter.submit("hello", "claude"));
    assert_eq!(presenter.model().status, "请先选择项目目录。");
    presenter.model.selected_project = Some(project.clone());
    assert!(!presenter.submit(" \n ", "claude"));
    assert_eq!(presenter.model().status, "Prompt 不能为空。");
    assert!(!presenter.submit("hello", " "));
    for (available, authenticated) in [(false, false), (true, false), (false, true)] {
        let probe = presenter
            .model
            .harnesses
            .get_mut(&HarnessKind::Claude)
            .unwrap();
        probe.available = available;
        probe.authenticated = authenticated;
        assert!(!presenter.model().can_submit());
        assert!(!presenter.submit("hello", "claude"));
    }
    assert!(presenter.storage.tasks(project.id).unwrap().is_empty());
    assert!(runner.0.borrow().commands.is_empty());
}

#[test]
fn expired_remote_start_does_not_change_selection_or_start_a_run() {
    let (mut presenter, runner, _directory) = fixture();
    let project_id = presenter.model.selected_project.as_ref().unwrap().id;
    let other_directory = tempfile::tempdir().unwrap();
    presenter.open_project(other_directory.path());
    let selected_project_id = presenter.model.selected_project.as_ref().unwrap().id;
    let status = presenter.model.status.clone();
    let (reply, response) = tokio::sync::oneshot::channel();
    drop(response);

    assert!(!presenter.handle_remote_command(RemoteCommand::StartRun {
        project_id,
        prompt: "expired request".into(),
        reply,
    }));

    assert_eq!(
        presenter.model.selected_project.as_ref().unwrap().id,
        selected_project_id
    );
    assert_eq!(presenter.model.status, status);
    assert!(presenter.model.active_run.is_none());
    assert!(presenter.storage.tasks(project_id).unwrap().is_empty());
    assert!(runner.0.borrow().commands.is_empty());

    let (reply, mut response) = tokio::sync::oneshot::channel();
    assert!(presenter.handle_remote_command(RemoteCommand::StartRun {
        project_id,
        prompt: "retry request".into(),
        reply,
    }));
    assert_eq!(response.try_recv().unwrap(), Ok(()));
    assert_eq!(presenter.storage.tasks(project_id).unwrap().len(), 1);
    assert_eq!(runner.0.borrow().commands.len(), 1);
}

#[test]
fn submit_persists_configuration_and_prevents_duplicate_runs() {
    let (mut presenter, runner, _directory) = fixture();
    presenter.select_model(ClaudeModel::Opus);
    presenter.select_effort(ThinkingEffort::XHigh);
    assert!(presenter.model().can_submit());
    assert!(presenter.submit("  explain this project\n", "claude-custom"));
    assert!(!presenter.model().can_submit());
    assert!(!presenter.submit("duplicate", "claude-custom"));
    let state = runner.0.borrow();
    assert_eq!(state.commands.len(), 1);
    let Command::RunStart(request) = &state.commands[0].command else {
        panic!("expected start");
    };
    assert_eq!(request.prompt, "explain this project");
    assert_eq!(request.model.as_deref(), Some("opus"));
    assert_eq!(request.effort, ThinkingEffort::XHigh);
    assert_eq!(
        request.executable,
        ready_probe(HarnessKind::Claude).executable
    );
    assert_eq!(presenter.model().active_run, Some(request.run_id));
    assert_eq!(presenter.model().selected_task, Some(request.task_id));
    let config = presenter
        .storage
        .conversation_config(request.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(config.executable, request.executable);
    assert_eq!(config.model, "opus");
    assert_eq!(config.effort, request.effort);
    assert_eq!(presenter.model().messages[0].content, request.prompt);
    assert_eq!(presenter.model().tasks.len(), 1);
}

#[test]
fn send_failure_finishes_saved_run_without_entering_busy_state() {
    let (mut presenter, runner, _directory) = fixture();
    runner.0.borrow_mut().fail_send = true;
    assert!(!presenter.submit("hello", "claude"));
    assert!(presenter.model().active_run.is_none());
    assert_eq!(presenter.model().status, "Runner 不可用，任务未启动。");
    let project_id = presenter.model().selected_project.as_ref().unwrap().id;
    let tasks = presenter.storage.tasks(project_id).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, RunStatus::Failed);
}

#[test]
fn runner_events_update_timeline_and_persist_terminal_statuses() {
    for status in [
        RunStatus::Completed,
        RunStatus::Failed,
        RunStatus::Cancelled,
        RunStatus::Interrupted,
    ] {
        let (mut presenter, runner, _directory) = fixture();
        assert!(presenter.submit("hello", "claude"));
        let run_id = presenter.model().active_run.unwrap();
        let task_id = presenter.model().active_task.unwrap();
        runner.emit(Event::RunStarted { run_id, pid: 42 });
        runner.emit(Event::RunOutputDelta {
            run_id,
            text: "partial".into(),
        });
        assert!(presenter.drain_events());
        assert_eq!(presenter.model().streaming_text, "partial");
        assert!(!presenter.drain_events());
        runner.emit(Event::RunMessageCompleted {
            run_id,
            text: "answer".into(),
        });
        if status == RunStatus::Failed {
            runner.emit(Event::RunFailed {
                run_id,
                code: ErrorCode::UnexpectedExit,
                message: "failed".into(),
            });
        }
        runner.emit(Event::RunExited {
            run_id,
            status,
            exit_code: Some(0),
        });
        presenter.drain_events();
        assert!(presenter.model().streaming_text.is_empty());
        assert!(presenter.model().active_run.is_none());
        assert!(presenter.model().active_task.is_none());
        assert!(presenter.model().active_harness.is_none());
        assert_eq!(presenter.model().tasks[0].status, status);
        let messages = presenter.storage.messages(task_id).unwrap();
        assert_eq!(messages[1].content, "answer");
        assert_eq!(messages[1].sequence, 2);
        if status == RunStatus::Failed {
            assert_eq!(messages[2].kind, MessageKind::Error);
        }
    }
}

#[test]
fn unrelated_run_events_cannot_replace_the_active_run() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("hello", "claude"));
    let active_run = presenter.model().active_run;
    let other_run = Uuid::new_v4();
    runner.emit(Event::RunStarted {
        run_id: other_run,
        pid: 42,
    });
    runner.emit(Event::RunOutputDelta {
        run_id: other_run,
        text: "unrelated".into(),
    });
    runner.emit(Event::RunMessageCompleted {
        run_id: other_run,
        text: "unrelated".into(),
    });
    runner.emit(Event::RunExited {
        run_id: other_run,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    assert_eq!(presenter.model().active_run, active_run);
    assert_eq!(presenter.model().messages.len(), 1);
    assert!(presenter.model().streaming_text.is_empty());
}

#[test]
fn active_run_locks_configuration_and_cancels_the_matching_run() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("hello", "claude"));
    let task_id = presenter.model().active_task;
    let run_id = presenter.model().active_run.unwrap();
    presenter.select_model(ClaudeModel::Opus);
    presenter.select_effort(ThinkingEffort::Max);
    assert!(!presenter.select_harness(HarnessKind::Codex, "claude"));
    presenter.new_task();
    presenter.select_codex_thread("history".into());
    assert_eq!(presenter.model().claude_model, ClaudeModel::Default);
    assert_eq!(presenter.model().effort, ThinkingEffort::Medium);
    assert_eq!(presenter.model().selected_task, task_id);
    assert!(presenter.model().selected_codex_thread.is_none());
    presenter.cancel();
    assert!(matches!(runner.0.borrow().commands.last().unwrap().command,
        Command::RunCancel { run_id: id } if id == run_id));
    let project_id = presenter.model().selected_project.as_ref().unwrap().id;
    assert_eq!(
        presenter.storage.tasks(project_id).unwrap()[0].status,
        RunStatus::Cancelling
    );
}

#[test]
fn switching_harnesses_restores_each_executable_and_codex_uses_default_model() {
    let (mut presenter, runner, _directory) = fixture();
    presenter.select_model(ClaudeModel::Opus);
    presenter
        .storage
        .set_setting("codex_executable", "/custom/codex")
        .unwrap();
    assert!(presenter.select_harness(HarnessKind::Codex, "/custom/claude"));
    assert_eq!(presenter.model().executable, "/custom/codex");
    assert!(!presenter.select_harness(HarnessKind::Codex, "edited"));
    assert_eq!(
        presenter
            .storage
            .setting("claude_executable")
            .unwrap()
            .as_deref(),
        Some("/custom/claude")
    );
    presenter
        .model
        .harnesses
        .insert(HarnessKind::Codex, ready_probe(HarnessKind::Codex));
    assert!(presenter.submit("hello", "/custom/codex"));
    let state = runner.0.borrow();
    let Command::RunStart(request) = &state.commands.last().unwrap().command else {
        panic!("expected start");
    };
    assert_eq!(request.harness, HarnessKind::Codex);
    assert!(request.model.is_none());
}

#[test]
fn provider_profile_keeps_secret_out_of_storage_and_injects_only_the_selected_run() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    let runner = FakeRunner::default();
    let credentials = FakeCredentialStore::default();
    let mut presenter = Presenter::new_with_credentials(
        storage,
        Ok(Box::new(runner.clone())),
        None,
        Box::new(credentials.clone()),
    );
    presenter.open_project(directory.path());
    assert!(presenter.select_harness(HarnessKind::Omp, "claude"));
    presenter.model.harnesses.insert(
        HarnessKind::Omp,
        HarnessProbe {
            authenticated: false,
            ..ready_probe(HarnessKind::Omp)
        },
    );
    runner.0.borrow_mut().commands.clear();

    let profile_id = presenter
        .save_provider_profile(profile_draft(None, "DeepSeek", "super-secret"))
        .unwrap();

    assert!(presenter.model().can_submit());
    assert_eq!(
        credentials.0.borrow().get(&profile_id).map(String::as_str),
        Some("super-secret")
    );
    assert!(
        !presenter
            .storage
            .setting("provider_profiles")
            .unwrap()
            .unwrap()
            .contains("super-secret")
    );
    assert!(presenter.submit("use the selected provider", "omp"));
    let state = runner.0.borrow();
    let Command::RunStart(request) = &state.commands[0].command else {
        panic!("expected start");
    };
    assert_eq!(request.harness, HarnessKind::Omp);
    assert_eq!(request.model.as_deref(), Some("deepseek/deepseek-v4-pro"));
    assert_eq!(request.environment.len(), 1);
    assert_eq!(request.environment[0].name, "DEEPSEEK_API_KEY");
    assert_eq!(request.environment[0].value, "super-secret");
    assert!(!format!("{:?}", state.commands[0]).contains("super-secret"));
}

#[test]
fn remote_state_uses_the_selected_provider_profile() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    let credentials = FakeCredentialStore::default();
    let mut presenter = Presenter::new_with_credentials(
        storage,
        Err(anyhow::anyhow!("runner unavailable")),
        None,
        Box::new(credentials),
    );
    presenter.open_project(directory.path());
    assert!(presenter.select_harness(HarnessKind::Omp, "claude"));
    presenter.model.harnesses.insert(
        HarnessKind::Omp,
        HarnessProbe {
            authenticated: false,
            ..ready_probe(HarnessKind::Omp)
        },
    );
    presenter
        .save_provider_profile(profile_draft(None, "DeepSeek", "super-secret"))
        .unwrap();
    let (reply, mut response) = tokio::sync::oneshot::channel();

    assert!(!presenter.handle_remote_command(RemoteCommand::GetState { reply }));
    let state = response.try_recv().unwrap();
    assert_eq!(state.harness, HarnessKind::Omp);
    assert_eq!(state.model.as_deref(), Some("deepseek/deepseek-v4-pro"));
    assert!(state.harness_ready);
}

#[test]
fn provider_profiles_can_be_updated_switched_and_deleted() {
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    let runner = FakeRunner::default();
    let credentials = FakeCredentialStore::default();
    let mut presenter = Presenter::new_with_credentials(
        storage,
        Ok(Box::new(runner)),
        None,
        Box::new(credentials.clone()),
    );
    assert!(presenter.select_harness(HarnessKind::Omp, "claude"));
    let profile_id = presenter
        .save_provider_profile(profile_draft(None, "DeepSeek", "first-key"))
        .unwrap();
    assert_eq!(
        presenter.model().selected_provider_profile().unwrap().id,
        profile_id
    );

    assert_eq!(
        presenter.save_provider_profile(profile_draft(Some(profile_id), "DeepSeek Production", "")),
        Some(profile_id)
    );
    assert_eq!(
        credentials.0.borrow().get(&profile_id).map(String::as_str),
        Some("first-key")
    );
    assert!(presenter.select_provider_profile(None));
    assert!(presenter.model().selected_provider_profile().is_none());
    credentials.0.borrow_mut().remove(&profile_id);
    assert!(presenter.select_provider_profile(Some(profile_id)));
    assert!(
        !presenter
            .model()
            .selected_provider_profile()
            .unwrap()
            .credential_configured
    );
    assert!(presenter.model().status.contains("没有 API Key"));
    assert!(presenter.delete_provider_profile(profile_id));
    assert!(presenter.model().provider_profiles.is_empty());
    assert!(!credentials.0.borrow().contains_key(&profile_id));
}

#[test]
fn provider_profiles_reject_process_control_environment_variables() {
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    let mut presenter = Presenter::new_with_credentials(
        storage,
        Err(anyhow::anyhow!("runner unavailable")),
        None,
        Box::new(FakeCredentialStore::default()),
    );
    let mut draft = profile_draft(None, "Unsafe", "secret");
    draft.api_key_env = "LD_PRELOAD".into();

    assert!(presenter.save_provider_profile(draft).is_none());
    assert!(presenter.model().provider_profiles.is_empty());
    assert!(presenter.model().status.contains("*_API_KEY"));
}

#[test]
fn provider_profiles_bound_visible_name_and_model_lengths() {
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    let mut presenter = Presenter::new_with_credentials(
        storage,
        Err(anyhow::anyhow!("runner unavailable")),
        None,
        Box::new(FakeCredentialStore::default()),
    );
    let mut draft = profile_draft(None, &"n".repeat(49), "secret");

    assert!(presenter.save_provider_profile(draft).is_none());
    assert!(presenter.model().status.contains("48"));

    draft = profile_draft(None, "Valid name", "secret");
    draft.model = "m".repeat(129);
    assert!(presenter.save_provider_profile(draft).is_none());
    assert!(presenter.model().status.contains("128"));
    assert!(presenter.model().provider_profiles.is_empty());
}

#[test]
fn selecting_a_saved_task_restores_its_configuration_and_messages() {
    let (mut presenter, runner, _directory) = fixture();
    presenter.select_model(ClaudeModel::Sonnet);
    presenter.select_effort(ThinkingEffort::High);
    assert!(presenter.submit("hello", "claude"));
    let run_id = presenter.model().active_run.unwrap();
    let task_id = presenter.model().selected_task.unwrap();
    runner.emit(Event::RunExited {
        run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    presenter.new_task();
    assert!(presenter.model().messages.is_empty());
    assert!(presenter.select_harness(HarnessKind::Codex, "claude"));
    presenter.select_effort(ThinkingEffort::Low);
    presenter.select_task(task_id);
    assert_eq!(presenter.model().selected_harness, HarnessKind::Claude);
    assert_eq!(presenter.model().claude_model, ClaudeModel::Sonnet);
    assert_eq!(presenter.model().effort, ThinkingEffort::High);
    assert_eq!(
        presenter.model().executable,
        ready_probe(HarnessKind::Claude).executable
    );
    assert_eq!(presenter.model().messages[0].content, "hello");
}

#[test]
fn history_responses_only_update_the_selected_thread() {
    let (mut presenter, _, _directory) = fixture();
    presenter.select_codex_thread("selected".into());
    presenter.model.codex_thread_loading = true;
    presenter.handle_codex_history_event(HistoryEvent::ThreadLoaded {
        thread_id: "previous".into(),
        result: Err("stale error".into()),
    });
    assert!(presenter.model().codex_thread_loading);
    assert!(presenter.model().codex_history_messages.is_empty());
    let message = HistoryMessage {
        role: MessageRole::Assistant,
        kind: MessageKind::Text,
        content: "history".into(),
    };
    presenter.handle_codex_history_event(HistoryEvent::ThreadLoaded {
        thread_id: "selected".into(),
        result: Ok(vec![message.clone()]),
    });
    assert!(!presenter.model().codex_thread_loading);
    assert_eq!(presenter.model().codex_history_messages, vec![message]);
    presenter.handle_codex_history_event(HistoryEvent::ThreadLoaded {
        thread_id: "selected".into(),
        result: Err("read failed".into()),
    });
    assert_eq!(
        presenter.model().codex_history_messages[0].kind,
        MessageKind::Error
    );
    assert_eq!(
        presenter.model().codex_history_messages[0].content,
        "read failed"
    );
}
