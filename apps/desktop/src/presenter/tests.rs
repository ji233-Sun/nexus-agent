use std::{cell::RefCell, collections::HashMap, fs, rc::Rc, time::Duration};

use super::*;
use crate::{
    infrastructure::{
        codex_history::Event as HistoryEvent, credentials::CredentialStore, storage::NewTaskRun,
    },
    model::{
        UserAskSubmissionState,
        history::{HistoryMessage, ThreadSummary},
    },
};
use nexus_domain::{
    MessageKind, MessageRole, ModelDescriptor, ModelReasoningEffort, RunStatus, UserAskAnswer,
    UserAskAnswerMode, UserAskAnswerValue, UserAskOption, UserAskQuestion, UserAskStatus,
};
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

    pub(crate) fn submitted_user_ask_answers(&self) -> Vec<Vec<UserAskAnswer>> {
        self.0
            .borrow()
            .commands
            .iter()
            .filter_map(|command| match &command.command {
                Command::RunUserAskAnswer { answers, .. } => Some(answers.clone()),
                _ => None,
            })
            .collect()
    }
}

pub(crate) fn fixture() -> (Presenter, FakeRunner, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    let runner = FakeRunner::default();
    let mut presenter = Presenter::new(storage, Ok(Box::new(runner.clone())), None);
    presenter.open_project(directory.path());
    presenter.model.model_catalog = ModelCatalogState::Ready(claude_aliases());
    presenter
        .model
        .harnesses
        .insert(HarnessKind::Claude, ready_probe(HarnessKind::Claude));
    runner.0.borrow_mut().commands.clear();
    (presenter, runner, directory)
}

pub(crate) fn pending_update(presenter: &mut Presenter) -> std::sync::mpsc::Sender<UpdateState> {
    let (sender, receiver) = std::sync::mpsc::channel();
    presenter.update_events = Some(receiver);
    presenter.model.updates.state = UpdateState::Checking;
    sender
}

#[test]
fn update_preferences_restore_and_invalid_values_follow_the_installed_channel() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("updates.sqlite");
    let mut presenter = Presenter::new(
        Storage::open(&path).unwrap(),
        Err(anyhow::anyhow!("test")),
        None,
    );
    assert_eq!(presenter.model().updates.channel, UpdateChannel::default());
    assert!(presenter.model().updates.check_on_startup);
    assert!(presenter.set_update_check_on_startup(false));
    for channel in [UpdateChannel::Nightly, UpdateChannel::Release] {
        assert!(presenter.set_update_channel(channel));
        drop(presenter);
        presenter = Presenter::new(
            Storage::open(&path).unwrap(),
            Err(anyhow::anyhow!("test")),
            None,
        );
        assert_eq!(presenter.model().updates.channel, channel);
        assert!(!presenter.model().updates.check_on_startup);
    }
    presenter
        .storage
        .set_setting("update_channel", "unknown")
        .unwrap();
    drop(presenter);
    let presenter = Presenter::new(
        Storage::open(&path).unwrap(),
        Err(anyhow::anyhow!("test")),
        None,
    );
    assert_eq!(presenter.model().updates.channel, UpdateChannel::default());
    assert!(!presenter.model().updates.check_on_startup);
}

#[test]
fn update_events_guard_concurrency_and_preserve_conversations_through_completion_and_failure() {
    let (mut presenter, _, directory) = fixture();
    assert!(presenter.submit("Keep this conversation running", "claude"));
    let task = presenter.model().selected_task;
    let run = presenter.model().active_run;
    let status = presenter.model().status.clone();
    let sender = pending_update(&mut presenter);
    let channel = presenter.model().updates.channel;
    assert!(!presenter.set_update_channel(UpdateChannel::Nightly));
    presenter.check_for_updates();
    sender
        .send(UpdateState::Downloading {
            tag: "v1.0.0".into(),
            received: 12,
            total: 24,
        })
        .unwrap();
    assert!(presenter.drain_update_events());
    assert_eq!(presenter.model().updates.channel, channel);
    assert!(
        presenter
            .model()
            .updates
            .state
            .message(Language::English)
            .contains("50%")
    );
    assert!(!presenter.drain_update_events());
    sender
        .send(UpdateState::Ready {
            tag: "v1.0.0".into(),
            path: directory.path().join("update.zip"),
        })
        .unwrap();
    assert!(presenter.drain_update_events());
    assert!(presenter.update_events.is_none());
    assert!(presenter.set_update_channel(channel));
    assert!(matches!(
        presenter.model().updates.state,
        UpdateState::Ready { .. }
    ));
    let sender = pending_update(&mut presenter);
    drop(sender);
    assert!(presenter.drain_update_events());
    assert!(matches!(
        presenter.model().updates.state,
        UpdateState::Failed(_)
    ));
    assert_eq!(presenter.model().selected_task, task);
    assert_eq!(presenter.model().active_run, run);
    assert_eq!(presenter.model().status, status);
    assert!(!presenter.drain_update_events());
}

#[test]
fn language_preferences_restore_and_fall_back_without_changing_appearance() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("language.sqlite");
    let mut presenter = Presenter::new(
        Storage::open(&path).unwrap(),
        Err(anyhow::anyhow!("test")),
        None,
    );
    assert_eq!(presenter.model().language, Language::Chinese);
    let appearance = AppearanceSettings {
        glass: false,
        ..Default::default()
    };
    assert!(presenter.set_appearance(appearance));
    for language in [Language::English, Language::Chinese] {
        assert!(presenter.set_language(language));
        assert_eq!(
            presenter.storage.setting("language").unwrap().as_deref(),
            Some(language.as_str())
        );
        drop(presenter);
        presenter = Presenter::new(
            Storage::open(&path).unwrap(),
            Err(anyhow::anyhow!("test")),
            None,
        );
        assert_eq!(presenter.model().language, language);
        assert_eq!(presenter.model().appearance, appearance);
    }
    for invalid in ["", "fr", "invalid json"] {
        presenter.storage.set_setting("language", invalid).unwrap();
        drop(presenter);
        presenter = Presenter::new(
            Storage::open(&path).unwrap(),
            Err(anyhow::anyhow!("test")),
            None,
        );
        assert_eq!(presenter.model().language, Language::Chinese);
    }
}

fn remote_status(presenter: &mut Presenter) -> String {
    let (reply, mut response) = tokio::sync::oneshot::channel();
    assert!(!presenter.handle_remote_command(RemoteCommand::GetState { reply }));
    response.try_recv().unwrap().status
}

#[test]
fn language_switch_updates_status_and_preserves_active_runs_and_remote_content() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(!presenter.submit(" ", "claude"));
    assert!(presenter.set_language(Language::English));
    assert_eq!(presenter.model().status_text(), "Prompt cannot be empty.");
    assert_eq!(remote_status(&mut presenter), "Prompt 不能为空。");
    assert!(presenter.submit("设置 {count} café", "claude"));
    let task_id = presenter.model().selected_task;
    let run_id = presenter.model().active_run;
    assert!(presenter.submit("待发送 {error}", "claude"));
    let command_count = runner.0.borrow().commands.len();
    let status_before = remote_status(&mut presenter);
    for language in [Language::Chinese, Language::English] {
        assert!(presenter.set_language(language));
        assert_eq!(presenter.model().selected_task, task_id);
        assert_eq!(presenter.model().active_run, run_id);
        assert_eq!(presenter.model().messages[0].content, "设置 {count} café");
        assert_eq!(
            presenter.model().queued_messages[0].prompt,
            "待发送 {error}"
        );
        assert_eq!(presenter.model().tasks[0].title, "设置 {count} café");
        assert_eq!(remote_status(&mut presenter), status_before);
        assert_eq!(runner.0.borrow().commands.len(), command_count);
    }
    runner.emit(Event::RunFailed {
        run_id: run_id.unwrap(),
        code: ErrorCode::UnexpectedExit,
        message: "原始诊断 {message}".into(),
    });
    presenter.drain_events();
    assert_eq!(presenter.model().status_text(), "原始诊断 {message}");
}

#[test]
fn language_translates_probe_summaries_and_default_effort_without_changing_cli_values() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.set_language(Language::English));
    for (available, authenticated, expected) in [
        (true, true, "Claude Code is ready."),
        (
            true,
            false,
            "Claude Code is not signed in. Sign in to the CLI or configure a provider profile.",
        ),
        (
            false,
            false,
            "Claude Code unavailable. Check its executable in settings.",
        ),
    ] {
        let mut probe = ready_probe(HarnessKind::Claude);
        probe.available = available;
        probe.authenticated = authenticated;
        probe.message = "原始探测诊断".into();
        runner.emit(Event::HarnessDetected(probe));
        presenter.drain_events();
        assert_eq!(presenter.model().status_text(), expected);
        assert_eq!(remote_status(&mut presenter), "原始探测诊断");
    }
    runner.emit(Event::HarnessDetected(ready_probe(HarnessKind::Claude)));
    presenter.drain_events();
    presenter.select_effort(ThinkingEffort::Default);
    assert!(presenter.submit("hello", "claude"));
    assert_eq!(
        presenter.model().status_text(),
        "Starting Claude Code · Model default"
    );
    assert_eq!(
        remote_status(&mut presenter),
        "正在启动 Claude Code · 模型默认"
    );
    assert!(runner.0.borrow().commands.iter().any(|command| matches!(
        &command.command,
        Command::RunStart(request) if request.effort == ThinkingEffort::Default && request.model.is_none()
    )));
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
                permission_mode: nexus_domain::PermissionMode::AutoEdit,
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
                permission_mode: nexus_domain::PermissionMode::AutoEdit,
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
            permission_mode: nexus_domain::PermissionMode::AutoEdit,
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
                    permission_mode: nexus_domain::PermissionMode::AutoEdit,
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
    presenter.model.model_catalog = ModelCatalogState::Ready(claude_aliases());
    runner.0.borrow_mut().commands.clear();
    (presenter, runner, credentials, directory)
}

fn claude_aliases() -> Vec<ModelDescriptor> {
    ["sonnet", "opus", "haiku"]
        .into_iter()
        .map(|id| ModelDescriptor {
            id: id.into(),
            display_name: id.into(),
            source: nexus_domain::ModelSource::ClaudeAliases,
            availability: nexus_domain::ModelAvailability::Unknown,
            provider: None,
            is_default: false,
            supported_reasoning_efforts: Vec::new(),
            default_reasoning_effort: None,
        })
        .collect()
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
        source: if provider.is_some() {
            nexus_domain::ModelSource::OmpCli
        } else {
            nexus_domain::ModelSource::CodexAppServer
        },
        availability: nexus_domain::ModelAvailability::Available,
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
    let ModelCatalogState::Loading { request_id, .. } = &presenter.model().model_catalog else {
        panic!("expected a loading model catalog")
    };
    *request_id
}

fn emit_current_catalog(
    presenter: &Presenter,
    runner: &FakeRunner,
    mut models: Vec<ModelDescriptor>,
) {
    for model in &mut models {
        model.source = match presenter.model().selected_harness {
            HarnessKind::Claude => nexus_domain::ModelSource::ClaudeAliases,
            HarnessKind::Codex => nexus_domain::ModelSource::CodexAppServer,
            HarnessKind::Omp => nexus_domain::ModelSource::OmpCli,
        };
    }
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

#[test]
fn permission_modes_are_snapshotted_per_turn_and_restored_per_harness_and_conversation() {
    let (mut presenter, runner, _directory) = fixture();
    presenter.select_permission_mode(PermissionMode::Ask);
    assert!(presenter.submit("first", "claude"));
    let first = last_start(&runner);
    assert_eq!(first.permission_mode, PermissionMode::Ask);
    runner.emit(Event::RunSessionStarted {
        run_id: first.run_id,
        session_id: "session".into(),
    });
    presenter.drain_events();
    presenter.select_permission_mode(PermissionMode::Yolo);
    assert!(presenter.submit("second", "claude"));
    let queued = presenter.model().queued_messages[0].id;
    assert!(!presenter.steer_queued_message(queued));
    presenter.select_permission_mode(PermissionMode::AutoEdit);
    assert!(presenter.submit("third", "claude"));
    presenter.select_permission_mode(PermissionMode::Ask);
    assert_eq!(last_start(&runner).permission_mode, PermissionMode::Ask);
    assert_eq!(
        presenter.model().active_permission_mode,
        Some(PermissionMode::Ask)
    );
    runner.emit(Event::RunExited {
        run_id: first.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    let second = last_start(&runner);
    assert_eq!(second.task_id, first.task_id);
    assert_eq!(second.permission_mode, PermissionMode::Yolo);
    assert_eq!(second.session_id.as_deref(), Some("session"));
    assert_eq!(
        presenter.model().active_permission_mode,
        Some(PermissionMode::Yolo)
    );
    assert_eq!(
        presenter.model().queued_messages[0].permission_mode,
        PermissionMode::AutoEdit
    );
    runner.emit(Event::RunExited {
        run_id: second.run_id,
        status: RunStatus::Failed,
        exit_code: Some(1),
    });
    presenter.drain_events();
    presenter.select_task(first.task_id);
    assert_eq!(presenter.model().permission_mode, PermissionMode::Yolo);
    let config = presenter
        .storage
        .conversation_config(first.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(config.permission_mode, PermissionMode::Yolo);
    presenter.new_task();
    assert_eq!(presenter.model().permission_mode, PermissionMode::Ask);
    assert!(presenter.select_harness(HarnessKind::Omp, "claude"));
    assert_eq!(presenter.model().permission_mode, PermissionMode::AutoEdit);
    presenter.select_permission_mode(PermissionMode::Yolo);
    assert!(presenter.select_harness(HarnessKind::Claude, "omp"));
    assert_eq!(presenter.model().permission_mode, PermissionMode::Ask);
    assert!(presenter.select_harness(HarnessKind::Omp, "claude"));
    assert_eq!(presenter.model().permission_mode, PermissionMode::Yolo);
}

#[test]
fn approvals_ignore_stale_events_validate_choices_and_remain_retryable_until_resolved() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("first", "claude"));
    let run_id = presenter.model().active_run.unwrap();
    let request = nexus_protocol::ApprovalRequest {
        request_id: Uuid::new_v4(),
        title: "Bash".into(),
        details: "echo test".into(),
        options: vec!["Approve".into(), "Deny".into()],
    };
    runner.emit(Event::RunApprovalRequested {
        run_id: Uuid::new_v4(),
        request: request.clone(),
    });
    presenter.drain_events();
    assert!(presenter.model().pending_approvals.is_empty());
    for _ in 0..2 {
        runner.emit(Event::RunApprovalRequested {
            run_id,
            request: request.clone(),
        });
    }
    let second = nexus_protocol::ApprovalRequest {
        request_id: Uuid::new_v4(),
        ..request.clone()
    };
    runner.emit(Event::RunApprovalRequested {
        run_id,
        request: second.clone(),
    });
    presenter.drain_events();
    assert_eq!(presenter.model().pending_approvals.len(), 2);
    assert!(!presenter.respond_approval(Uuid::new_v4(), request.request_id, Some(0)));
    assert!(!presenter.respond_approval(run_id, second.request_id, Some(0)));
    assert!(!presenter.respond_approval(run_id, request.request_id, Some(2)));
    runner.0.borrow_mut().fail_send = true;
    assert!(!presenter.respond_approval(run_id, request.request_id, Some(0)));
    assert!(presenter.model().responding_approval.is_none());
    runner.0.borrow_mut().fail_send = false;
    assert!(presenter.respond_approval(run_id, request.request_id, Some(1)));
    assert!(!presenter.respond_approval(run_id, request.request_id, Some(1)));
    assert_eq!(presenter.model().pending_approvals.len(), 2);
    assert!(matches!(runner.0.borrow().commands.last().unwrap().command,
        Command::RunApprovalRespond { run_id: run, request_id, option: Some(1) } if run == run_id && request_id == request.request_id));
    runner.emit(Event::RunApprovalResolved {
        run_id,
        request_id: request.request_id,
    });
    presenter.drain_events();
    assert_eq!(presenter.model().pending_approvals.len(), 1);
    assert!(presenter.respond_approval(run_id, second.request_id, None));
    presenter.cancel();
    assert!(presenter.model().pending_approvals.is_empty());
    assert!(presenter.model().responding_approval.is_none());
    runner.emit(Event::RunApprovalRequested {
        run_id,
        request: second,
    });
    presenter.drain_events();
    assert!(presenter.model().pending_approvals.is_empty());
    assert!(!presenter.respond_approval(run_id, request.request_id, Some(0)));
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
        ("permission_mode.codex", "yolo"),
    ] {
        storage.set_setting(key, value).unwrap();
    }
    let runner = FakeRunner::default();
    let presenter = Presenter::new(storage, Ok(Box::new(runner.clone())), None);

    assert_eq!(presenter.model().selected_harness, HarnessKind::Codex);
    assert!(presenter.model().model_override.is_none());
    assert_eq!(presenter.model().effort, ThinkingEffort::High);
    assert_eq!(presenter.model().permission_mode, PermissionMode::Yolo);
    assert_eq!(presenter.model().executable, "/custom/codex");
    let state = runner.0.borrow();
    assert_eq!(state.commands.len(), 4);
    assert!(matches!(state.commands[0].command, Command::RunnerHello));
    assert!(state.commands.iter().any(|command| matches!(&command.command,
        Command::HarnessProbe { harness: HarnessKind::Codex, executable } if executable == "/custom/codex")));
    assert!(!presenter.model().can_submit());
}

#[test]
fn catalog_lifecycle_ignores_stale_responses_and_supports_retry() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.select_harness(HarnessKind::Codex, "claude"));
    let stale_request_id = current_catalog_request_id(&presenter);

    assert!(presenter.refresh_model_catalog());
    let failed_request_id = current_catalog_request_id(&presenter);
    assert_ne!(stale_request_id, failed_request_id);
    runner.0.borrow_mut().events.push(EventEnvelope {
        protocol_version: PROTOCOL_VERSION - 1,
        id: Uuid::new_v4(),
        sequence: 1,
        event: Event::ModelCatalogLoaded {
            request_id: failed_request_id,
            harness: HarnessKind::Codex,
            models: vec![],
        },
    });
    presenter.drain_events();
    assert_eq!(current_catalog_request_id(&presenter), failed_request_id);
    assert!(presenter.model().status_text().contains("协议版本不匹配"));
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
        ModelCatalogState::Loading { request_id, .. } if request_id == failed_request_id
    ));

    runner.emit(Event::ModelCatalogFailed {
        request_id: failed_request_id,
        harness: HarnessKind::Codex,
        message: "model/list unavailable".into(),
    });
    presenter.drain_events();
    assert!(matches!(
        &presenter.model().model_catalog,
        ModelCatalogState::Failed { message, .. } if message.render(Language::Chinese) == "model/list unavailable"
    ));
    assert!(
        presenter
            .model()
            .status_text()
            .contains("model/list unavailable")
    );

    assert!(presenter.refresh_model_catalog());
    emit_current_catalog(&presenter, &runner, Vec::new());
    presenter.drain_events();
    assert!(matches!(
        presenter.model().model_catalog,
        ModelCatalogState::Empty
    ));
    assert!(presenter.model().status_text().contains("目录为空"));

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
    assert_eq!(
        presenter.model().status_text(),
        "已加载 1 个 Codex CLI 模型。"
    );

    // Claude exercises the shared readiness state without starting a Codex history client.
    assert!(presenter.select_harness(HarnessKind::Claude, "codex"));
    let request_id = current_catalog_request_id(&presenter);
    runner.emit(Event::HarnessDetected(HarnessProbe {
        available: false,
        ..ready_probe(HarnessKind::Claude)
    }));
    runner.emit(Event::ModelCatalogFailed {
        request_id,
        harness: HarnessKind::Claude,
        message: "late catalog failure".into(),
    });
    presenter.drain_events();
    assert!(matches!(
        presenter.model().model_catalog,
        ModelCatalogState::NotReady(_)
    ));
    runner.emit(Event::HarnessDetected(ready_probe(HarnessKind::Claude)));
    presenter.drain_events();
    assert_ne!(current_catalog_request_id(&presenter), request_id);
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
    assert_eq!(presenter.model().status_text(), "请先选择项目目录。");
    presenter.model.selected_project = Some(project.clone());
    assert!(!presenter.submit(" \n ", "claude"));
    assert_eq!(presenter.model().status_text(), "Prompt 不能为空。");
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
    assert!(presenter.select_harness(HarnessKind::Codex, "claude"));
    presenter
        .model
        .harnesses
        .insert(HarnessKind::Codex, ready_probe(HarnessKind::Codex));
    emit_current_catalog(
        &presenter,
        &runner,
        vec![catalog_model(
            "explicit-remote-model",
            true,
            &[ThinkingEffort::Low, ThinkingEffort::High],
            ThinkingEffort::Low,
        )],
    );
    presenter.drain_events();
    presenter.select_catalog_model(Some("explicit-remote-model".into()));
    presenter.select_effort(ThinkingEffort::High);
    let project_id = presenter.model.selected_project.as_ref().unwrap().id;
    let other_directory = tempfile::tempdir().unwrap();
    presenter.open_project(other_directory.path());
    assert!(
        !presenter
            .model()
            .selected_catalog_model()
            .unwrap()
            .is_default
    );
    runner.0.borrow_mut().commands.clear();
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
    let request = last_start(&runner);
    assert_eq!(request.model.as_deref(), Some("explicit-remote-model"));
    assert_eq!(request.effort, ThinkingEffort::High);
    assert_eq!(presenter.remote_state().model, request.model);
    assert_eq!(presenter.remote_state().effort, request.effort);
    let config = presenter
        .storage
        .conversation_config(request.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(config.model, "explicit-remote-model");
    assert_eq!(config.effort, ThinkingEffort::High);
    assert_eq!(presenter.storage.tasks(project_id).unwrap().len(), 1);
    assert_eq!(
        runner
            .0
            .borrow()
            .commands
            .iter()
            .filter(|envelope| matches!(envelope.command, Command::RunStart(_)))
            .count(),
        1
    );
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
    presenter.select_catalog_model(Some("opus".into()));
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
    assert_eq!(request.effort, ThinkingEffort::Default);
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
    assert_eq!(presenter.model().tasks[0].title, "explain this project");
}

#[test]
fn generated_title_replaces_fallback_and_is_persisted() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("  **请修复登录流程。**\n并补充回归测试  ", "claude"));
    let task_id = presenter.model().selected_task.unwrap();
    assert_eq!(
        presenter.model().tasks[0].title,
        "请修复登录流程。 并补充回归测试"
    );

    runner.emit(Event::TaskTitleGenerated {
        task_id,
        title: "```".into(),
    });
    assert!(presenter.drain_events());
    assert_eq!(
        presenter.model().tasks[0].title,
        "请修复登录流程。 并补充回归测试"
    );

    runner.emit(Event::TaskTitleGenerated {
        task_id,
        title: "**修复登录流程。**".into(),
    });
    assert!(presenter.drain_events());

    assert_eq!(presenter.model().tasks[0].title, "修复登录流程");
    let project_id = presenter.model().selected_project.as_ref().unwrap().id;
    assert_eq!(
        presenter.storage.tasks(project_id).unwrap()[0].title,
        "修复登录流程"
    );
}

#[test]
fn title_generation_settings_persist_independently_of_conversation_selection() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("title-settings.sqlite");
    let mut presenter = Presenter::new(
        Storage::open(&path).unwrap(),
        Err(anyhow::anyhow!("test")),
        None,
    );
    assert!(presenter.select_title_harness(HarnessKind::Omp));
    assert!(!presenter.select_title_model(Some("missing".into())));
    presenter.model.title_model_catalog = ModelCatalogState::Ready(vec![catalog_model(
        "provider/title-model",
        false,
        &[],
        ThinkingEffort::Default,
    )]);
    assert!(presenter.select_title_model(Some("provider/title-model".into())));
    assert!(presenter.select_harness(HarnessKind::Codex, "claude"));
    let expected = presenter.model.title_generation.clone();
    drop(presenter);
    let mut presenter = Presenter::new(
        Storage::open(&path).unwrap(),
        Err(anyhow::anyhow!("test")),
        None,
    );
    assert_eq!(presenter.model.title_generation, expected);
    assert_eq!(presenter.model.selected_harness, HarnessKind::Codex);
    assert!(presenter.select_title_harness(HarnessKind::Claude));
    assert!(presenter.model.title_generation.model.is_none());
    assert_eq!(presenter.model.selected_harness, HarnessKind::Codex);
}

#[test]
fn title_generation_uses_separate_configuration_and_skips_resumed_runs() {
    let (mut presenter, runner, credentials, _directory) = provider_fixture();
    presenter.select_harness(HarnessKind::Omp, "claude");
    let title_profile = presenter
        .save_provider_profile(profile_draft(None, "Title", "title-secret"))
        .unwrap();
    presenter.select_title_harness(HarnessKind::Omp);
    presenter.model.title_model_catalog = ModelCatalogState::Ready(vec![catalog_model(
        "provider/title-model",
        false,
        &[],
        ThinkingEffort::Default,
    )]);
    assert!(presenter.select_title_model(Some("provider/title-model".into())));
    presenter.select_harness(HarnessKind::Claude, "/custom/omp");
    presenter
        .save_provider_profile(profile_draft(None, "Conversation", "conversation-secret"))
        .unwrap();
    presenter
        .model
        .harnesses
        .insert(HarnessKind::Claude, ready_probe(HarnessKind::Claude));
    emit_current_catalog(&presenter, &runner, claude_aliases());
    presenter.drain_events();
    presenter.select_catalog_model(Some("opus".into()));

    assert!(presenter.submit("first", "claude"));
    let first = last_start(&runner);
    assert_eq!(first.harness, HarnessKind::Claude);
    assert_eq!(first.model.as_deref(), Some("opus"));
    assert_eq!(first.environment[0].value, "conversation-secret");
    let title = first.title_generation.as_ref().unwrap();
    assert_eq!(title.harness, HarnessKind::Omp);
    assert_eq!(title.executable, "/custom/omp");
    assert_eq!(title.model.as_deref(), Some("provider/title-model"));
    assert_eq!(title.environment[0].value, "title-secret");
    assert!(!format!("{first:?}").contains("title-secret"));

    runner.emit(Event::RunSessionStarted {
        run_id: first.run_id,
        session_id: "session".into(),
    });
    runner.emit(Event::RunExited {
        run_id: first.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    assert!(presenter.submit("continue", "claude"));
    let resumed = last_start(&runner);
    assert!(resumed.title_generation.is_none());
    runner.emit(Event::RunExited {
        run_id: resumed.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    presenter.new_task();
    credentials.delete_api_key(title_profile).unwrap();
    assert!(presenter.submit("missing title credentials", "claude"));
    assert!(last_start(&runner).title_generation.is_none());
    assert_eq!(presenter.model.tasks[0].title, "missing title credentials");
}

#[test]
fn title_model_catalog_is_independent_and_ignores_stale_results() {
    let (mut presenter, runner, _directory) = fixture();
    presenter.refresh_model_catalog();
    let conversation_request = current_catalog_request_id(&presenter);
    assert!(presenter.refresh_title_model_catalog());
    let ModelCatalogState::Loading {
        request_id: stale_request,
        ..
    } = presenter.model.title_model_catalog
    else {
        panic!("loading")
    };
    assert!(presenter.select_title_harness(HarnessKind::Omp));
    assert!(presenter.refresh_title_model_catalog());
    let ModelCatalogState::Loading { request_id, .. } = presenter.model.title_model_catalog else {
        panic!("loading")
    };
    let status = presenter.model.status.clone();
    for (id, harness) in [
        (stale_request, HarnessKind::Claude),
        (request_id, HarnessKind::Claude),
    ] {
        runner.emit(Event::ModelCatalogLoaded {
            request_id: id,
            harness,
            models: claude_aliases(),
        });
    }
    presenter.drain_events();
    assert!(presenter.model.title_model_catalog.accepts(request_id));
    assert_eq!(current_catalog_request_id(&presenter), conversation_request);
    runner.emit(Event::ModelCatalogLoaded {
        request_id,
        harness: HarnessKind::Omp,
        models: vec![catalog_model(
            "provider/title-model",
            false,
            &[],
            ThinkingEffort::Default,
        )],
    });
    presenter.drain_events();
    assert_eq!(presenter.model.status, status);
    assert!(presenter.select_title_model(Some("provider/title-model".into())));
    assert!(!presenter.select_title_model(Some("not-in-catalog".into())));
    emit_current_catalog(&presenter, &runner, claude_aliases());
    presenter.drain_events();
    assert_eq!(presenter.model.selected_harness, HarnessKind::Claude);
    assert!(presenter.model.model_override.is_none());
    assert_eq!(
        presenter.model.title_generation.model.as_deref(),
        Some("provider/title-model")
    );
    assert_eq!(
        presenter.model.title_model_catalog.models().unwrap()[0].id,
        "provider/title-model"
    );
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
        assert!(
            presenter
                .model()
                .status_text()
                .contains(if missing_session {
                    "无法继续对话"
                } else {
                    "Runner 不可用"
                })
        );
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
fn user_ask_reply_uses_the_active_run_without_creating_a_prompt_or_run() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("ask me", "claude"));
    let run_id = presenter.model().active_run.unwrap();
    let request_id = Uuid::new_v4();
    runner.emit(Event::RunUserAskRequested {
        run_id: Uuid::new_v4(),
        request_id,
        questions: user_ask_questions(),
    });
    runner.emit(Event::RunUserAskRequested {
        run_id,
        request_id,
        questions: user_ask_questions(),
    });
    presenter.drain_events();
    assert_eq!(presenter.model().pending_user_asks.len(), 1);
    assert_eq!(presenter.model().pending_user_asks[0].questions.len(), 2);

    let answers = user_ask_answers();
    assert!(presenter.answer_user_ask(request_id, answers.clone()));
    assert_eq!(
        presenter.model().pending_user_asks[0].submission,
        UserAskSubmissionState::Submitting
    );
    assert!(!presenter.answer_user_ask(request_id, answers.clone()));
    assert!(matches!(
        &runner.0.borrow().commands.last().unwrap().command,
        Command::RunUserAskAnswer {
            run_id: id,
            request_id: request,
            answers: sent,
        } if *id == run_id && *request == request_id && *sent == answers
    ));

    runner.emit(Event::RunUserAskAnswerRejected {
        run_id,
        request_id,
        message: "invalid".into(),
    });
    presenter.drain_events();
    assert_eq!(
        presenter.model().pending_user_asks[0].submission,
        UserAskSubmissionState::Pending
    );
    assert_eq!(
        presenter.model().pending_user_asks[0].error.as_deref(),
        Some("invalid")
    );
    assert!(presenter.answer_user_ask(request_id, answers));
    runner.emit(Event::RunUserAskAnswerSent { run_id, request_id });
    presenter.drain_events();
    assert_eq!(
        presenter.model().pending_user_asks[0].submission,
        UserAskSubmissionState::Sent
    );
    runner.emit(Event::RunUserAskAnswerRejected {
        run_id,
        request_id,
        message: "duplicate".into(),
    });
    presenter.drain_events();
    assert_eq!(
        presenter.model().pending_user_asks[0].submission,
        UserAskSubmissionState::Sent
    );
    runner.emit(Event::RunUserAskFinished {
        run_id,
        request_id,
        status: UserAskStatus::Answered,
        message: None,
    });
    presenter.drain_events();
    assert!(presenter.model().pending_user_asks.is_empty());
    assert_eq!(presenter.model().messages.len(), 3);
    assert!(presenter.model().messages[1].content.contains("Agent 提问"));
    assert!(presenter.model().messages[1].content.contains("Target?"));
    assert!(
        presenter.model().messages[2]
            .content
            .contains("User Ask 已回答")
    );
    assert!(presenter.model().messages[2].content.contains("Workspace"));
    assert!(
        presenter.model().messages[2]
            .content
            .contains("Keep it focused")
    );
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
fn user_ask_drafts_cover_choice_text_navigation_and_retry_without_duplicate_submission() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("ask me", "claude"));
    let run_id = presenter.model().active_run.unwrap();
    let request_id = Uuid::new_v4();
    runner.emit(Event::RunUserAskRequested {
        run_id,
        request_id,
        questions: rich_user_ask_questions(),
    });
    presenter.drain_events();

    assert!(!presenter.can_submit_user_ask(request_id));
    assert!(presenter.set_user_ask_question(request_id, 3));
    assert!(!presenter.set_user_ask_question(request_id, 4));
    assert_eq!(presenter.model().pending_user_asks[0].active_question, 3);
    assert!(presenter.toggle_user_ask_collapsed(request_id));
    assert!(presenter.model().pending_user_asks[0].collapsed);
    assert!(presenter.toggle_user_ask_collapsed(request_id));
    assert!(!presenter.model().pending_user_asks[0].collapsed);

    assert!(presenter.set_user_ask_option(request_id, "target", "library", true));
    assert!(presenter.set_user_ask_option(request_id, "target", "workspace", true));
    assert!(presenter.set_user_ask_option(request_id, "checks", "tests", true));
    assert!(presenter.set_user_ask_option(request_id, "checks", "clippy", true));
    assert!(presenter.set_user_ask_option(request_id, "scope", "focused", true));
    assert!(presenter.set_user_ask_text(request_id, "scope", String::new()));
    assert_eq!(
        presenter.model().pending_user_asks[0].drafts.get("scope"),
        Some(&UserAskAnswerValue::Selected(vec!["focused".into()]))
    );
    assert!(presenter.set_user_ask_text(request_id, "scope", "entire workspace".into(),));
    assert!(presenter.set_user_ask_text(request_id, "note", "keep it focused".into()));
    assert!(!presenter.set_user_ask_text(request_id, "target", "invalid".into()));
    assert!(!presenter.set_user_ask_option(request_id, "note", "tests", true));
    assert!(!presenter.set_user_ask_option(request_id, "checks", "unknown", true));
    assert!(presenter.can_submit_user_ask(request_id));

    let expected = vec![
        UserAskAnswer {
            question_id: "target".into(),
            value: UserAskAnswerValue::Selected(vec!["workspace".into()]),
        },
        UserAskAnswer {
            question_id: "checks".into(),
            value: UserAskAnswerValue::Selected(vec!["tests".into(), "clippy".into()]),
        },
        UserAskAnswer {
            question_id: "scope".into(),
            value: UserAskAnswerValue::Text("entire workspace".into()),
        },
        UserAskAnswer {
            question_id: "note".into(),
            value: UserAskAnswerValue::Text("keep it focused".into()),
        },
    ];
    assert!(presenter.submit_user_ask(request_id));
    assert!(!presenter.submit_user_ask(request_id));
    assert!(matches!(
        &runner.0.borrow().commands.last().unwrap().command,
        Command::RunUserAskAnswer { answers, .. } if answers == &expected
    ));
    assert_eq!(
        presenter.model().pending_user_asks[0].submitted_answers,
        Some(expected.clone())
    );

    runner.emit(Event::RunUserAskAnswerRejected {
        run_id,
        request_id,
        message: "try again".into(),
    });
    presenter.drain_events();
    let request = &presenter.model().pending_user_asks[0];
    assert_eq!(request.submission, UserAskSubmissionState::Pending);
    assert_eq!(request.error.as_deref(), Some("try again"));
    assert_eq!(request.submitted_answers, None);
    assert_eq!(
        request.drafts.get("note"),
        Some(&UserAskAnswerValue::Text("keep it focused".into()))
    );
    assert!(presenter.can_submit_user_ask(request_id));
    assert!(presenter.submit_user_ask(request_id));
    assert_eq!(
        runner
            .0
            .borrow()
            .commands
            .iter()
            .filter(|command| matches!(command.command, Command::RunUserAskAnswer { .. }))
            .count(),
        2
    );
}

#[test]
fn user_ask_terminal_events_preserve_request_order_and_write_read_only_history() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("ask twice", "claude"));
    let run_id = presenter.model().active_run.unwrap();
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    for request_id in [first, second] {
        runner.emit(Event::RunUserAskRequested {
            run_id,
            request_id,
            questions: user_ask_questions(),
        });
    }
    presenter.drain_events();
    assert_eq!(
        presenter
            .model()
            .pending_user_asks
            .iter()
            .map(|request| request.request_id)
            .collect::<Vec<_>>(),
        vec![first, second]
    );

    runner.emit(Event::RunUserAskFinished {
        run_id,
        request_id: first,
        status: UserAskStatus::Cancelled,
        message: Some("native request closed".into()),
    });
    presenter.drain_events();
    assert_eq!(presenter.model().pending_user_asks.len(), 1);
    assert_eq!(presenter.model().pending_user_asks[0].request_id, second);
    assert_eq!(presenter.model().status_text(), "Agent 正在等待你的回答。");
    assert!(presenter.model().messages.iter().any(|message| {
        message.content.contains("User Ask 已取消")
            && message.content.contains("native request closed")
    }));

    runner.emit(Event::RunExited {
        run_id,
        status: RunStatus::Failed,
        exit_code: Some(1),
    });
    presenter.drain_events();
    assert!(presenter.model().pending_user_asks.is_empty());
    assert!(presenter.model().messages.iter().any(|message| {
        message.content.contains("User Ask 已失效") && message.content.contains("Target?")
    }));
    assert!(!presenter.submit_user_ask(second));
}

#[test]
fn user_ask_send_failure_and_run_exit_leave_no_stale_desktop_state() {
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("ask me", "claude"));
    let run_id = presenter.model().active_run.unwrap();
    let request_id = Uuid::new_v4();
    runner.emit(Event::RunUserAskRequested {
        run_id,
        request_id,
        questions: user_ask_questions(),
    });
    presenter.drain_events();

    runner.0.borrow_mut().fail_send = true;
    assert!(!presenter.answer_user_ask(request_id, user_ask_answers()));
    assert_eq!(
        presenter.model().pending_user_asks[0].submission,
        UserAskSubmissionState::Pending
    );
    assert!(presenter.model().pending_user_asks[0].error.is_some());
    runner.0.borrow_mut().fail_send = false;

    runner.emit(Event::RunExited {
        run_id,
        status: RunStatus::Failed,
        exit_code: Some(1),
    });
    presenter.drain_events();
    assert!(presenter.model().pending_user_asks.is_empty());
    assert!(presenter.model().active_run.is_none());
    assert!(!presenter.answer_user_ask(request_id, user_ask_answers()));
}

fn user_ask_questions() -> Vec<UserAskQuestion> {
    vec![
        UserAskQuestion {
            id: "target".into(),
            prompt: "Target?".into(),
            answer_mode: UserAskAnswerMode::Choice {
                multiple: false,
                allow_custom: false,
            },
            options: vec![UserAskOption {
                id: "workspace".into(),
                label: "Workspace".into(),
                description: None,
            }],
        },
        UserAskQuestion {
            id: "note".into(),
            prompt: "Note?".into(),
            answer_mode: UserAskAnswerMode::Text,
            options: Vec::new(),
        },
    ]
}

fn rich_user_ask_questions() -> Vec<UserAskQuestion> {
    vec![
        UserAskQuestion {
            id: "target".into(),
            prompt: "Target?".into(),
            answer_mode: UserAskAnswerMode::Choice {
                multiple: false,
                allow_custom: false,
            },
            options: vec![
                UserAskOption {
                    id: "library".into(),
                    label: "Library".into(),
                    description: None,
                },
                UserAskOption {
                    id: "workspace".into(),
                    label: "Workspace".into(),
                    description: None,
                },
            ],
        },
        UserAskQuestion {
            id: "checks".into(),
            prompt: "Checks?".into(),
            answer_mode: UserAskAnswerMode::Choice {
                multiple: true,
                allow_custom: false,
            },
            options: vec![
                UserAskOption {
                    id: "tests".into(),
                    label: "Tests".into(),
                    description: None,
                },
                UserAskOption {
                    id: "clippy".into(),
                    label: "Clippy".into(),
                    description: None,
                },
            ],
        },
        UserAskQuestion {
            id: "scope".into(),
            prompt: "Scope?".into(),
            answer_mode: UserAskAnswerMode::Choice {
                multiple: false,
                allow_custom: true,
            },
            options: vec![UserAskOption {
                id: "focused".into(),
                label: "Focused".into(),
                description: None,
            }],
        },
        UserAskQuestion {
            id: "note".into(),
            prompt: "Note?".into(),
            answer_mode: UserAskAnswerMode::Text,
            options: Vec::new(),
        },
    ]
}

fn user_ask_answers() -> Vec<UserAskAnswer> {
    vec![
        UserAskAnswer {
            question_id: "target".into(),
            value: UserAskAnswerValue::Selected(vec!["workspace".into()]),
        },
        UserAskAnswer {
            question_id: "note".into(),
            value: UserAskAnswerValue::Text("Keep it focused".into()),
        },
    ]
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
        let cwd = directory.path().canonicalize().unwrap();
        assert_eq!(Path::new(&last_start(&runner).cwd), cwd);
        assert_eq!(presenter.model().working_directory(), cwd.to_str());
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
        assert_eq!(presenter.model().working_directory(), cwd.to_str());
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
        assert!(presenter.model().status_text().contains("可执行文件不一致"));
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
            assert_eq!(
                Some(request.cwd.as_str()),
                presenter.model().working_directory()
            );
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
        assert_eq!(presenter.model().working_directory(), cwd.to_str());
        let state = runner.0.borrow();
        let Command::RunStart(request) = &state.commands.last().unwrap().command else {
            panic!("expected new task");
        };
        assert_ne!(request.task_id, task_id);
        assert!(request.session_id.is_none());
        assert_eq!(
            Some(request.cwd.as_str()),
            presenter.model().working_directory()
        );
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
        assert!(
            presenter
                .model()
                .status_text()
                .contains(if missing_session {
                    "未保存可恢复的会话"
                } else {
                    "切回该 Harness"
                })
        );
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
    assert_eq!(
        presenter.model().status_text(),
        "Runner 不可用，任务未启动。"
    );
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
    presenter.select_catalog_model(Some("opus".into()));
    presenter.select_effort(ThinkingEffort::Max);
    assert!(!presenter.select_harness(HarnessKind::Codex, "claude"));
    assert!(!presenter.select_model_configuration(HarnessKind::Codex, None, "claude"));
    presenter.new_task();
    presenter.select_codex_thread("history".into());
    assert!(presenter.model().model_override.is_none());
    assert_eq!(presenter.model().effort, ThinkingEffort::Default);
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
    presenter.select_catalog_model(Some("opus".into()));
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
fn all_harnesses_share_selection_priority_and_run_configuration() {
    for harness in HarnessKind::ALL {
        let (mut presenter, runner, _credentials, _directory) = provider_fixture();
        presenter.select_harness(harness, "claude");
        presenter
            .model
            .harnesses
            .insert(harness, ready_probe(harness));
        let mut draft = profile_draft(None, "Provider Profile", "profile-secret");
        draft.model = "profile-model".into();
        let profile_id = presenter.save_provider_profile(draft).unwrap();
        let models = vec![
            catalog_model(
                "cli-model",
                true,
                &[ThinkingEffort::Low],
                ThinkingEffort::Low,
            ),
            catalog_model(
                "profile-model",
                false,
                &[ThinkingEffort::Medium],
                ThinkingEffort::Medium,
            ),
            catalog_model(
                "explicit-model",
                false,
                &[ThinkingEffort::Low, ThinkingEffort::High],
                ThinkingEffort::High,
            ),
        ];
        emit_current_catalog(&presenter, &runner, models.clone());
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
                .setting(&catalog_model_setting_key(harness, Some(profile_id),))
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
                .setting(&catalog_model_setting_key(harness, Some(profile_id),))
                .unwrap()
                .as_deref(),
            Some("")
        );

        presenter.select_catalog_model(Some("explicit-model".into()));
        assert!(presenter.model().can_submit());
        assert!(presenter.submit("use catalog selection", harness.default_executable()));
        let resolved = presenter.model().resolved_model_selection();
        let remote = presenter.remote_state();
        assert_eq!(remote.model, resolved.model);
        assert_eq!(remote.effort, resolved.effort);
        assert!(
            !serde_json::to_string(&remote)
                .unwrap()
                .contains("profile-secret")
        );
        assert_eq!(
            presenter
                .model()
                .selected_provider_profile()
                .unwrap()
                .model
                .as_deref(),
            Some("profile-model")
        );
        let mut request = last_start(&runner);
        assert_eq!(request.harness, harness);
        assert_eq!(request.model.as_deref(), Some("explicit-model"));
        assert_eq!(request.effort, ThinkingEffort::High);
        let config = presenter
            .storage
            .conversation_config(request.task_id)
            .unwrap()
            .unwrap();
        assert_eq!(config.model, "explicit-model");
        assert_eq!(config.effort, ThinkingEffort::High);

        for (profile, expected_model, expected_effort) in [
            (
                Some(profile_id),
                Some("profile-model"),
                ThinkingEffort::Medium,
            ),
            (None, None, ThinkingEffort::Default),
        ] {
            runner.emit(Event::RunExited {
                run_id: request.run_id,
                status: RunStatus::Completed,
                exit_code: Some(0),
            });
            presenter.drain_events();
            presenter.new_task();
            presenter.select_catalog_model(None);
            if profile.is_none() {
                presenter.select_provider_profile(None);
                emit_current_catalog(&presenter, &runner, models.clone());
                presenter.drain_events();
            }
            assert_eq!(presenter.model().configured_catalog_model(), expected_model);
            assert!(presenter.submit("follow default", harness.default_executable()));
            request = last_start(&runner);
            assert_eq!(request.model.as_deref(), expected_model);
            assert_eq!(request.effort, expected_effort);
            let remote = presenter.remote_state();
            assert_eq!(remote.model, request.model);
            assert_eq!(remote.effort, request.effort);
            let config = presenter
                .storage
                .conversation_config(request.task_id)
                .unwrap()
                .unwrap();
            assert_eq!(config.model, expected_model.unwrap_or("default"));
            assert_eq!(config.effort, request.effort);
        }
    }
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

    assert!(presenter.refresh_model_catalog());
    runner.emit(Event::ModelCatalogFailed {
        request_id: current_catalog_request_id(&presenter),
        harness: HarnessKind::Codex,
        message: "temporary catalog failure".into(),
    });
    presenter.drain_events();
    assert_eq!(presenter.model().effort, ThinkingEffort::High);
    assert_eq!(
        presenter.model().resolved_model_selection().effort,
        ThinkingEffort::High
    );
    assert_eq!(
        presenter
            .storage
            .setting(&catalog_effort_setting_key(
                HarnessKind::Codex,
                Some(first_profile_id),
            ))
            .unwrap()
            .as_deref(),
        Some("high")
    );
    assert!(presenter.refresh_model_catalog());
    emit_current_catalog(&presenter, &runner, models.clone());
    presenter.drain_events();
    assert_eq!(presenter.model().effort, ThinkingEffort::High);
    assert_eq!(
        presenter.model().resolved_model_selection().effort,
        ThinkingEffort::High
    );

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
    assert!(presenter.model().selected_catalog_model().is_none());
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
    assert!(presenter.model().status_text().contains("恢复为模型默认"));
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
    presenter
        .save_provider_profile(profile_draft(
            None,
            "Other Codex Profile",
            "other-codex-secret",
        ))
        .unwrap();

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

    runner.0.borrow_mut().commands.clear();
    assert!(presenter.select_model_configuration(
        HarnessKind::Codex,
        Some(codex_profile_id),
        "omp"
    ));
    {
        let state = runner.0.borrow();
        let catalogs = state
            .commands
            .iter()
            .filter_map(|envelope| match &envelope.command {
                Command::ModelCatalogRefresh {
                    harness,
                    executable,
                    environment,
                    ..
                } => Some((harness, executable, environment)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(catalogs.len(), 1);
        let (harness, executable, environment) = catalogs[0];
        assert_eq!(*harness, HarnessKind::Codex);
        assert_eq!(executable, "codex");
        assert_eq!(environment.len(), 1);
        assert_eq!(environment[0].value, "codex-secret");
    }
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
fn all_harnesses_restore_each_profile_and_cli_selection_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("preferences.db");
    let credentials = FakeCredentialStore::default();
    let runner = FakeRunner::default();
    let mut presenter = Presenter::new_with_credentials(
        Storage::open(&database).unwrap(),
        Ok(Box::new(runner.clone())),
        None,
        Box::new(credentials.clone()),
    );
    presenter.open_project(directory.path());
    let mut expected = Vec::new();
    for harness in HarnessKind::ALL {
        let executable = presenter.model().executable.clone();
        presenter.select_harness(harness, &executable);
        for (index, name) in ["cli", "first", "second"].into_iter().enumerate() {
            let profile = if index == 0 {
                presenter.select_provider_profile(None);
                None
            } else {
                Some(
                    presenter
                        .save_provider_profile(profile_draft(None, name, "profile-secret"))
                        .unwrap(),
                )
            };
            let id = format!("{harness}/{name}/model");
            let mut model = catalog_model(
                &id,
                false,
                &[ThinkingEffort::Low, ThinkingEffort::High],
                ThinkingEffort::High,
            );
            model.display_name = format!("{name} model");
            emit_current_catalog(&presenter, &runner, vec![model.clone()]);
            presenter.drain_events();
            presenter.select_catalog_model(Some(id.clone()));
            let effort = if index == 1 {
                ThinkingEffort::High
            } else {
                ThinkingEffort::Low
            };
            presenter.select_effort(effort);
            expected.push((harness, profile, model, effort));
        }
    }
    drop(presenter);
    let mut presenter = Presenter::new_with_credentials(
        Storage::open(&database).unwrap(),
        Ok(Box::new(runner.clone())),
        None,
        Box::new(credentials),
    );
    presenter.open_project(directory.path());
    for (harness, profile, model, effort) in expected.into_iter().rev() {
        let executable = presenter.model().executable.clone();
        presenter.select_harness(harness, &executable);
        presenter.select_provider_profile(profile);
        assert_eq!(
            presenter.model().model_override.as_deref(),
            Some(model.id.as_str())
        );
        assert_eq!(
            presenter.model().model_override_name.as_deref(),
            Some(model.display_name.as_str())
        );
        assert_eq!(presenter.model().effort, effort);
        emit_current_catalog(&presenter, &runner, vec![model.clone()]);
        presenter.drain_events();
        assert_eq!(
            presenter.model().resolved_model_selection().model,
            Some(model.id)
        );
        assert_eq!(presenter.model().resolved_model_selection().effort, effort);
    }
}

#[test]
fn claude_legacy_preferences_migrate_once_without_leaking_to_profiles() {
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    storage.set_setting("claude_model", "opus").unwrap();
    let (model, effort) =
        load_catalog_preferences(&storage, HarnessKind::Claude, None, ThinkingEffort::High);
    assert_eq!(model.as_deref(), Some("opus"));
    assert_eq!(effort, ThinkingEffort::High);
    let (model, effort) = load_catalog_preferences(
        &storage,
        HarnessKind::Claude,
        Some(Uuid::new_v4()),
        ThinkingEffort::High,
    );
    assert!(model.is_none());
    assert_eq!(effort, ThinkingEffort::Default);
    storage
        .set_setting("claude_model_override_cli", "")
        .unwrap();
    assert!(
        load_catalog_preferences(&storage, HarnessKind::Claude, None, ThinkingEffort::High)
            .0
            .is_none()
    );
}

#[test]
fn catalog_context_changes_ignore_late_responses_and_keep_missing_model_names() {
    for harness in HarnessKind::ALL {
        let (mut presenter, runner, _credentials, directory) = provider_fixture();
        presenter.select_harness(harness, "claude");
        presenter.refresh_model_catalog();
        let mut model = catalog_model(
            "chosen-id",
            false,
            &[ThinkingEffort::High],
            ThinkingEffort::High,
        );
        model.display_name = "Recognizable name".into();
        emit_current_catalog(&presenter, &runner, vec![model]);
        presenter.drain_events();
        presenter.select_catalog_model(Some("chosen-id".into()));
        presenter.probe("/new/executable");
        assert!(presenter.model().selected_catalog_model().is_none());
        let stale = current_catalog_request_id(&presenter);
        let other = directory.path().join("other-project");
        fs::create_dir(&other).unwrap();
        presenter.open_project(&other);
        let current = current_catalog_request_id(&presenter);
        assert_ne!(stale, current);
        for event in [
            Event::ModelCatalogLoaded {
                request_id: stale,
                harness,
                models: vec![],
            },
            Event::ModelCatalogFailed {
                request_id: stale,
                harness,
                message: "stale failure".into(),
            },
        ] {
            runner.emit(event);
        }
        presenter.drain_events();
        assert_eq!(current_catalog_request_id(&presenter), current);
        assert!(runner.0.borrow().commands.iter().any(|command| matches!(&command.command,
            Command::ModelCatalogRefresh { request_id, executable, cwd, .. }
            if *request_id == current && executable == "/new/executable" && Path::new(cwd) == other.canonicalize().unwrap())));
        emit_current_catalog(&presenter, &runner, vec![]);
        presenter.drain_events();
        assert!(!presenter.model().catalog_selection_is_valid());
        assert_eq!(
            presenter.model().model_override_name.as_deref(),
            Some("Recognizable name")
        );
        presenter.select_catalog_model(None);
        assert!(presenter.model().catalog_selection_is_valid());
    }
}

#[test]
fn unavailable_models_and_unknown_efforts_cannot_change_the_requested_configuration() {
    let (mut presenter, runner, _directory) = fixture();
    presenter.select_catalog_model(Some("opus".into()));
    presenter.select_effort(ThinkingEffort::Max);
    assert_eq!(presenter.model().effort, ThinkingEffort::Default);
    assert!(presenter.model().status_text().contains("不支持"));
    assert!(presenter.refresh_model_catalog());
    let mut unavailable = claude_aliases().remove(1);
    unavailable.availability = nexus_domain::ModelAvailability::Unavailable {
        reason: "disabled by provider".into(),
    };
    emit_current_catalog(&presenter, &runner, vec![unavailable]);
    presenter.drain_events();
    assert!(!presenter.model().can_submit());
    assert!(!presenter.submit("must not start", "claude"));
    assert_eq!(presenter.model().model_override.as_deref(), Some("opus"));
    presenter.select_catalog_model(None);
    assert!(presenter.model().can_submit());
    assert!(presenter.refresh_model_catalog());
    let request_id = current_catalog_request_id(&presenter);
    assert!(presenter.submit("use defaults", "claude"));
    let request = last_start(&runner);
    presenter.probe("other");
    presenter.select_effort(ThinkingEffort::High);
    runner.emit(Event::ModelCatalogLoaded {
        request_id,
        harness: HarnessKind::Claude,
        models: claude_aliases(),
    });
    presenter.drain_events();
    assert!(matches!(
        presenter.model().model_catalog,
        ModelCatalogState::Loading { .. }
    ));
    let remote = presenter.remote_state();
    assert_eq!(remote.model, request.model);
    assert_eq!(remote.effort, request.effort);
    assert_eq!(presenter.model().executable, "claude");
    let recorded = presenter
        .storage
        .conversation_config(request.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(recorded.effort, request.effort);

    runner.0.borrow_mut().commands.clear();
    runner.emit(Event::RunExited {
        run_id: request.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    assert_ne!(current_catalog_request_id(&presenter), request_id);
    assert_eq!(runner.0.borrow().commands.len(), 1);
    emit_current_catalog(&presenter, &runner, claude_aliases());
    presenter.drain_events();
    assert!(matches!(
        presenter.model().model_catalog,
        ModelCatalogState::Ready(_)
    ));
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
        ModelCatalogState::Loading { request_id, .. } if request_id == current_request_id
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
    assert!(presenter.model().status_text().contains("尚未验证可用"));
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
    assert!(presenter.model().status_text().contains("未通过目录验证"));
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
    assert!(presenter.model().status_text().contains("没有 API Key"));
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
    assert!(presenter.model().status_text().contains("*_API_KEY"));
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
    assert!(presenter.model().status_text().contains("48"));

    draft = profile_draft(None, "Valid name", "secret");
    draft.model = "m".repeat(129);
    assert!(presenter.save_provider_profile(draft).is_none());
    assert!(presenter.model().status_text().contains("128"));
    assert!(presenter.model().provider_profiles.is_empty());
}

#[test]
fn selecting_a_saved_task_restores_its_configuration_and_messages() {
    let (mut presenter, runner, _directory) = fixture();
    presenter.select_catalog_model(Some("sonnet".into()));
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
    assert_eq!(presenter.model().model_override.as_deref(), Some("sonnet"));
    assert_eq!(presenter.model().effort, ThinkingEffort::Default);
    assert_eq!(
        presenter.model().executable,
        ready_probe(HarnessKind::Claude).executable
    );
    assert_eq!(presenter.model().messages[0].content, "hello");

    for catalog_pending in [false, true] {
        presenter.new_task();
        let executable = presenter.model().executable.clone();
        assert!(presenter.select_harness(HarnessKind::Codex, &executable));
        presenter
            .model
            .harnesses
            .insert(HarnessKind::Codex, ready_probe(HarnessKind::Codex));
        if !catalog_pending {
            emit_current_catalog(&presenter, &runner, vec![]);
            presenter.drain_events();
        }
        assert!(presenter.submit("another task", "codex"));
        let run_id = presenter.model().active_run.unwrap();
        presenter.select_task(task_id);
        assert_eq!(presenter.model().selected_harness, HarnessKind::Codex);
        runner.0.borrow_mut().commands.clear();

        runner.emit(Event::RunExited {
            run_id,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        let state = runner.0.borrow();
        let refreshes = state
            .commands
            .iter()
            .map(|envelope| &envelope.command)
            .filter(|command| matches!(command, Command::ModelCatalogRefresh { .. }))
            .collect::<Vec<_>>();
        assert_eq!(
            refreshes.len(),
            1,
            "task restoration must refresh only once"
        );
        assert!(matches!(refreshes[0], Command::ModelCatalogRefresh {
            request_id, harness: HarnessKind::Claude, executable, cwd, ..
        } if *request_id == current_catalog_request_id(&presenter)
            && *executable == ready_probe(HarnessKind::Claude).executable
            && *cwd == presenter.model().selected_project.as_ref().unwrap().canonical_path));
        drop(state);

        emit_current_catalog(&presenter, &runner, claude_aliases());
        presenter.drain_events();
        assert_eq!(presenter.model().model_override.as_deref(), Some("sonnet"));
        assert_eq!(presenter.model().messages[0].content, "hello");
        assert!(presenter.model().catalog_selection_is_valid());
    }
}

#[test]
fn working_directory_follows_project_and_task_selection_during_background_runs() {
    let (mut presenter, runner, directory) = fixture();
    let mut conversations = Vec::new();
    for parent in ["first", "第二个 项目"] {
        let path = directory.path().join(parent).join("同名目录 nexus");
        fs::create_dir_all(&path).unwrap();
        presenter.open_project(&path);
        assert!(!presenter.model().project_dirty);
        assert_eq!(
            presenter.model().working_directory(),
            path.canonicalize().unwrap().to_str()
        );
        assert!(presenter.submit("first question", "claude"));
        let start = last_start(&runner);
        assert_eq!(
            Some(start.cwd.as_str()),
            presenter.model().working_directory()
        );
        conversations.push((
            presenter.model().selected_project.clone().unwrap(),
            start.task_id,
        ));
        runner.emit(Event::RunSessionStarted {
            run_id: start.run_id,
            session_id: start.task_id.to_string(),
        });
        runner.emit(Event::RunExited {
            run_id: start.run_id,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        assert_eq!(
            Some(start.cwd.as_str()),
            presenter.model().working_directory()
        );
    }
    let (first_project, first_task) = &conversations[0];
    let (second_project, second_task) = &conversations[1];
    assert_eq!(first_project.display_name, second_project.display_name);
    assert_ne!(first_project.canonical_path, second_project.canonical_path);
    assert!(presenter.submit("background follow-up", "claude"));
    let background = last_start(&runner);

    presenter.select_project(first_project.clone());
    presenter.select_task(*first_task);
    runner.emit(Event::RunOutputDelta {
        run_id: background.run_id,
        text: "background output".into(),
    });
    presenter.drain_events();
    assert_eq!(
        presenter.model().working_directory(),
        Some(first_project.canonical_path.as_str())
    );
    runner.emit(Event::RunExited {
        run_id: background.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    assert_eq!(presenter.model().selected_task, Some(*first_task));
    assert_eq!(
        presenter.model().working_directory(),
        Some(first_project.canonical_path.as_str())
    );
    presenter.select_project(second_project.clone());
    presenter.select_task(*second_task);
    assert!(presenter.submit("resume second task", "claude"));
    assert_eq!(last_start(&runner).cwd, second_project.canonical_path);
    assert_eq!(
        presenter.model().working_directory(),
        Some(second_project.canonical_path.as_str())
    );
}

#[test]
fn history_responses_only_update_the_selected_thread() {
    let (mut presenter, _, _directory) = fixture();
    let local_path = presenter.model().working_directory().unwrap().to_owned();
    let history_path = Path::new(&local_path)
        .join("Codex 历史")
        .display()
        .to_string();
    presenter.handle_codex_history_event(HistoryEvent::ThreadsLoaded(Ok(vec![ThreadSummary {
        id: "selected".into(),
        title: "history".into(),
        cwd: history_path.clone(),
        source: "cli".into(),
        updated_at: 0,
        archived: false,
    }])));
    presenter.select_codex_thread("selected".into());
    assert_eq!(
        presenter.model().working_directory(),
        Some(history_path.as_str())
    );
    presenter.model.codex_thread_loading = true;
    presenter.handle_codex_history_event(HistoryEvent::ThreadLoaded {
        thread_id: "previous".into(),
        result: Err("stale error".into()),
    });
    assert!(presenter.model().codex_thread_loading);
    assert!(presenter.model().codex_history_messages.is_empty());
    assert_eq!(
        presenter.model().working_directory(),
        Some(history_path.as_str())
    );
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
    assert_eq!(
        presenter.model().working_directory(),
        Some(history_path.as_str())
    );
    presenter.model.codex_threads[0].cwd.clear();
    assert_eq!(presenter.model().working_directory(), None);
    presenter.select_codex_thread("missing-thread".into());
    assert_eq!(presenter.model().working_directory(), None);
    presenter.new_task();
    assert_eq!(
        presenter.model().working_directory(),
        Some(local_path.as_str())
    );
}
