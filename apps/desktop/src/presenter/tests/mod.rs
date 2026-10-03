mod attachments;
mod catalogs;
mod concurrency;
mod configuration;
mod generation;
mod issues;
mod preferences;
mod provider_profiles;
mod queue;
mod remote;
mod runs;
mod tasks;
mod updates;
mod user_ask;
mod voice;
mod workspaces;

use std::{
    cell::RefCell,
    collections::HashMap,
    fs,
    rc::Rc,
    time::{Duration, Instant},
};

use super::*;
use crate::{
    infrastructure::{credentials::CredentialStore, storage::NewTaskRun},
    model::UserAskSubmissionState,
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
    let mut presenter = Presenter::new_with_credentials(
        storage,
        Ok(Box::new(runner.clone())),
        None,
        Box::new(FakeCredentialStore::default()),
    );
    presenter.issues_client = crate::infrastructure::issues::Client::fake();
    presenter.open_project(directory.path());
    presenter.model.conversation.model_catalog = ModelCatalogState::Ready(claude_aliases());
    presenter
        .model
        .harnesses
        .insert(HarnessKind::Claude, ready_probe(HarnessKind::Claude));
    runner.0.borrow_mut().commands.clear();
    (presenter, runner, directory)
}

pub(crate) fn cnb_issue(number: &str) -> crate::model::issues::Issue {
    serde_json::from_value(serde_json::json!({
        "number":number, "title":format!("CNB 集成测试 #{number}"), "state":"open",
        "body":"## 问题描述\n\n支持 **Markdown**、链接与代码。\n\n```rust\nfn main() {}\n```",
        "author":{"username":"author", "nickname":"开发者"},
        "assignees":[{"username":"owner"}], "labels":[{"name":"enhancement", "color":"#315DC5"}],
        "created_at":"2026-09-09T00:00:00Z", "updated_at":"2026-09-09T01:00:00Z", "priority":"P1", "comment_count":2
    })).unwrap()
}

pub(crate) fn cnb_comment(id: &str) -> crate::model::issues::Comment {
    serde_json::from_value(serde_json::json!({
        "id":id, "body":format!("验收条件 {id}：保留 **Markdown** 与附件 ![截图](https://example.test/comment.png)"),
        "author":{"username":"reviewer", "nickname":"评审者"}, "created_at":"2026-09-10T00:00:00Z"
    })).unwrap()
}

pub(crate) fn seed_issues(presenter: &mut Presenter, provider: IssueProvider) {
    *presenter.model.issues_mut(provider) = crate::model::issues::IssuesModel {
        repository: Some("team/project".into()),
        cli: Some(crate::model::issues::Cli {
            path: "/missing-test-cnb".into(),
            version: "1.10.10".into(),
        }),
        issues: (1..=30)
            .map(|number| cnb_issue(&number.to_string()))
            .collect(),
        total: 61,
        ..Default::default()
    };
}

pub(crate) fn pull_detail(number: &str) -> crate::model::pull_requests::PullDetail {
    use crate::model::pull_requests::{Check, PullDetail, PullRequest, Review, ReviewThread};
    let pull = PullRequest {
        number: number.into(),
        title: format!("PR {number}"),
        body: "PR body **Markdown**".into(),
        state: "open".into(),
        author: "author".into(),
        head_repository: "team/project".into(),
        head_branch: format!("feat/pr-{number}"),
        base_branch: "main".into(),
        head_sha: "b".repeat(40),
        base_sha: "a".repeat(40),
        draft: false,
        merge_status: "MERGEABLE · CLEAN".into(),
        mergeable: true,
    };
    let mut root = cnb_comment("31");
    root.body = "Other Agent finding".into();
    let mut reply = cnb_comment("32");
    reply.body = "Last conversation reply".into();
    PullDetail {
        pull: pull.clone(),
        comments: vec![cnb_comment("99")],
        reviews: vec![Review {
            id: "9".into(),
            author: "other-agent".into(),
            state: "CHANGES_REQUESTED".into(),
            body: "Review finding".into(),
        }],
        threads: vec![ReviewThread {
            id: "T1".into(),
            path: "src/main.rs".into(),
            line: Some(12),
            resolved: false,
            outdated: false,
            comments: vec![root, reply],
        }],
        checks: vec![Check {
            name: "test".into(),
            state: "failure".into(),
            description: "Test failed".into(),
            url: "https://github.com/team/project/actions/runs/8".into(),
        }],
        stack: vec![
            PullRequest {
                number: "1".into(),
                head_branch: "parent".into(),
                ..pull.clone()
            },
            pull,
        ],
    }
}

pub(crate) fn seed_pull_requests(presenter: &mut Presenter, provider: IssueProvider) {
    seed_issues(presenter, provider);
    presenter.model.issues_mut(provider).pulls.pulls =
        vec![pull_detail("2").pull, pull_detail("3").pull];
    presenter.model.issues_mut(provider).pulls.total = 61;
}

pub(crate) fn finish_issue_request(
    presenter: &mut Presenter,
    provider: IssueProvider,
    response: crate::infrastructure::issues::Response,
) {
    use crate::infrastructure::issues::{Event, Response};
    let id = match &response {
        Response::Inspection { .. } => presenter.model.issues(provider).detection_request,
        Response::List(_) => presenter.model.issues(provider).list_request,
        Response::Detail(_) => presenter.model.issues(provider).detail_request,
        Response::Comments(_) => presenter.model.issues(provider).comments_request,
        Response::Action(_) => presenter
            .model
            .issues(provider)
            .action_request
            .map(|(id, _)| id),
        Response::NpcAction(_) => presenter.model.issues(provider).npc_request,
        Response::PullRequests(response) => {
            let pulls = &presenter.model.issues(provider).pulls;
            match response {
                crate::infrastructure::pull_requests::Response::List(_) => pulls.list_request,
                crate::infrastructure::pull_requests::Response::Detail(_) => pulls.detail_request,
                crate::infrastructure::pull_requests::Response::Action(_) => {
                    pulls.action_request.map(|(id, _)| id)
                }
            }
        }
    }
    .expect("pending issue request");
    presenter.handle_issue_event(Event {
        provider,
        id,
        response,
    });
}

pub(crate) fn finish_workspace_operation(presenter: &mut Presenter) {
    // Git subprocesses are slow on Windows CI, especially while the whole
    // test suite runs in parallel, so only fail on a genuinely stuck operation.
    let deadline = Instant::now() + Duration::from_secs(120);
    while presenter.model.workspace_busy {
        assert!(Instant::now() < deadline, "workspace operation timed out");
        presenter.drain_events();
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn start_test_worktree(presenter: &mut Presenter, runner: &FakeRunner, prompt: &str) -> StartRun {
    presenter.select_workspace_kind(crate::model::workspace::WorkspaceKind::Worktree);
    assert!(presenter.submit(prompt, "claude"));
    finish_workspace_operation(presenter);
    emit_current_catalog(presenter, runner, claude_aliases());
    presenter.drain_events();
    last_start(runner)
}

pub(crate) fn worktree_fixture(
    prompt: &str,
) -> (Presenter, FakeRunner, tempfile::TempDir, StartRun) {
    let (directory, project) = crate::infrastructure::git::tests::repository_fixture();
    let (mut presenter, runner, _fixture) = fixture();
    presenter.worktree_root = Ok(directory.path().canonicalize().unwrap().join("worktrees"));
    presenter.open_project(Path::new(&project.canonical_path));
    let start = start_test_worktree(&mut presenter, &runner, prompt);
    (presenter, runner, directory, start)
}

pub(crate) fn pending_update(presenter: &mut Presenter) -> std::sync::mpsc::Sender<UpdateState> {
    let (sender, receiver) = std::sync::mpsc::channel();
    presenter.update_events = Some(receiver);
    presenter.model.updates.state = UpdateState::Checking;
    sender
}

pub(crate) fn update_package() -> std::sync::Arc<crate::model::updates::UpdatePackage> {
    use crate::model::updates::{UpdateAsset, UpdatePackage};
    std::sync::Arc::new(UpdatePackage {
        tag: "v1.0.0".into(),
        notes: "## Changes\n\n- Fix application updates.".into(),
        asset: UpdateAsset {
            name: "nexus-agent-v1.0.0-aarch64-apple-darwin.zip".into(),
            browser_download_url: "https://github.com/ji233-Sun/nexus-agent/releases/download/v1.0.0/nexus-agent-v1.0.0-aarch64-apple-darwin.zip".into(),
            size: 24,
            digest: Some(format!("sha256:{}", "0".repeat(64))),
        },
    })
}

pub(crate) fn seed_harness_installations(
    presenter: &mut Presenter,
) -> &mut BTreeMap<HarnessKind, crate::model::harness_installation::HarnessInstallation> {
    use crate::model::harness_installation::{
        HarnessInstallation, InstallMethod, InstallOption, MaintenanceCommand,
    };
    for harness in HarnessKind::ALL {
        let command = MaintenanceCommand {
            program: "/fake/installer".into(),
            args: vec!["install".into()],
            environment: BTreeMap::new(),
        };
        let installed = harness != HarnessKind::Codex;
        presenter.model.harness_manager.installations.insert(
            harness,
            HarnessInstallation {
                configured: harness.default_executable().into(),
                executable: installed
                    .then(|| format!("/fake/{}{}", "long-directory/".repeat(12), harness).into()),
                discovered_from_manager: false,
                version: installed.then(|| "1.0.0".into()),
                latest_version: Ok("2.0.0".into()),
                source: if harness == HarnessKind::Omp {
                    "自定义 / 未知来源".into()
                } else {
                    "Vite+ (vp)".into()
                },
                diagnostic: None,
                update: (harness == HarnessKind::Claude).then(|| command.clone()),
                install_options: if installed {
                    vec![]
                } else {
                    vec![InstallOption {
                        method: InstallMethod::VitePlus,
                        command,
                    }]
                },
            },
        );
    }
    &mut presenter.model.harness_manager.installations
}

pub(crate) fn pending_cli_installation(
    presenter: &mut Presenter,
) -> std::sync::mpsc::Sender<Result<()>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    presenter.cli_installation_result = Some(receiver);
    presenter.model.cli_installation_busy = true;
    presenter.model.cli_installation_message = None;
    sender
}

fn remote_status(presenter: &mut Presenter) -> String {
    let (reply, mut response) = tokio::sync::oneshot::channel();
    assert!(!presenter.handle_remote_command(RemoteCommand::GetState { reply }));
    response.try_recv().unwrap().status
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
            attachments: &[],
            workspace_id: None,
            permission_mode: nexus_domain::PermissionMode::AutoEdit,
            task_id: None,
            project_id: Some(archived_project.id),
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
                    attachments: &[],
                    workspace_id: None,
                    permission_mode: nexus_domain::PermissionMode::AutoEdit,
                    task_id: None,
                    project_id: Some(project.id),
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
    presenter.model.conversation.model_catalog = ModelCatalogState::Ready(claude_aliases());
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
    let ModelCatalogState::Loading { request_id, .. } =
        &presenter.model().conversation.model_catalog
    else {
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
        model.source = match presenter.model().conversation.selected_harness {
            HarnessKind::Claude => nexus_domain::ModelSource::ClaudeAliases,
            HarnessKind::Codex => nexus_domain::ModelSource::CodexAppServer,
            HarnessKind::Omp => nexus_domain::ModelSource::OmpCli,
            HarnessKind::Pi => nexus_domain::ModelSource::PiRpc,
            HarnessKind::Kimi => nexus_domain::ModelSource::KimiAcp,
            HarnessKind::Qoder => nexus_domain::ModelSource::QoderAcp,
            HarnessKind::QoderCn => nexus_domain::ModelSource::QoderCnAcp,
            HarnessKind::Codebuddy => nexus_domain::ModelSource::CodebuddyAcp,
            HarnessKind::Opencode => nexus_domain::ModelSource::OpencodeAcp,
            HarnessKind::Deepseek => nexus_domain::ModelSource::DeepseekAcp,
            HarnessKind::CommandCode => nexus_domain::ModelSource::CommandCodeCli,
        };
    }
    runner.emit(Event::ModelCatalogLoaded {
        request_id: current_catalog_request_id(presenter),
        harness: presenter.model().conversation.selected_harness,
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
