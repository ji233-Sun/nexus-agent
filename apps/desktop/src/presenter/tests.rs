use std::{cell::RefCell, collections::HashMap, fs, rc::Rc, time::Duration};

use super::*;
use crate::{
    infrastructure::{
        codex_history::Event as HistoryEvent, credentials::CredentialStore, storage::NewTaskRun,
    },
    model::history::HistoryMessage,
};
use nexus_domain::{MessageKind, MessageRole, ModelDescriptor, ModelReasoningEffort, RunStatus};
use nexus_protocol::{ErrorCode, Event, HarnessProbe, PROTOCOL_VERSION, StartRun};

#[derive(Clone, Default)]
pub(crate) struct FakeRunner(Rc<RefCell<FakeRunnerState>>);

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
    pub(crate) fn emit(&self, event: Event) {
        self.0.borrow_mut().events.push(EventEnvelope {
            protocol_version: PROTOCOL_VERSION,
            id: Uuid::new_v4(),
            sequence: 1,
            event,
        });
    }
}

pub(crate) fn fixture() -> (Presenter, FakeRunner, tempfile::TempDir) {
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

#[test]
fn appearance_preferences_restore_and_accept_missing_or_invalid_settings() {
    use crate::model::{AppearanceSettings, ThemePreference};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("appearance.sqlite");
    let mut presenter = Presenter::new(
        Storage::open(&path).unwrap(),
        Err(anyhow::anyhow!("test")),
        None,
    );
    assert_eq!(presenter.model().appearance, AppearanceSettings::default());
    for theme in [
        ThemePreference::Dark,
        ThemePreference::Light,
        ThemePreference::System,
    ] {
        let appearance = AppearanceSettings {
            theme,
            glass: false,
            reduced_motion: true,
        };
        assert!(presenter.set_appearance(appearance));
        drop(presenter);
        presenter = Presenter::new(
            Storage::open(&path).unwrap(),
            Err(anyhow::anyhow!("test")),
            None,
        );
        assert_eq!(presenter.model().appearance, appearance);
    }
    for (raw, expected) in [
        (
            r#"{"theme":"dark"}"#,
            AppearanceSettings {
                theme: ThemePreference::Dark,
                ..Default::default()
            },
        ),
        ("invalid json", AppearanceSettings::default()),
        (r#"{"theme":"unknown"}"#, AppearanceSettings::default()),
    ] {
        presenter.storage.set_setting("appearance", raw).unwrap();
        drop(presenter);
        presenter = Presenter::new(
            Storage::open(&path).unwrap(),
            Err(anyhow::anyhow!("test")),
            None,
        );
        assert_eq!(presenter.model().appearance, expected);
    }
}

#[test]
fn conversation_actions_keep_active_and_archived_models_in_sync() {
    let (mut presenter, _runner, _directory) = fixture();
    let project = presenter.model().selected_project.clone().unwrap();
    let create_task = |presenter: &mut Presenter, title: &str| {
        presenter
            .storage
            .create_task_run(NewTaskRun {
                task_id: None,
                project_id: project.id,
                title,
                prompt: title,
                harness: HarnessKind::Claude,
                executable: "claude",
                model: None,
                effort: ThinkingEffort::Medium,
                harness_version: None,
            })
            .unwrap()
            .0
    };
    let first_task = create_task(&mut presenter, "First conversation");
    let second_task = create_task(&mut presenter, "Second conversation");
    for task_id in [first_task, second_task] {
        presenter
            .model
            .queued_messages
            .push_back(crate::model::QueuedMessage {
                id: Uuid::new_v4(),
                task_id,
                prompt: "unsent follow-up".into(),
            });
    }
    presenter.select_project(project);
    presenter.select_task(first_task);

    assert!(presenter.archive_task(first_task));
    assert!(presenter.model().selected_task.is_none());
    assert!(presenter.model().messages.is_empty());
    assert_eq!(presenter.model().tasks[0].id, second_task);
    assert_eq!(presenter.model().archived_tasks[0].id, first_task);
    assert_eq!(presenter.model().queued_messages.len(), 2);

    assert!(presenter.restore_task(first_task));
    assert_eq!(presenter.model().tasks.len(), 2);
    assert!(presenter.model().archived_tasks.is_empty());

    presenter.model.active_run = Some(Uuid::new_v4());
    assert!(!presenter.delete_task(first_task));
    assert_eq!(presenter.model().tasks.len(), 2);
    assert_eq!(presenter.model().queued_messages.len(), 2);
    presenter.model.active_run = None;

    assert!(presenter.delete_task(first_task));
    assert_eq!(presenter.model().tasks.len(), 1);
    assert_eq!(presenter.model().queued_messages.len(), 1);
    assert_eq!(presenter.model().queued_messages[0].task_id, second_task);
    assert!(presenter.archive_task(second_task));
    assert_eq!(presenter.model().archived_tasks.len(), 1);
    assert!(presenter.delete_archived_tasks());
    assert!(presenter.model().archived_tasks.is_empty());
    assert!(presenter.model().tasks.is_empty());
    assert!(presenter.model().queued_messages.is_empty());
}

struct ArchivedProjectFixture {
    presenter: Presenter,
    _directory: tempfile::TempDir,
    archived_project: Project,
    archived_task: Uuid,
    oldest_recent_project: Project,
    oldest_recent_task: Uuid,
}

fn archived_project_fixture() -> ArchivedProjectFixture {
    let directory = tempfile::tempdir().unwrap();
    let mut storage = Storage::open(&directory.path().join("nexus.db")).unwrap();
    let archived_project_path = directory.path().join("archived-project");
    fs::create_dir(&archived_project_path).unwrap();
    let archived_project = storage.open_project(&archived_project_path).unwrap();
    let archived_task = storage
        .create_task_run(NewTaskRun {
            task_id: None,
            project_id: archived_project.id,
            title: "Archived conversation",
            prompt: "Archived conversation",
            harness: HarnessKind::Claude,
            executable: "claude",
            model: None,
            effort: ThinkingEffort::Medium,
            harness_version: None,
        })
        .unwrap()
        .0;
    storage.archive_task(archived_task).unwrap();
    let mut oldest_recent = None;
    for index in 0..20 {
        let project_path = directory.path().join(format!("recent-project-{index:02}"));
        fs::create_dir(&project_path).unwrap();
        let project = storage.open_project(&project_path).unwrap();
        if index == 0 {
            let task = storage
                .create_task_run(NewTaskRun {
                    task_id: None,
                    project_id: project.id,
                    title: "Selected conversation",
                    prompt: "Selected conversation",
                    harness: HarnessKind::Claude,
                    executable: "claude",
                    model: None,
                    effort: ThinkingEffort::Medium,
                    harness_version: None,
                })
                .unwrap()
                .0;
            oldest_recent = Some((project, task));
        }
    }
    let (oldest_recent_project, oldest_recent_task) = oldest_recent.unwrap();

    ArchivedProjectFixture {
        presenter: Presenter::new(storage, Err(anyhow::anyhow!("test")), None),
        _directory: directory,
        archived_project,
        archived_task,
        oldest_recent_project,
        oldest_recent_task,
    }
}

#[test]
fn restoring_archived_project_preserves_the_selected_project_and_tasks() {
    let ArchivedProjectFixture {
        mut presenter,
        _directory,
        archived_project,
        archived_task,
        oldest_recent_project,
        oldest_recent_task,
    } = archived_project_fixture();
    assert_eq!(presenter.model().archived_tasks[0].id, archived_task);
    let archived_project_metadata = presenter
        .model()
        .projects
        .iter()
        .find(|project| project.id == archived_project.id)
        .unwrap();
    assert_eq!(archived_project_metadata.display_name, "archived-project");
    presenter.select_project(oldest_recent_project.clone());
    presenter.select_task(oldest_recent_task);

    assert!(presenter.restore_task(archived_task));
    assert!(presenter.model().archived_tasks.is_empty());
    assert_eq!(
        presenter.model().selected_project.as_ref().unwrap().id,
        oldest_recent_project.id
    );
    assert_eq!(presenter.model().selected_task, Some(oldest_recent_task));
    assert!(
        presenter
            .model()
            .tasks
            .iter()
            .any(|task| task.id == oldest_recent_task)
    );
    assert!(
        presenter
            .model()
            .projects
            .iter()
            .any(|project| project.id == oldest_recent_project.id)
    );
    let restored_project = presenter
        .model()
        .projects
        .iter()
        .find(|project| project.id == archived_project.id)
        .unwrap()
        .clone();
    presenter.select_project(restored_project);
    assert!(
        presenter
            .model()
            .tasks
            .iter()
            .any(|task| task.id == archived_task)
    );
}

#[test]
fn deleting_archived_tasks_refreshes_projects_for_individual_and_bulk_actions() {
    for delete_all in [false, true] {
        let ArchivedProjectFixture {
            mut presenter,
            _directory,
            archived_project,
            archived_task,
            ..
        } = archived_project_fixture();
        assert!(
            presenter
                .model()
                .projects
                .iter()
                .any(|project| project.id == archived_project.id)
        );

        if delete_all {
            assert!(presenter.delete_archived_tasks());
        } else {
            assert!(presenter.delete_task(archived_task));
        }

        assert!(presenter.model().archived_tasks.is_empty());
        assert_eq!(presenter.model().projects.len(), 20);
        assert!(
            presenter
                .model()
                .projects
                .iter()
                .all(|project| project.id != archived_project.id)
        );
    }
}

#[test]
fn tool_events_preserve_ids_and_full_payloads_after_reloading_a_task() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("run tools", "claude"));
    let run_id = presenter.model().active_run.unwrap();
    let task_id = presenter.model().active_task.unwrap();
    let long_text = "完整内容\n".repeat(100);
    for tool_id in ["a", "b"] {
        runner.emit(Event::RunToolStarted {
            run_id,
            tool_id: tool_id.into(),
            name: "Bash".into(),
            summary: long_text.clone(),
        });
    }
    for tool_id in ["b", "a"] {
        runner.emit(Event::RunToolCompleted {
            run_id,
            tool_id: tool_id.into(),
            output: long_text.clone(),
            is_error: tool_id == "b",
        });
    }
    presenter.drain_events();
    presenter.select_task(task_id);
    let messages = &presenter.model().messages;
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[1].tool.as_ref().unwrap().id, "a");
    assert_eq!(messages[2].tool.as_ref().unwrap().id, "b");
    assert_eq!(messages[3].tool.as_ref().unwrap().id, "b");
    assert!(messages[3].tool.as_ref().unwrap().is_error);
    assert!(!messages[4].tool.as_ref().unwrap().is_error);
    assert_eq!(messages[1].content, format!("Bash\n{long_text}"));
    assert_eq!(messages[4].content, long_text);
    let items = crate::model::tools::timeline_items(messages);
    let crate::model::tools::TimelineItem::Tools(batch) = &items[1] else {
        panic!("expected one tool batch")
    };
    assert_eq!(items.len(), 2);
    assert_eq!(batch.len(), 2);
    assert_eq!(batch[0].result.unwrap().id, messages[4].id);
    assert_eq!(batch[1].result.unwrap().id, messages[3].id);
    assert!(!batch[0].is_error());
    assert!(batch[1].is_error());
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

fn provider_fixture() -> (
    Presenter,
    FakeRunner,
    FakeCredentialStore,
    tempfile::TempDir,
) {
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
    runner.0.borrow_mut().commands.clear();
    (presenter, runner, credentials, directory)
}

fn catalog_model(
    id: &str,
    is_default: bool,
    supported_efforts: &[ThinkingEffort],
    default_effort: ThinkingEffort,
) -> ModelDescriptor {
    catalog_model_with_provider(
        None,
        id,
        id,
        is_default,
        supported_efforts,
        Some(default_effort),
    )
}

fn catalog_model_with_provider(
    provider: Option<&str>,
    id: &str,
    display_name: &str,
    is_default: bool,
    supported_efforts: &[ThinkingEffort],
    default_effort: Option<ThinkingEffort>,
) -> ModelDescriptor {
    ModelDescriptor {
        id: id.into(),
        display_name: display_name.into(),
        provider: provider.map(str::to_owned),
        is_default,
        supported_reasoning_efforts: supported_efforts
            .iter()
            .map(|effort| ModelReasoningEffort {
                effort: *effort,
                description: effort.to_string(),
            })
            .collect(),
        default_reasoning_effort: default_effort,
    }
}

fn current_catalog_request_id(presenter: &Presenter) -> Uuid {
    let ModelCatalogState::Loading { request_id } = &presenter.model().model_catalog else {
        panic!("expected a loading model catalog")
    };
    *request_id
}

fn emit_current_catalog(presenter: &Presenter, runner: &FakeRunner, models: Vec<ModelDescriptor>) {
    runner.emit(Event::ModelCatalogLoaded {
        request_id: current_catalog_request_id(presenter),
        harness: presenter.model().selected_harness,
        models,
    });
}

fn last_start(runner: &FakeRunner) -> StartRun {
    runner
        .0
        .borrow()
        .commands
        .iter()
        .rev()
        .find_map(|envelope| match &envelope.command {
            Command::RunStart(request) => Some(request.clone()),
            _ => None,
        })
        .expect("expected start")
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
        ("codex_effort_cli", "high"),
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
fn codex_catalog_lifecycle_ignores_stale_responses_and_supports_retry() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.select_harness(HarnessKind::Codex, "claude"));
    let stale_request_id = current_catalog_request_id(&presenter);

    assert!(presenter.refresh_model_catalog());
    let failed_request_id = current_catalog_request_id(&presenter);
    assert_ne!(stale_request_id, failed_request_id);
    runner.emit(Event::ModelCatalogLoaded {
        request_id: stale_request_id,
        harness: HarnessKind::Codex,
        models: vec![catalog_model(
            "stale-model",
            true,
            &[ThinkingEffort::Low],
            ThinkingEffort::Low,
        )],
    });
    assert!(presenter.drain_events());
    assert!(matches!(
        presenter.model().model_catalog,
        ModelCatalogState::Loading { request_id } if request_id == failed_request_id
    ));

    runner.emit(Event::ModelCatalogFailed {
        request_id: failed_request_id,
        harness: HarnessKind::Codex,
        message: "model/list unavailable".into(),
    });
    presenter.drain_events();
    assert!(matches!(
        &presenter.model().model_catalog,
        ModelCatalogState::Failed(message) if message == "model/list unavailable"
    ));
    assert!(presenter.model().status.contains("model/list unavailable"));

    assert!(presenter.refresh_model_catalog());
    emit_current_catalog(&presenter, &runner, Vec::new());
    presenter.drain_events();
    assert!(matches!(
        presenter.model().model_catalog,
        ModelCatalogState::Empty
    ));
    assert!(presenter.model().status.contains("目录为空"));

    assert!(presenter.refresh_model_catalog());
    emit_current_catalog(
        &presenter,
        &runner,
        vec![catalog_model(
            "codex-current",
            true,
            &[ThinkingEffort::Low, ThinkingEffort::High],
            ThinkingEffort::High,
        )],
    );
    presenter.drain_events();
    let ModelCatalogState::Ready(models) = &presenter.model().model_catalog else {
        panic!("expected a ready model catalog")
    };
    assert_eq!(models[0].id, "codex-current");
    assert_eq!(presenter.model().status, "已加载 1 个 Codex CLI 模型。");
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
    assert!(presenter.active_run_started_at.is_none());
    assert!(presenter.model().active_run_elapsed_seconds.is_none());
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
    let task_id = presenter.model().selected_task.unwrap();
    let run_id = presenter.model().active_run.unwrap();
    runner.emit(Event::RunSessionStarted {
        run_id,
        session_id: "desktop-session".into(),
    });
    runner.emit(Event::RunExited {
        run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();

    // Remote StartRun remains an explicit new-task request, even with a task selected.
    let (reply, mut response) = tokio::sync::oneshot::channel();
    presenter.handle_remote_command(RemoteCommand::StartRun {
        project_id,
        prompt: " ".into(),
        reply,
    });
    assert!(response.try_recv().unwrap().is_err());
    assert_eq!(presenter.model().selected_task, Some(task_id));
    let (reply, mut response) = tokio::sync::oneshot::channel();
    presenter.handle_remote_command(RemoteCommand::StartRun {
        project_id,
        prompt: "new remote task".into(),
        reply,
    });
    assert_eq!(response.try_recv().unwrap(), Ok(()));
    assert_ne!(presenter.model().selected_task, Some(task_id));
    assert_eq!(presenter.storage.tasks(project_id).unwrap().len(), 2);
    let state = runner.0.borrow();
    let Command::RunStart(request) = &state.commands.last().unwrap().command else {
        panic!("expected new remote task");
    };
    assert!(request.session_id.is_none());
}

#[test]
fn submit_persists_configuration_and_queues_without_starting_concurrent_runs() {
    let (mut presenter, runner, _directory) = fixture();
    presenter.select_model(ClaudeModel::Opus);
    presenter.select_effort(ThinkingEffort::XHigh);
    assert!(presenter.model().can_submit());
    assert!(presenter.submit("  explain this project\n", "claude-custom"));
    assert!(!presenter.model().can_submit());
    assert!(presenter.submit("follow-up", "claude-custom"));
    assert_eq!(presenter.model().queued_messages.len(), 1);
    assert_eq!(presenter.model().queued_messages[0].prompt, "follow-up");
    let state = runner.0.borrow();
    assert_eq!(state.commands.len(), 1);
    let Command::RunStart(request) = &state.commands[0].command else {
        panic!("expected start");
    };
    assert_eq!(request.prompt, "explain this project");
    assert!(request.session_id.is_none());
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
fn queued_messages_start_one_at_a_time_and_resume_the_same_session() {
    for harness in HarnessKind::ALL {
        let (mut presenter, runner, _directory) = fixture();
        presenter.select_harness(harness, "claude");
        presenter
            .model
            .harnesses
            .insert(harness, ready_probe(harness));
        assert!(presenter.submit("first", harness.default_executable()));
        let task_id = presenter.model().active_task.unwrap();
        let mut run_id = presenter.model().active_run.unwrap();
        assert!(presenter.submit(" second ", harness.default_executable()));
        assert!(presenter.submit("third", harness.default_executable()));
        assert_eq!(presenter.model().messages.len(), 1);
        assert!(!presenter.submit(" \n ", harness.default_executable()));
        runner.emit(Event::RunSessionStarted {
            run_id,
            session_id: "queued-session".into(),
        });
        for (prompt, remaining) in [("second", 1), ("third", 0)] {
            runner.emit(Event::RunExited {
                run_id,
                status: RunStatus::Completed,
                exit_code: Some(0),
            });
            presenter.drain_events();
            let request = last_start(&runner);
            assert_ne!(request.run_id, run_id);
            assert_eq!(request.task_id, task_id);
            assert_eq!(request.session_id.as_deref(), Some("queued-session"));
            assert_eq!(request.prompt, prompt);
            assert_eq!(request.harness, harness);
            assert_eq!(presenter.model().queued_messages.len(), remaining);
            run_id = request.run_id;
        }
        runner.emit(Event::RunExited {
            run_id,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        assert!(presenter.model().active_run.is_none());
        assert_eq!(presenter.model().tasks.len(), 1);
        assert_eq!(
            presenter
                .model()
                .messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", "third"]
        );
    }
}

#[test]
fn failed_or_cancelled_runs_keep_the_queue_for_explicit_retry() {
    for status in [
        RunStatus::Failed,
        RunStatus::Cancelled,
        RunStatus::Interrupted,
        RunStatus::Completed,
    ] {
        let (mut presenter, runner, _directory) = fixture();
        assert!(presenter.submit("first", "claude"));
        let run_id = presenter.model().active_run.unwrap();
        let task_id = presenter.model().active_task.unwrap();
        assert!(presenter.submit("keep me", "claude"));
        assert!(presenter.submit("remove me", "claude"));
        let message_id = presenter.model().queued_messages[0].id;
        presenter.remove_queued_message(presenter.model().queued_messages[1].id);
        if status == RunStatus::Completed {
            // Stop can race with a successful exit; it must still pause the queue.
            presenter.cancel();
            assert!(!presenter.submit("too late", "claude"));
        }
        runner.emit(Event::RunSessionStarted {
            run_id,
            session_id: "saved-session".into(),
        });
        runner.emit(Event::RunExited {
            run_id,
            status,
            exit_code: None,
        });
        presenter.drain_events();
        assert!(presenter.model().active_run.is_none());
        assert_eq!(presenter.model().queued_messages.len(), 1);
        presenter.new_task();
        assert!(!presenter.send_queued_message(message_id));
        presenter.select_task(task_id);
        assert!(presenter.send_queued_message(message_id));
        assert_eq!(last_start(&runner).prompt, "keep me");
        assert!(presenter.model().queued_messages.is_empty());
    }
}

#[test]
fn queued_message_survives_missing_session_and_runner_send_failure() {
    for missing_session in [true, false] {
        let (mut presenter, runner, _directory) = fixture();
        assert!(presenter.submit("first", "claude"));
        let run_id = presenter.model().active_run.unwrap();
        assert!(presenter.submit("keep me", "claude"));
        if !missing_session {
            runner.emit(Event::RunSessionStarted {
                run_id,
                session_id: "saved-session".into(),
            });
            runner.0.borrow_mut().fail_send = true;
        }
        runner.emit(Event::RunExited {
            run_id,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        assert!(presenter.model().active_run.is_none());
        assert_eq!(presenter.model().queued_messages[0].prompt, "keep me");
        assert!(presenter.model().status.contains(if missing_session {
            "无法继续对话"
        } else {
            "Runner 不可用"
        }));
        if !missing_session {
            let task_id = presenter.model().selected_task.unwrap();
            let message_id = presenter.model().queued_messages[0].id;
            for _ in 0..2 {
                let stored = presenter.storage.messages(task_id).unwrap();
                assert_eq!(
                    stored.len(),
                    1,
                    "failed queue attempts must not add user messages"
                );
                assert_eq!(stored[0].content, "first");
                assert_eq!(
                    presenter
                        .storage
                        .tasks(presenter.model().selected_project.as_ref().unwrap().id)
                        .unwrap()[0]
                        .status,
                    RunStatus::Completed
                );
                assert!(!presenter.send_queued_message(message_id));
                assert_eq!(presenter.model().queued_messages[0].id, message_id);
            }
            runner.0.borrow_mut().fail_send = false;
            assert!(presenter.send_queued_message(message_id));
            let request = last_start(&runner);
            assert_eq!(request.task_id, task_id);
            assert_eq!(request.session_id.as_deref(), Some("saved-session"));
            assert!(presenter.model().queued_messages.is_empty());
            presenter.select_task(task_id);
            assert_eq!(
                presenter
                    .model()
                    .messages
                    .iter()
                    .map(|message| message.content.as_str())
                    .collect::<Vec<_>>(),
                ["first", "keep me"]
            );
            assert_eq!(presenter.model().messages[1].sequence, 2);
            assert_eq!(
                runner
                    .0
                    .borrow()
                    .commands
                    .iter()
                    .filter(|command| matches!(command.command, Command::RunStart(_)))
                    .count(),
                2
            );
        }
    }
}

#[test]
fn steer_uses_the_active_run_and_dequeues_only_after_a_matching_receipt() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("first", "claude"));
    let run_id = presenter.model().active_run.unwrap();
    assert!(presenter.submit("ordinary queue", "claude"));
    assert!(presenter.submit("urgent correction", "claude"));
    let message_id = presenter.model().queued_messages[1].id;
    assert!(presenter.steer_queued_message(message_id));
    assert_eq!(presenter.model().queued_messages[0].id, message_id);
    assert_eq!(presenter.model().messages.len(), 1);
    assert_eq!(presenter.model().steering_message, Some(message_id));
    assert!(!presenter.steer_queued_message(message_id));
    presenter.remove_queued_message(message_id);
    assert_eq!(presenter.model().queued_messages.len(), 2);
    assert!(
        matches!(&runner.0.borrow().commands.last().unwrap().command,
        Command::RunSteer { run_id: id, message_id: received, prompt }
            if *id == run_id && *received == message_id && prompt == "urgent correction")
    );
    for event in [
        Event::RunInputAccepted {
            run_id: Uuid::new_v4(),
            message_id,
        },
        Event::RunInputAccepted {
            run_id,
            message_id: Uuid::new_v4(),
        },
    ] {
        runner.emit(event);
    }
    presenter.drain_events();
    assert_eq!(presenter.model().queued_messages.len(), 2);
    for _ in 0..2 {
        runner.emit(Event::RunInputAccepted { run_id, message_id });
    }
    presenter.drain_events();
    assert!(presenter.model().steering_message.is_none());
    assert_eq!(presenter.model().queued_messages.len(), 1);
    let user_messages = presenter
        .model()
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .collect::<Vec<_>>();
    assert_eq!(user_messages.len(), 2);
    assert_eq!(user_messages[1].content, "urgent correction");
    assert_eq!(user_messages[1].run_id, run_id);
    assert_eq!(
        runner
            .0
            .borrow()
            .commands
            .iter()
            .filter(|command| matches!(command.command, Command::RunStart(_)))
            .count(),
        1
    );
}

#[test]
fn accepted_steer_is_saved_without_changing_another_tasks_timeline() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("other conversation", "claude"));
    let other_task = presenter.model().active_task.unwrap();
    runner.emit(Event::RunExited {
        run_id: presenter.model().active_run.unwrap(),
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    presenter.new_task();

    assert!(presenter.submit("active conversation", "claude"));
    let task_id = presenter.model().active_task.unwrap();
    let run_id = presenter.model().active_run.unwrap();
    assert!(presenter.submit("correction for active conversation", "claude"));
    let message_id = presenter.model().queued_messages[0].id;
    assert!(presenter.steer_queued_message(message_id));
    presenter.select_task(other_task);
    let visible_message_ids = presenter
        .model()
        .messages
        .iter()
        .map(|message| message.id)
        .collect::<Vec<_>>();

    runner.emit(Event::RunInputAccepted { run_id, message_id });
    presenter.drain_events();

    assert!(presenter.model().queued_messages.is_empty());
    assert!(presenter.model().steering_message.is_none());
    assert_eq!(presenter.model().selected_task, Some(other_task));
    assert_eq!(
        presenter
            .model()
            .messages
            .iter()
            .map(|message| message.id)
            .collect::<Vec<_>>(),
        visible_message_ids
    );
    assert_eq!(presenter.storage.messages(other_task).unwrap().len(), 1);
    presenter.select_task(task_id);
    let messages = &presenter.model().messages;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].content, "correction for active conversation");
    assert_eq!(messages[1].task_id, task_id);
    assert_eq!(messages[1].run_id, run_id);
}

#[test]
fn rejected_steer_falls_back_to_the_next_turn_and_survives_cancellation() {
    for cancelled in [false, true] {
        let (mut presenter, runner, _directory) = fixture();
        assert!(presenter.submit("first", "claude"));
        let run_id = presenter.model().active_run.unwrap();
        assert!(presenter.submit("ordinary queue", "claude"));
        assert!(presenter.submit("correction", "claude"));
        let message_id = presenter.model().queued_messages[1].id;
        assert!(presenter.steer_queued_message(message_id));
        runner.emit(Event::RunSessionStarted {
            run_id,
            session_id: "saved-session".into(),
        });
        if cancelled {
            presenter.cancel();
        }
        runner.emit(Event::RunInputRejected {
            run_id,
            message_id,
            message: "turn ended".into(),
        });
        presenter.drain_events();
        assert_eq!(presenter.model().queued_messages.len(), 2);
        assert!(presenter.model().steering_message.is_none());
        runner.emit(Event::RunExited {
            run_id,
            status: if cancelled {
                RunStatus::Cancelled
            } else {
                RunStatus::Completed
            },
            exit_code: None,
        });
        presenter.drain_events();
        if cancelled {
            assert_eq!(presenter.model().queued_messages.len(), 2);
            assert!(presenter.model().active_run.is_none());
        } else {
            let next = last_start(&runner);
            assert_ne!(next.run_id, run_id);
            assert_eq!(next.prompt, "correction");
            assert_eq!(next.session_id.as_deref(), Some("saved-session"));
            assert_eq!(
                presenter.model().queued_messages[0].prompt,
                "ordinary queue"
            );
        }
        runner.emit(Event::RunInputAccepted { run_id, message_id });
        presenter.drain_events();
        assert_eq!(
            presenter.model().queued_messages.len(),
            if cancelled { 2 } else { 1 }
        );
    }
}

#[test]
fn steer_send_failure_preserves_queue_order_and_stopping_blocks_steer() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("first", "claude"));
    assert!(presenter.submit("second", "claude"));
    assert!(presenter.submit("third", "claude"));
    let message_id = presenter.model().queued_messages[1].id;
    runner.0.borrow_mut().fail_send = true;
    assert!(!presenter.steer_queued_message(message_id));
    assert_eq!(presenter.model().queued_messages[0].prompt, "second");
    assert!(presenter.model().steering_message.is_none());
    runner.0.borrow_mut().fail_send = false;
    presenter.cancel();
    assert!(!presenter.steer_queued_message(message_id));
}

#[test]
fn follow_up_resumes_the_saved_session_after_reopening_and_new_task_starts_fresh() {
    for harness in HarnessKind::ALL {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("nexus.db");
        let runner = FakeRunner::default();
        let mut presenter = Presenter::new(
            Storage::open(&database).unwrap(),
            Ok(Box::new(runner.clone())),
            None,
        );
        presenter.open_project(directory.path());
        presenter.select_harness(harness, "claude");
        presenter
            .model
            .harnesses
            .insert(harness, ready_probe(harness));
        assert!(presenter.submit("first question", harness.default_executable()));
        let task_id = presenter.model().active_task.unwrap();
        let first_run = presenter.model().active_run.unwrap();
        let first_message = presenter.model().messages[0].id;
        runner.emit(Event::RunSessionStarted {
            run_id: first_run,
            session_id: "saved-session".into(),
        });
        runner.emit(Event::RunMessageCompleted {
            run_id: first_run,
            text: "first answer".into(),
        });
        runner.emit(Event::RunExited {
            run_id: first_run,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        drop(presenter);

        let mut presenter = Presenter::new(
            Storage::open(&database).unwrap(),
            Ok(Box::new(runner.clone())),
            None,
        );
        presenter.open_project(directory.path());
        let saved_probe = ready_probe(harness);
        let other_probe = HarnessProbe {
            executable: format!("/other/{harness}"),
            ..saved_probe.clone()
        };
        presenter
            .model
            .harnesses
            .insert(harness, other_probe.clone());
        runner.0.borrow_mut().commands.clear();
        presenter.select_task(task_id);
        assert!(!presenter.submit("follow-up before probe", &saved_probe.executable));
        assert!(!presenter.model().can_submit());
        assert_eq!(presenter.model().executable, saved_probe.executable);
        assert!(runner.0.borrow().commands.iter().any(|command| matches!(
            &command.command, Command::HarnessProbe { harness: probed_harness, executable }
                if *probed_harness == harness && executable == &saved_probe.executable
        )));
        if harness == HarnessKind::Codex {
            assert!(runner.0.borrow().commands.iter().any(|command| matches!(
                &command.command, Command::ModelCatalogRefresh { executable, .. }
                    if executable == &saved_probe.executable
            )));
        }
        // A late result for another executable must not authorize this session.
        runner.emit(Event::HarnessDetected(other_probe));
        presenter.drain_events();
        assert!(!presenter.submit("follow-up with stale probe", &saved_probe.executable));
        assert!(presenter.model().status.contains("可执行文件不一致"));
        assert!(
            runner
                .0
                .borrow()
                .commands
                .iter()
                .all(|command| !matches!(command.command, Command::RunStart(_)))
        );
        assert_eq!(presenter.model().messages.len(), 2);

        runner.emit(Event::HarnessDetected(saved_probe.clone()));
        presenter.drain_events();
        assert!(presenter.model().can_submit());
        assert!(presenter.submit("  follow-up  ", harness.default_executable()));
        let second_run = presenter.model().active_run.unwrap();
        assert_ne!(first_run, second_run);
        assert_eq!(presenter.model().selected_task, Some(task_id));
        assert_eq!(presenter.model().tasks.len(), 1);
        assert_eq!(presenter.model().tasks[0].title, "first question");
        assert_eq!(presenter.model().messages[0].id, first_message);
        assert_eq!(
            presenter
                .model()
                .messages
                .iter()
                .map(|message| (message.sequence, message.content.as_str(), message.run_id))
                .collect::<Vec<_>>(),
            vec![
                (1, "first question", first_run),
                (2, "first answer", first_run),
                (3, "follow-up", second_run)
            ]
        );
        {
            let state = runner.0.borrow();
            let Command::RunStart(request) = &state.commands.last().unwrap().command else {
                panic!("expected follow-up run");
            };
            assert_eq!(request.task_id, task_id);
            assert_eq!(request.session_id.as_deref(), Some("saved-session"));
            assert_eq!(request.prompt, "follow-up");
            assert_eq!(request.harness, harness);
            assert_eq!(request.executable, saved_probe.executable);
        }
        // A failed continuation must not lose the saved session.
        runner.emit(Event::RunExited {
            run_id: second_run,
            status: RunStatus::Failed,
            exit_code: Some(1),
        });
        presenter.drain_events();
        assert!(presenter.submit("retry", harness.default_executable()));
        let retry_run = presenter.model().active_run.unwrap();
        {
            let state = runner.0.borrow();
            let Command::RunStart(request) = &state.commands.last().unwrap().command else {
                panic!("expected retry run");
            };
            assert_eq!(request.task_id, task_id);
            assert_eq!(request.session_id.as_deref(), Some("saved-session"));
        }
        runner.emit(Event::RunExited {
            run_id: retry_run,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        presenter.new_task();
        assert!(presenter.submit("new question", harness.default_executable()));
        let state = runner.0.borrow();
        let Command::RunStart(request) = &state.commands.last().unwrap().command else {
            panic!("expected new task");
        };
        assert_ne!(request.task_id, task_id);
        assert!(request.session_id.is_none());
        assert_eq!(presenter.model().tasks.len(), 2);
        assert_eq!(presenter.model().messages.len(), 1);
    }
}

#[test]
fn follow_up_does_not_silently_restart_when_the_session_is_missing_or_harness_changes() {
    for missing_session in [true, false] {
        let (mut presenter, runner, _directory) = fixture();
        assert!(presenter.submit("first question", "claude"));
        let run_id = presenter.model().active_run.unwrap();
        if !missing_session {
            runner.emit(Event::RunSessionStarted {
                run_id,
                session_id: "claude-session".into(),
            });
        }
        runner.emit(Event::RunExited {
            run_id,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        if !missing_session {
            presenter.select_harness(HarnessKind::Codex, "claude");
            presenter
                .model
                .harnesses
                .insert(HarnessKind::Codex, ready_probe(HarnessKind::Codex));
        }
        runner.0.borrow_mut().commands.clear();
        assert!(!presenter.submit("follow-up", "configured-cli"));
        assert!(runner.0.borrow().commands.is_empty());
        assert!(presenter.model().active_run.is_none());
        assert_eq!(presenter.model().messages.len(), 1);
        assert_eq!(presenter.model().tasks.len(), 1);
        assert!(presenter.model().status.contains(if missing_session {
            "未保存可恢复的会话"
        } else {
            "切回该 Harness"
        }));
    }
}

#[test]
fn send_failure_rolls_back_a_new_task_without_entering_busy_state() {
    let (mut presenter, runner, _directory) = fixture();
    runner.0.borrow_mut().fail_send = true;
    assert!(!presenter.submit("hello", "claude"));
    assert!(presenter.model().active_run.is_none());
    assert!(presenter.active_run_started_at.is_none());
    assert!(presenter.model().active_run_elapsed_seconds.is_none());
    assert_eq!(presenter.model().status, "Runner 不可用，任务未启动。");
    let project_id = presenter.model().selected_project.as_ref().unwrap().id;
    assert!(presenter.storage.tasks(project_id).unwrap().is_empty());
    runner.0.borrow_mut().fail_send = false;
    assert!(presenter.submit("hello", "claude"));
    assert_eq!(presenter.storage.tasks(project_id).unwrap().len(), 1);
    assert_eq!(presenter.model().messages.len(), 1);
    assert_eq!(presenter.model().messages[0].content, "hello");
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
        let started = presenter.active_run_started_at.unwrap();
        assert!(presenter.refresh_run_elapsed(started + Duration::from_secs(5)));
        let run_id = presenter.model().active_run.unwrap();
        let task_id = presenter.model().active_task.unwrap();
        runner.emit(Event::RunStarted { run_id, pid: 42 });
        runner.emit(Event::RunSessionStarted {
            run_id,
            session_id: "saved-session".into(),
        });
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
        assert!(presenter.active_run_started_at.is_none());
        assert!(presenter.model().active_run_elapsed_seconds.is_none());
        assert!(!presenter.refresh_run_elapsed(started + Duration::from_secs(10)));
        assert_eq!(presenter.model().tasks[0].status, status);
        let messages = presenter.storage.messages(task_id).unwrap();
        assert_eq!(messages[1].content, "answer");
        assert_eq!(messages[1].sequence, 2);
        if status == RunStatus::Failed {
            assert_eq!(messages[2].kind, MessageKind::Error);
        }
        assert!(presenter.submit("next task", "claude"));
        assert_eq!(presenter.model().selected_task, Some(task_id));
        assert_eq!(presenter.model().tasks.len(), 1);
        assert_eq!(presenter.model().active_run_elapsed_seconds, Some(0));
        assert!(presenter.active_run_started_at.unwrap() >= started);
    }
}

#[test]
fn run_elapsed_advances_without_output_and_only_changes_each_second() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(!presenter.refresh_run_elapsed(Instant::now()));
    assert!(presenter.submit("hello", "claude"));
    let started = presenter.active_run_started_at.unwrap();
    let run_id = presenter.model().active_run.unwrap();
    assert_eq!(presenter.model().active_run_elapsed_seconds, Some(0));
    assert!(!presenter.refresh_run_elapsed(started + Duration::from_millis(999)));
    assert!(!presenter.drain_events());
    assert!(presenter.refresh_run_elapsed(started + Duration::from_secs(1)));
    assert_eq!(presenter.model().active_run_elapsed_seconds, Some(1));
    assert!(!presenter.refresh_run_elapsed(started + Duration::from_millis(1999)));

    runner.emit(Event::RunStarted { run_id, pid: 42 });
    runner.emit(Event::RunOutputDelta {
        run_id,
        text: "partial".into(),
    });
    assert!(presenter.drain_events());
    assert_eq!(presenter.active_run_started_at, Some(started));
    assert!(presenter.refresh_run_elapsed(started + Duration::from_secs(65)));
    assert_eq!(presenter.model().active_run_elapsed_seconds, Some(65));

    presenter.cancel();
    assert!(presenter.refresh_run_elapsed(started + Duration::from_secs(66)));
    assert_eq!(presenter.model().active_run_elapsed_seconds, Some(66));
    assert_eq!(presenter.active_run_started_at, Some(started));
    assert!(presenter.model().active_run.is_some());
}

#[test]
fn unrelated_run_events_cannot_replace_the_active_run() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("hello", "claude"));
    let active_run = presenter.model().active_run;
    let started = presenter.active_run_started_at;
    let other_run = Uuid::new_v4();
    runner.emit(Event::RunStarted {
        run_id: other_run,
        pid: 42,
    });
    runner.emit(Event::RunSessionStarted {
        run_id: other_run,
        session_id: "unrelated".into(),
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
    assert_eq!(presenter.active_run_started_at, started);
    assert_eq!(presenter.model().active_run_elapsed_seconds, Some(0));
    assert_eq!(presenter.model().messages.len(), 1);
    assert!(presenter.model().streaming_text.is_empty());
    assert!(
        presenter
            .storage
            .conversation_config(presenter.model().active_task.unwrap())
            .unwrap()
            .unwrap()
            .session_id
            .is_none()
    );
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
    assert_eq!(request.effort, ThinkingEffort::Default);
    let config = presenter
        .storage
        .conversation_config(request.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(config.model, "default");
    assert_eq!(config.effort, ThinkingEffort::Default);
}

#[test]
fn codex_selection_priority_follow_default_and_run_configuration_match() {
    let (mut presenter, runner, _credentials, _directory) = provider_fixture();
    assert!(presenter.select_harness(HarnessKind::Codex, "claude"));
    presenter
        .model
        .harnesses
        .insert(HarnessKind::Codex, ready_probe(HarnessKind::Codex));
    let mut draft = profile_draft(None, "Codex Profile", "profile-secret");
    draft.model = "profile-model".into();
    let profile_id = presenter.save_provider_profile(draft).unwrap();
    emit_current_catalog(
        &presenter,
        &runner,
        vec![
            catalog_model(
                "profile-model",
                true,
                &[ThinkingEffort::Medium],
                ThinkingEffort::Medium,
            ),
            catalog_model(
                "explicit-model",
                false,
                &[ThinkingEffort::Low, ThinkingEffort::High],
                ThinkingEffort::High,
            ),
        ],
    );
    presenter.drain_events();
    assert_eq!(
        presenter.model().configured_catalog_model(),
        Some("profile-model")
    );

    presenter.select_catalog_model(Some("explicit-model".into()));
    assert_eq!(
        presenter.model().configured_catalog_model(),
        Some("explicit-model")
    );
    assert_eq!(
        presenter
            .storage
            .setting(&catalog_model_setting_key(
                HarnessKind::Codex,
                Some(profile_id),
            ))
            .unwrap()
            .as_deref(),
        Some("explicit-model")
    );

    presenter.select_catalog_model(None);
    assert_eq!(
        presenter.model().configured_catalog_model(),
        Some("profile-model")
    );
    assert_eq!(
        presenter
            .storage
            .setting(&catalog_model_setting_key(
                HarnessKind::Codex,
                Some(profile_id),
            ))
            .unwrap()
            .as_deref(),
        Some("")
    );

    presenter.select_catalog_model(Some("explicit-model".into()));
    assert!(presenter.model().can_submit());
    assert!(presenter.submit("use catalog selection", "codex"));
    let state = runner.0.borrow();
    let request = state
        .commands
        .iter()
        .rev()
        .find_map(|envelope| match &envelope.command {
            Command::RunStart(request) => Some(request),
            _ => None,
        })
        .expect("expected start");
    assert_eq!(request.model.as_deref(), Some("explicit-model"));
    assert_eq!(request.effort, ThinkingEffort::High);
    let config = presenter
        .storage
        .conversation_config(request.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(config.model, "explicit-model");
    assert_eq!(config.effort, ThinkingEffort::High);
}

#[test]
fn codex_preferences_are_isolated_per_profile_and_invalid_effort_resets() {
    let (mut presenter, runner, _credentials, _directory) = provider_fixture();
    assert!(presenter.select_harness(HarnessKind::Codex, "claude"));
    let models = vec![
        catalog_model(
            "model-alpha",
            true,
            &[ThinkingEffort::High],
            ThinkingEffort::High,
        ),
        catalog_model(
            "model-beta",
            false,
            &[ThinkingEffort::Low],
            ThinkingEffort::Low,
        ),
    ];

    let mut first_draft = profile_draft(None, "First Codex", "first-secret");
    first_draft.model = "profile-first".into();
    let first_profile_id = presenter.save_provider_profile(first_draft).unwrap();
    emit_current_catalog(&presenter, &runner, models.clone());
    presenter.drain_events();
    presenter.select_catalog_model(Some("model-alpha".into()));
    presenter.select_effort(ThinkingEffort::High);

    let mut second_draft = profile_draft(None, "Second Codex", "second-secret");
    second_draft.model = "profile-second".into();
    let second_profile_id = presenter.save_provider_profile(second_draft).unwrap();
    emit_current_catalog(&presenter, &runner, models.clone());
    presenter.drain_events();
    assert!(presenter.model().model_override.is_none());
    assert_eq!(presenter.model().effort, ThinkingEffort::Default);
    presenter.select_catalog_model(Some("model-beta".into()));
    presenter.select_effort(ThinkingEffort::Low);

    assert!(presenter.select_provider_profile(Some(first_profile_id)));
    assert_eq!(
        presenter.model().model_override.as_deref(),
        Some("model-alpha")
    );
    assert_eq!(presenter.model().effort, ThinkingEffort::High);
    emit_current_catalog(&presenter, &runner, models);
    presenter.drain_events();
    assert_eq!(presenter.model().effort, ThinkingEffort::High);

    assert!(presenter.select_provider_profile(Some(second_profile_id)));
    assert_eq!(
        presenter.model().model_override.as_deref(),
        Some("model-beta")
    );
    assert_eq!(presenter.model().effort, ThinkingEffort::Low);
    emit_current_catalog(
        &presenter,
        &runner,
        vec![catalog_model(
            "model-beta",
            true,
            &[ThinkingEffort::High],
            ThinkingEffort::High,
        )],
    );
    presenter.drain_events();
    assert_eq!(presenter.model().effort, ThinkingEffort::Default);
    assert_eq!(
        presenter
            .storage
            .setting(&catalog_effort_setting_key(
                HarnessKind::Codex,
                Some(second_profile_id),
            ))
            .unwrap()
            .as_deref(),
        Some("default")
    );
    assert!(presenter.model().status.contains("恢复为模型默认"));
}

#[test]
fn omp_selection_preserves_full_selector_and_default_priority_in_run_configuration() {
    let (mut presenter, runner, _credentials, _directory) = provider_fixture();
    assert!(presenter.select_harness(HarnessKind::Omp, "claude"));
    presenter
        .model
        .harnesses
        .insert(HarnessKind::Omp, ready_probe(HarnessKind::Omp));
    let mut draft = profile_draft(None, "OMP Profile", "profile-secret");
    draft.model = "openai/shared-model".into();
    presenter.save_provider_profile(draft).unwrap();
    let models = vec![
        catalog_model_with_provider(
            Some("openai"),
            "openai/shared-model",
            "Shared Model",
            false,
            &[ThinkingEffort::Low],
            None,
        ),
        catalog_model_with_provider(
            Some("bigmodel"),
            "bigmodel/shared-model",
            "Shared Model",
            false,
            &[
                ThinkingEffort::Off,
                ThinkingEffort::Minimal,
                ThinkingEffort::XHigh,
                ThinkingEffort::Auto,
            ],
            None,
        ),
    ];
    emit_current_catalog(&presenter, &runner, models.clone());
    presenter.drain_events();

    assert_eq!(
        presenter.model().configured_catalog_model(),
        Some("openai/shared-model")
    );
    presenter.select_catalog_model(Some("bigmodel/shared-model".into()));
    assert_eq!(
        presenter
            .model()
            .selected_catalog_model()
            .and_then(|model| model.provider.as_deref()),
        Some("bigmodel")
    );
    presenter.select_effort(ThinkingEffort::XHigh);
    assert!(presenter.submit("use explicit OMP model", "omp"));
    let explicit = last_start(&runner);
    assert_eq!(explicit.model.as_deref(), Some("bigmodel/shared-model"));
    assert_eq!(explicit.effort, ThinkingEffort::XHigh);
    let config = presenter
        .storage
        .conversation_config(explicit.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(config.model, "bigmodel/shared-model");
    assert_eq!(config.effort, ThinkingEffort::XHigh);

    runner.emit(Event::RunExited {
        run_id: explicit.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    presenter.new_task();
    presenter.select_catalog_model(None);
    assert!(presenter.submit("use Profile default", "omp"));
    let profile_default = last_start(&runner);
    assert_eq!(
        profile_default.model.as_deref(),
        Some("openai/shared-model")
    );
    assert_eq!(profile_default.effort, ThinkingEffort::Default);

    runner.emit(Event::RunExited {
        run_id: profile_default.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    presenter.new_task();
    assert!(presenter.select_provider_profile(None));
    emit_current_catalog(&presenter, &runner, models);
    presenter.drain_events();
    assert!(presenter.submit("use CLI default", "omp"));
    let cli_default = last_start(&runner);
    assert!(cli_default.model.is_none());
    let config = presenter
        .storage
        .conversation_config(cli_default.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(config.model, "default");
}

#[test]
fn catalog_preferences_are_isolated_by_harness_and_profile() {
    let (mut presenter, runner, _credentials, _directory) = provider_fixture();
    assert!(presenter.select_harness(HarnessKind::Codex, "claude"));
    let mut codex_draft = profile_draft(None, "Codex Profile", "codex-secret");
    codex_draft.model = String::new();
    let codex_profile_id = presenter.save_provider_profile(codex_draft).unwrap();
    emit_current_catalog(
        &presenter,
        &runner,
        vec![catalog_model(
            "codex-model",
            true,
            &[ThinkingEffort::High],
            ThinkingEffort::High,
        )],
    );
    presenter.drain_events();
    presenter.select_catalog_model(Some("codex-model".into()));
    presenter.select_effort(ThinkingEffort::High);

    assert!(presenter.select_harness(HarnessKind::Omp, "codex"));
    let mut omp_draft = profile_draft(None, "OMP Profile", "omp-secret");
    omp_draft.model = String::new();
    let omp_profile_id = presenter.save_provider_profile(omp_draft).unwrap();
    emit_current_catalog(
        &presenter,
        &runner,
        vec![catalog_model_with_provider(
            Some("bigmodel"),
            "bigmodel/omp-model",
            "OMP Model",
            false,
            &[ThinkingEffort::XHigh],
            None,
        )],
    );
    presenter.drain_events();
    assert!(presenter.model().model_override.is_none());
    presenter.select_catalog_model(Some("bigmodel/omp-model".into()));
    presenter.select_effort(ThinkingEffort::XHigh);

    assert!(presenter.select_harness(HarnessKind::Codex, "omp"));
    assert_eq!(
        presenter
            .model()
            .selected_provider_profile()
            .map(|profile| profile.id),
        Some(codex_profile_id)
    );
    assert_eq!(
        presenter.model().model_override.as_deref(),
        Some("codex-model")
    );
    assert_eq!(presenter.model().effort, ThinkingEffort::High);

    assert!(presenter.select_harness(HarnessKind::Omp, "codex"));
    assert_eq!(
        presenter
            .model()
            .selected_provider_profile()
            .map(|profile| profile.id),
        Some(omp_profile_id)
    );
    assert_eq!(
        presenter.model().model_override.as_deref(),
        Some("bigmodel/omp-model")
    );
    assert_eq!(presenter.model().effort, ThinkingEffort::XHigh);
}

#[test]
fn omp_legacy_efforts_migrate_to_their_actual_cli_values() {
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    storage.set_setting("omp_effort_cli", "max").unwrap();
    let (_, effort) =
        load_catalog_preferences(&storage, HarnessKind::Omp, None, ThinkingEffort::Default);
    assert_eq!(effort, ThinkingEffort::XHigh);
    assert_eq!(
        storage.setting("omp_effort_cli").unwrap().as_deref(),
        Some("xhigh")
    );

    let storage = Storage::open(Path::new(":memory:")).unwrap();
    let (_, effort) =
        load_catalog_preferences(&storage, HarnessKind::Omp, None, ThinkingEffort::None);
    assert_eq!(effort, ThinkingEffort::Off);
    assert_eq!(
        storage.setting("omp_effort_cli").unwrap().as_deref(),
        Some("off")
    );
}

#[test]
fn omp_catalog_ignores_a_response_from_the_previous_profile() {
    let (mut presenter, runner, _credentials, _directory) = provider_fixture();
    assert!(presenter.select_harness(HarnessKind::Omp, "claude"));
    let first_profile = presenter
        .save_provider_profile(profile_draft(None, "First OMP", "first-secret"))
        .unwrap();
    let stale_request_id = current_catalog_request_id(&presenter);

    let mut second_draft = profile_draft(None, "Second OMP", "second-secret");
    second_draft.model = "second/profile-model".into();
    let second_profile = presenter.save_provider_profile(second_draft).unwrap();
    let current_request_id = current_catalog_request_id(&presenter);
    assert_ne!(first_profile, second_profile);
    assert_ne!(stale_request_id, current_request_id);

    runner.emit(Event::ModelCatalogLoaded {
        request_id: stale_request_id,
        harness: HarnessKind::Omp,
        models: vec![catalog_model_with_provider(
            Some("stale"),
            "stale/model",
            "Stale Model",
            false,
            &[ThinkingEffort::Low],
            None,
        )],
    });
    presenter.drain_events();
    assert!(matches!(
        presenter.model().model_catalog,
        ModelCatalogState::Loading { request_id } if request_id == current_request_id
    ));

    emit_current_catalog(
        &presenter,
        &runner,
        vec![catalog_model_with_provider(
            Some("second"),
            "second/model",
            "Current Model",
            false,
            &[ThinkingEffort::Auto],
            None,
        )],
    );
    presenter.drain_events();
    let ModelCatalogState::Ready(models) = &presenter.model().model_catalog else {
        panic!("expected current catalog")
    };
    assert_eq!(models[0].id, "second/model");
}

#[test]
fn omp_profile_custom_model_is_preserved_when_catalog_availability_is_unknown() {
    let (mut presenter, runner, _credentials, _directory) = provider_fixture();
    assert!(presenter.select_harness(HarnessKind::Omp, "claude"));
    presenter.model.harnesses.insert(
        HarnessKind::Omp,
        HarnessProbe {
            authenticated: false,
            ..ready_probe(HarnessKind::Omp)
        },
    );
    let mut draft = profile_draft(None, "Custom OMP", "custom-secret");
    draft.model = "private-provider/custom-model".into();
    presenter.save_provider_profile(draft).unwrap();
    emit_current_catalog(
        &presenter,
        &runner,
        vec![catalog_model_with_provider(
            Some("public-provider"),
            "public-provider/custom-model",
            "Custom Model",
            false,
            &[ThinkingEffort::Medium],
            None,
        )],
    );
    presenter.drain_events();

    assert!(presenter.model().model_override.is_none());
    assert_eq!(
        presenter.model().configured_catalog_model(),
        Some("private-provider/custom-model")
    );
    assert!(presenter.model().selected_catalog_model().is_none());
    assert!(presenter.model().can_submit());
    assert!(presenter.model().status.contains("尚未验证可用"));
}

#[test]
fn invalid_codex_catalog_selection_cannot_be_submitted() {
    let (mut presenter, runner, _directory) = fixture();
    presenter
        .storage
        .set_setting("codex_model_override_cli", "removed-model")
        .unwrap();
    assert!(presenter.select_harness(HarnessKind::Codex, "claude"));
    presenter
        .model
        .harnesses
        .insert(HarnessKind::Codex, ready_probe(HarnessKind::Codex));
    emit_current_catalog(
        &presenter,
        &runner,
        vec![catalog_model(
            "current-model",
            true,
            &[ThinkingEffort::Medium],
            ThinkingEffort::Medium,
        )],
    );
    presenter.drain_events();
    runner.0.borrow_mut().commands.clear();

    assert!(!presenter.model().can_submit());
    assert!(!presenter.submit("must not start", "codex"));
    assert!(presenter.model().active_run.is_none());
    assert!(
        presenter
            .storage
            .tasks(presenter.model().selected_project.as_ref().unwrap().id)
            .unwrap()
            .is_empty()
    );
    assert!(runner.0.borrow().commands.is_empty());
    assert!(presenter.model().status.contains("未通过目录验证"));
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
    assert!(matches!(
        &state.commands[0].command,
        Command::ModelCatalogRefresh { .. }
    ));
    let request = state
        .commands
        .iter()
        .find_map(|envelope| match &envelope.command {
            Command::RunStart(request) => Some(request),
            _ => None,
        })
        .expect("expected start");
    assert_eq!(request.harness, HarnessKind::Omp);
    assert_eq!(request.model.as_deref(), Some("deepseek/deepseek-v4-pro"));
    assert_eq!(request.environment.len(), 1);
    assert_eq!(request.environment[0].name, "DEEPSEEK_API_KEY");
    assert_eq!(request.environment[0].value, "super-secret");
    assert_eq!(
        format!("{:?}", request.environment[0]),
        r#"EnvironmentVariable { name: "DEEPSEEK_API_KEY", value: "[REDACTED]" }"#
    );
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
