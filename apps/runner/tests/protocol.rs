use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    time::Duration,
};

use nexus_domain::{HarnessKind, RunStatus, ThinkingEffort};
use nexus_protocol::{
    Command, CommandEnvelope, EnvironmentVariable, Event, EventEnvelope, StartRun,
};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout},
    time::timeout,
};
use uuid::Uuid;

fn fake_harness(directory: &Path) -> PathBuf {
    let executable = directory.join(format!("fake-harness{}", std::env::consts::EXE_SUFFIX));
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_harness.rs");
    let output = ProcessCommand::new("rustc")
        .args(["--edition=2024", "-D", "warnings"])
        .arg(fixture)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    executable
}

struct TestRunner {
    child: Child,
    stdin: ChildStdin,
    events: Lines<BufReader<ChildStdout>>,
}

impl TestRunner {
    fn spawn() -> Self {
        Self::spawn_command(&mut tokio::process::Command::new(env!(
            "CARGO_BIN_EXE_nexus-runner"
        )))
    }

    fn spawn_command(command: &mut tokio::process::Command) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let events = BufReader::new(child.stdout.take().unwrap()).lines();
        Self {
            child,
            stdin,
            events,
        }
    }

    async fn send(&mut self, command: Command) {
        let mut frame = serde_json::to_vec(&CommandEnvelope::new(command)).unwrap();
        frame.push(b'\n');
        self.stdin.write_all(&frame).await.unwrap();
        self.stdin.flush().await.unwrap();
    }

    async fn next(&mut self) -> Event {
        let line = timeout(Duration::from_secs(15), self.events.next_line())
            .await
            .expect("runner event timeout")
            .unwrap()
            .expect("runner closed its event stream");
        serde_json::from_str::<EventEnvelope>(&line).unwrap().event
    }

    async fn expect_runner_ready(&mut self) {
        loop {
            match self.next().await {
                Event::RunnerReady => return,
                Event::TaskTitleGenerated { .. } => {}
                event => panic!("expected runner.ready, got {event:?}"),
            }
        }
    }

    async fn collect_run(&mut self, run_id: Uuid, expected: RunStatus) -> Vec<Event> {
        let mut events = Vec::new();
        loop {
            let event = self.next().await;
            if let Event::RunExited {
                run_id: id, status, ..
            } = &event
            {
                assert_eq!(*id, run_id);
                assert_eq!(*status, expected);
                return events;
            }
            events.push(event);
        }
    }

    async fn collect_run_and_title(
        &mut self,
        run_id: Uuid,
        task_id: Uuid,
        expected: RunStatus,
    ) -> (Vec<Event>, String) {
        let mut events = Vec::new();
        let mut run_finished = false;
        let mut title = None;
        while !run_finished || title.is_none() {
            let event = self.next().await;
            match &event {
                Event::RunExited {
                    run_id: id, status, ..
                } if *id == run_id => {
                    assert_eq!(*status, expected);
                    run_finished = true;
                }
                Event::TaskTitleGenerated {
                    task_id: id,
                    title: generated,
                } if *id == task_id => title = Some(generated.clone()),
                _ => {}
            }
            events.push(event);
        }
        (events, title.unwrap())
    }

    async fn shutdown(mut self) {
        self.send(Command::RunnerShutdown).await;
        self.wait_for_exit().await;
    }

    async fn wait_for_exit(mut self) {
        drop(self.stdin);
        assert!(
            timeout(Duration::from_secs(10), self.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
}

fn request(directory: &Path, executable: PathBuf, harness: HarnessKind, prompt: &str) -> StartRun {
    StartRun {
        title_generation: Some(nexus_protocol::TitleGenerationConfig {
            harness,
            executable: executable.to_string_lossy().into_owned(),
            model: None,
            effort: ThinkingEffort::Default,
            environment: Vec::new(),
        }),
        permission_mode: nexus_domain::PermissionMode::AutoEdit,
        run_id: Uuid::new_v4(),
        task_id: Uuid::new_v4(),
        session_id: None,
        cwd: directory.to_string_lossy().into_owned(),
        prompt: prompt.into(),
        harness,
        executable: executable.to_string_lossy().into_owned(),
        model: None,
        effort: ThinkingEffort::High,
        environment: Vec::new(),
    }
}

async fn next_approval(runner: &mut TestRunner, run_id: Uuid) -> nexus_protocol::ApprovalRequest {
    loop {
        match runner.next().await {
            Event::RunApprovalRequested {
                run_id: id,
                request,
            } => {
                assert_eq!(id, run_id);
                return request;
            }
            Event::RunExited { .. } => panic!("run ended before approval"),
            _ => {}
        }
    }
}

#[tokio::test]
async fn two_checkouts_run_together_and_cancel_and_approval_are_scoped() {
    let fixtures = tempfile::tempdir().unwrap();
    let executable = fake_harness(fixtures.path());
    let first_dir = tempfile::tempdir().unwrap();
    let second_dir = tempfile::tempdir().unwrap();
    let third_dir = tempfile::tempdir().unwrap();
    let mut first = request(
        first_dir.path(),
        executable.clone(),
        HarnessKind::Claude,
        "approval-round-trip",
    );
    first.title_generation = None;
    let mut second = request(
        second_dir.path(),
        executable.clone(),
        HarnessKind::Codex,
        "approval-round-trip",
    );
    second.title_generation = None;
    let mut runner = TestRunner::spawn();
    runner.send(Command::RunStart(first.clone())).await;
    let first_approval = next_approval(&mut runner, first.run_id).await;

    let mut same_checkout = request(
        first_dir.path(),
        executable.clone(),
        HarnessKind::Claude,
        "must not launch",
    );
    same_checkout.title_generation = None;
    runner.send(Command::RunStart(same_checkout.clone())).await;
    let rejected = runner
        .collect_run(same_checkout.run_id, RunStatus::Failed)
        .await;
    assert!(rejected.iter().any(|event| matches!(
        event,
        Event::RunFailed {
            code: nexus_protocol::ErrorCode::RunAlreadyActive,
            ..
        }
    )));

    runner.send(Command::RunStart(second.clone())).await;
    let second_approval = next_approval(&mut runner, second.run_id).await;
    let mut third = request(
        third_dir.path(),
        executable,
        HarnessKind::Claude,
        "must wait",
    );
    third.title_generation = None;
    runner.send(Command::RunStart(third.clone())).await;
    let rejected = runner.collect_run(third.run_id, RunStatus::Failed).await;
    assert!(rejected.iter().any(|event| matches!(
        event,
        Event::RunFailed {
            code: nexus_protocol::ErrorCode::RunAlreadyActive,
            ..
        }
    )));

    runner
        .send(Command::RunApprovalRespond {
            run_id: second.run_id,
            request_id: first_approval.request_id,
            option: Some(0),
        })
        .await;
    loop {
        if let Event::RunApprovalRejected {
            run_id, request_id, ..
        } = runner.next().await
        {
            assert_eq!(run_id, second.run_id);
            assert_eq!(request_id, first_approval.request_id);
            break;
        }
    }
    runner
        .send(Command::RunCancel {
            run_id: first.run_id,
        })
        .await;
    runner.collect_run(first.run_id, RunStatus::Cancelled).await;
    runner
        .send(Command::RunApprovalRespond {
            run_id: second.run_id,
            request_id: second_approval.request_id,
            option: Some(0),
        })
        .await;
    let events = runner
        .collect_run(second.run_id, RunStatus::Completed)
        .await;
    assert!(events.iter().any(|event| matches!(event, Event::RunMessageCompleted { run_id, text } if *run_id == second.run_id && text.contains("approved"))));
    runner.shutdown().await;
}

#[tokio::test]
async fn approval_round_trip_for_each_harness_rejects_invalid_and_duplicate_responses() {
    let fixtures = tempfile::tempdir().unwrap();
    let executable = fake_harness(fixtures.path());
    for harness in HarnessKind::ALL {
        for option in [0, 1] {
            let directory = tempfile::tempdir().unwrap();
            let mut request = request(
                directory.path(),
                executable.clone(),
                harness,
                "approval-round-trip",
            );
            request.permission_mode = nexus_domain::PermissionMode::Ask;
            let run_id = request.run_id;
            let mut runner = TestRunner::spawn();
            runner.send(Command::RunStart(request)).await;
            let approval = next_approval(&mut runner, run_id).await;
            assert!(approval.details.contains("echo approved"));
            assert!(!directory.path().join("approval-response.json").exists());
            for (run, id, selected) in [
                (Uuid::new_v4(), approval.request_id, option),
                (run_id, Uuid::new_v4(), option),
                (run_id, approval.request_id, 99),
            ] {
                runner
                    .send(Command::RunApprovalRespond {
                        run_id: run,
                        request_id: id,
                        option: Some(selected),
                    })
                    .await;
                loop {
                    match runner.next().await {
                        Event::RunApprovalRejected {
                            run_id: rejected_run,
                            request_id,
                            ..
                        } => {
                            assert_eq!((rejected_run, request_id), (run, id));
                            break;
                        }
                        Event::RunExited { .. } => panic!("invalid reply ended run"),
                        _ => {}
                    }
                }
                assert!(!directory.path().join("approval-response.json").exists());
            }
            let response = Command::RunApprovalRespond {
                run_id,
                request_id: approval.request_id,
                option: Some(option),
            };
            runner.send(response.clone()).await;
            runner.send(response).await;
            let events = runner.collect_run(run_id, RunStatus::Completed).await;
            assert!(events.iter().any(|event| matches!(event, Event::RunApprovalResolved { request_id, .. } if *request_id == approval.request_id)));
            assert!(events.iter().any(|event| matches!(event, Event::RunMessageCompleted { text, .. } if text == if option == 0 { "approved" } else { "denied" })));
            let response: serde_json::Value = serde_json::from_str(
                &fs::read_to_string(directory.path().join("approval-response.json")).unwrap(),
            )
            .unwrap();
            match harness {
                HarnessKind::Claude => {
                    assert_eq!(response["response"]["request_id"], "approval-1");
                    if option == 0 {
                        assert_eq!(
                            response["response"]["response"]["updatedInput"]["command"],
                            "echo approved"
                        );
                    }
                }
                HarnessKind::Codex => assert_eq!(response["id"], 99),
                HarnessKind::Omp => assert_eq!(response["id"], "approval-1"),
            }
            runner.shutdown().await;
        }
    }
}

#[tokio::test]
async fn pending_approvals_are_cleared_on_cancel_and_late_replies_never_reach_harness() {
    let fixtures = tempfile::tempdir().unwrap();
    let executable = fake_harness(fixtures.path());
    for harness in HarnessKind::ALL {
        let directory = tempfile::tempdir().unwrap();
        let request = request(
            directory.path(),
            executable.clone(),
            harness,
            "approval-cancel",
        );
        let run_id = request.run_id;
        let mut runner = TestRunner::spawn();
        runner.send(Command::RunStart(request)).await;
        let approval = next_approval(&mut runner, run_id).await;
        runner.send(Command::RunCancel { run_id }).await;
        runner
            .send(Command::RunApprovalRespond {
                run_id,
                request_id: approval.request_id,
                option: Some(0),
            })
            .await;
        runner.collect_run(run_id, RunStatus::Cancelled).await;
        assert!(!directory.path().join("approval-response.json").exists());
        runner.shutdown().await;
    }
}

#[tokio::test]
async fn native_approval_cancellation_removes_the_request_before_the_run_ends() {
    let fixtures = tempfile::tempdir().unwrap();
    let executable = fake_harness(fixtures.path());
    for harness in HarnessKind::ALL {
        let directory = tempfile::tempdir().unwrap();
        let request = request(
            directory.path(),
            executable.clone(),
            harness,
            "approval-native-cancel",
        );
        let run_id = request.run_id;
        let mut runner = TestRunner::spawn();
        runner.send(Command::RunStart(request)).await;
        let approval = next_approval(&mut runner, run_id).await;
        fs::write(directory.path().join("resolve-approval"), "resolve").unwrap();
        loop {
            match runner.next().await {
                Event::RunApprovalResolved { request_id, .. } => {
                    assert_eq!(request_id, approval.request_id);
                    break;
                }
                Event::RunExited { .. } => panic!("run ended while awaiting native cancellation"),
                _ => {}
            }
        }
        runner
            .send(Command::RunApprovalRespond {
                run_id,
                request_id: approval.request_id,
                option: Some(0),
            })
            .await;
        loop {
            match runner.next().await {
                Event::RunApprovalRejected { request_id, .. } => {
                    assert_eq!(request_id, approval.request_id);
                    break;
                }
                Event::RunExited { .. } => panic!("stale reply reached harness"),
                _ => {}
            }
        }
        fs::write(directory.path().join("finish-turn"), "finish").unwrap();
        runner.collect_run(run_id, RunStatus::Completed).await;
        runner.shutdown().await;
    }
}

#[tokio::test]
async fn omp_approval_timeout_cancels_without_granting_permission() {
    let directory = tempfile::tempdir().unwrap();
    let request = request(
        directory.path(),
        fake_harness(directory.path()),
        HarnessKind::Omp,
        "approval-timeout",
    );
    let run_id = request.run_id;
    let mut runner = TestRunner::spawn();
    runner.send(Command::RunStart(request)).await;
    let events = runner.collect_run(run_id, RunStatus::Completed).await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::RunApprovalResolved { .. }))
    );
    assert!(
        events.iter().any(
            |event| matches!(event, Event::RunMessageCompleted { text, .. } if text == "denied")
        )
    );
    let response: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("approval-response.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(response["cancelled"], true);
    runner.shutdown().await;
}

#[tokio::test]
async fn exited_run_releases_the_slot_before_the_next_message_starts() {
    let directory = tempfile::tempdir().unwrap();
    let executable = fake_harness(directory.path());
    let mut runner = TestRunner::spawn();
    for _ in 0..10 {
        let request = request(
            directory.path(),
            executable.clone(),
            HarnessKind::Claude,
            "next message",
        );
        let run_id = request.run_id;
        runner.send(Command::RunStart(request)).await;
        runner.collect_run(run_id, RunStatus::Completed).await;
    }
    runner.shutdown().await;
}

#[tokio::test]
async fn runner_resumes_each_harness_session_across_processes() {
    let directory = tempfile::tempdir().unwrap();
    let executable = fake_harness(directory.path());
    for harness in HarnessKind::ALL {
        let mut request = request(
            directory.path(),
            executable.clone(),
            harness,
            "remember this",
        );
        let mut runner = TestRunner::spawn();
        runner.send(Command::RunStart(request.clone())).await;
        let events = runner
            .collect_run(request.run_id, RunStatus::Completed)
            .await;
        let session_id = events
            .iter()
            .find_map(|event| match event {
                Event::RunSessionStarted { run_id, session_id } if *run_id == request.run_id => {
                    Some(session_id.clone())
                }
                _ => None,
            })
            .expect("the harness must report its session ID");
        runner.shutdown().await;

        // Restart the runner as well as the harness to require native persistence.
        let mut runner = TestRunner::spawn();
        request.run_id = Uuid::new_v4();
        request.prompt = "follow-up".into();
        request.session_id = Some(session_id.clone());
        runner.send(Command::RunStart(request.clone())).await;
        let events = runner
            .collect_run(request.run_id, RunStatus::Completed)
            .await;
        runner.shutdown().await;
        assert!(events.iter().any(|event| matches!(event,
            Event::RunSessionStarted { session_id: id, .. } if id == &session_id)));
        assert!(events.iter().any(|event| matches!(event,
            Event::RunMessageCompleted { text, .. } if text == "remember this")));
        assert_eq!(
            fs::read_to_string(directory.path().join("stdin.txt")).unwrap(),
            "follow-up"
        );
        let args_file = match harness {
            HarnessKind::Claude => "args.txt",
            HarnessKind::Codex => "codex-args.txt",
            HarnessKind::Omp => "omp-args.txt",
        };
        let args = fs::read_to_string(directory.path().join(args_file)).unwrap();
        if harness == HarnessKind::Codex {
            let frame: serde_json::Value = serde_json::from_str(
                &fs::read_to_string(directory.path().join("thread-params.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(frame["method"], "thread/resume");
            assert_eq!(frame["params"]["threadId"], session_id);
        } else {
            assert!(args.lines().any(|arg| arg == session_id));
        }
        assert!(!args.lines().any(|arg| matches!(
            arg,
            "--ephemeral" | "--no-session" | "--no-session-persistence" | "--last" | "--continue"
        )));
    }
}

#[tokio::test]
async fn runner_streams_fake_claude_and_forwards_model_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let mut request = request(
        directory.path(),
        fake_harness(directory.path()),
        HarnessKind::Claude,
        "test prompt",
    );
    request.model = Some("opus".into());
    request.effort = ThinkingEffort::XHigh;
    let run_id = request.run_id;
    let mut runner = TestRunner::spawn();
    runner.send(Command::RunStart(request)).await;
    let events = runner.collect_run(run_id, RunStatus::Completed).await;
    runner.shutdown().await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::RunStarted { run_id: id, .. } if *id == run_id))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::RunOutputDelta { text, .. } if text == "hello"))
    );
    assert!(
        events.iter().any(
            |event| matches!(event, Event::RunMessageCompleted { text, .. } if text == "hello")
        )
    );
    let args = fs::read_to_string(directory.path().join("args.txt")).unwrap();
    let args = args.lines().collect::<Vec<_>>();
    assert!(args.windows(2).any(|pair| pair == ["--model", "opus"]));
    assert!(args.windows(2).any(|pair| pair == ["--effort", "xhigh"]));
    assert!(!args.contains(&"test prompt"));
}

#[tokio::test]
async fn runner_streams_fake_codex_and_preserves_workspace_permissions() {
    let directory = tempfile::tempdir().unwrap();
    let request = request(
        directory.path(),
        fake_harness(directory.path()),
        HarnessKind::Codex,
        "test prompt",
    );
    let run_id = request.run_id;
    let mut runner = TestRunner::spawn();
    runner.send(Command::RunStart(request)).await;
    let events = runner.collect_run(run_id, RunStatus::Completed).await;
    runner.shutdown().await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::RunStarted { run_id: id, .. } if *id == run_id))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::RunToolStarted { name, .. } if name == "Command"))
    );
    assert!(events.iter().any(|event| matches!(event, Event::RunToolCompleted { output, is_error: false, .. } if output == "project")));
    assert!(
        events.iter().any(
            |event| matches!(event, Event::RunMessageCompleted { text, .. } if text == "done")
        )
    );
    let args = fs::read_to_string(directory.path().join("codex-args.txt")).unwrap();
    let args = args.lines().collect::<Vec<_>>();
    assert_eq!(args.first().copied(), Some("app-server"));
    let thread: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("thread-params.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(thread["params"]["sandbox"], "workspace-write");
    assert_eq!(thread["params"]["approvalPolicy"], "on-request");
    let turn: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("turn-params.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(turn["params"]["effort"], "high");
    assert!(!args.contains(&"test prompt"));
}

#[tokio::test]
async fn runner_routes_claude_alias_catalog_without_starting_a_run() {
    let directory = tempfile::tempdir().unwrap();
    let executable = fake_harness(directory.path());
    let request_id = Uuid::new_v4();
    let mut runner = TestRunner::spawn();
    runner
        .send(Command::ModelCatalogRefresh {
            context_id: None,
            purpose: Default::default(),
            request_id,
            harness: HarnessKind::Claude,
            executable: executable.to_string_lossy().into_owned(),
            cwd: directory.path().to_string_lossy().into_owned(),
            environment: vec![EnvironmentVariable {
                name: "ANTHROPIC_API_KEY".into(),
                value: "catalog-secret".into(),
            }],
        })
        .await;
    let event = runner.next().await;
    assert!(!format!("{event:?}").contains("catalog-secret"));
    assert!(
        matches!(event, Event::ModelCatalogLoaded { request_id: id, harness: HarnessKind::Claude, models }
        if id == request_id && models.len() == 3 && models.iter().all(|model|
            model.source.harness() == HarnessKind::Claude && model.supported_reasoning_efforts.is_empty()))
    );
    runner.send(Command::RunnerHello).await;
    runner.expect_runner_ready().await;
    runner.shutdown().await;
}

#[tokio::test]
async fn runner_loads_all_codex_model_pages_and_reaps_the_app_server() {
    let directory = tempfile::tempdir().unwrap();
    let executable = fake_harness(directory.path());
    let request_id = Uuid::new_v4();
    let mut runner = TestRunner::spawn();
    runner
        .send(Command::ModelCatalogRefresh {
            context_id: None,
            purpose: Default::default(),
            request_id,
            harness: HarnessKind::Codex,
            executable: executable.to_string_lossy().into_owned(),
            cwd: directory.path().to_string_lossy().into_owned(),
            environment: Vec::new(),
        })
        .await;

    let Event::ModelCatalogLoaded {
        request_id: received_id,
        harness,
        models,
    } = runner.next().await
    else {
        panic!("expected model catalog")
    };

    assert_eq!(received_id, request_id);
    assert_eq!(harness, HarnessKind::Codex);
    assert_eq!(
        models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["gpt-first", "gpt-second"]
    );
    assert!(models[0].is_default);
    assert_eq!(
        models[1].default_reasoning_effort,
        Some(ThinkingEffort::Ultra)
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("catalog-stopped.txt")).unwrap(),
        "stopped"
    );
    runner.shutdown().await;
}

#[tokio::test]
async fn omp_probe_times_out_in_both_stages() {
    let directory = tempfile::tempdir().unwrap();
    let executable = fake_harness(directory.path());
    for (variable, available, message) in [
        ("TEST_OMP_VERSION_BLOCK", false, "版本探测超时"),
        ("TEST_OMP_CATALOG_BLOCK", true, "模型探测超时"),
    ] {
        let mut runner = TestRunner::spawn_command(
            tokio::process::Command::new(env!("CARGO_BIN_EXE_nexus-runner"))
                .current_dir(directory.path())
                .env(variable, "1"),
        );
        let started = std::time::Instant::now();
        runner
            .send(Command::HarnessProbe {
                harness: HarnessKind::Omp,
                executable: executable.to_string_lossy().into_owned(),
            })
            .await;
        let line = timeout(Duration::from_secs(65), runner.events.next_line())
            .await
            .expect("probe must time out")
            .unwrap()
            .unwrap();
        assert!(started.elapsed() >= Duration::from_secs(60));
        let Event::HarnessDetected(probe) =
            serde_json::from_str::<EventEnvelope>(&line).unwrap().event
        else {
            panic!("expected probe result")
        };
        assert_eq!(probe.available, available);
        assert!(!probe.authenticated);
        assert!(probe.message.contains(message), "{}", probe.message);
        assert_eq!(
            probe.version.as_deref(),
            available.then_some("fake-omp 1.0")
        );
        runner.shutdown().await;
    }
}

#[tokio::test]
async fn blocked_omp_probe_does_not_block_commands_or_shutdown() {
    let fixtures = tempfile::tempdir().unwrap();
    let executable = fake_harness(fixtures.path());
    for cancel in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let mut runner = TestRunner::spawn_command(
            tokio::process::Command::new(env!("CARGO_BIN_EXE_nexus-runner"))
                .current_dir(directory.path())
                .env("TEST_OMP_CATALOG_BLOCK", "1"),
        );
        let mut request = request(
            directory.path(),
            executable.clone(),
            HarnessKind::Omp,
            "approval-round-trip",
        );
        request.title_generation = None;
        request.permission_mode = nexus_domain::PermissionMode::Ask;
        let run_id = request.run_id;
        runner.send(Command::RunStart(request)).await;
        let approval = next_approval(&mut runner, run_id).await;
        runner
            .send(Command::HarnessProbe {
                harness: HarnessKind::Omp,
                executable: executable.to_string_lossy().into_owned(),
            })
            .await;
        timeout(Duration::from_secs(5), async {
            while !directory.path().join("omp-catalog-cwd.txt").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            runner.send(Command::RunnerHello).await;
            runner.expect_runner_ready().await;
            runner.send(if cancel {
                Command::RunCancel { run_id }
            } else {
                Command::RunApprovalRespond { run_id, request_id: approval.request_id, option: Some(0) }
            }).await;
            let events = runner.collect_run(run_id, if cancel { RunStatus::Cancelled } else { RunStatus::Completed }).await;
            assert!(!events.iter().any(|event| matches!(event, Event::HarnessDetected(_))));
            if !cancel {
                assert!(events.iter().any(|event| matches!(event, Event::RunMessageCompleted { text, .. } if text == "approved")));
            }
            runner.shutdown().await;
        }).await.expect("probe blocked command handling or shutdown");
    }
}

#[tokio::test]
async fn runner_loads_omp_catalog_with_provider_context_and_reaps_the_command() {
    let directory = tempfile::tempdir().unwrap();
    let executable = fake_harness(directory.path());
    let request_id = Uuid::new_v4();
    let mut runner = TestRunner::spawn();
    runner
        .send(Command::ModelCatalogRefresh {
            context_id: None,
            purpose: Default::default(),
            request_id,
            harness: HarnessKind::Omp,
            executable: executable.to_string_lossy().into_owned(),
            cwd: directory.path().to_string_lossy().into_owned(),
            environment: vec![EnvironmentVariable {
                name: "TEST_PROVIDER_API_KEY".into(),
                value: "catalog-secret".into(),
            }],
        })
        .await;

    let Event::ModelCatalogLoaded {
        request_id: received_id,
        harness,
        models,
    } = runner.next().await
    else {
        panic!("expected OMP model catalog")
    };

    assert_eq!(received_id, request_id);
    assert_eq!(harness, HarnessKind::Omp);
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "alpha/shared-model");
    assert_eq!(models[0].provider.as_deref(), Some("alpha"));
    assert_eq!(models[1].id, "beta/shared-model");
    assert_eq!(models[1].provider.as_deref(), Some("beta"));
    assert_eq!(models[0].display_name, models[1].display_name);
    assert!(models[0].supports_effort(&ThinkingEffort::Off));
    assert!(models[0].supports_effort(&ThinkingEffort::Auto));
    assert_eq!(
        PathBuf::from(fs::read_to_string(directory.path().join("omp-catalog-cwd.txt")).unwrap())
            .canonicalize()
            .unwrap(),
        directory.path().canonicalize().unwrap()
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("omp-catalog-env.txt")).unwrap(),
        "catalog-secret"
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("omp-catalog-stopped.txt")).unwrap(),
        "stopped"
    );
    runner.shutdown().await;
}

#[tokio::test]
async fn omp_catalog_reports_command_json_and_empty_states_without_leaking_stderr() {
    let directory = tempfile::tempdir().unwrap();
    let executable = fake_harness(directory.path());
    let mut runner = TestRunner::spawn();
    for (name, expected) in [
        ("TEST_OMP_CATALOG_ERROR", "命令执行失败"),
        ("TEST_OMP_CATALOG_MALFORMED", "无效 JSON"),
    ] {
        let request_id = Uuid::new_v4();
        runner
            .send(Command::ModelCatalogRefresh {
                context_id: None,
                purpose: Default::default(),
                request_id,
                harness: HarnessKind::Omp,
                executable: executable.to_string_lossy().into_owned(),
                cwd: directory.path().to_string_lossy().into_owned(),
                environment: vec![EnvironmentVariable {
                    name: name.into(),
                    value: "1".into(),
                }],
            })
            .await;
        assert!(matches!(
            runner.next().await,
            Event::ModelCatalogFailed { request_id: id, message, .. }
                if id == request_id
                    && message.contains(expected)
                    && !message.contains("test-secret-must-not-leak")
        ));
    }

    let request_id = Uuid::new_v4();
    runner
        .send(Command::ModelCatalogRefresh {
            context_id: None,
            purpose: Default::default(),
            request_id,
            harness: HarnessKind::Omp,
            executable: executable.to_string_lossy().into_owned(),
            cwd: directory.path().to_string_lossy().into_owned(),
            environment: vec![EnvironmentVariable {
                name: "TEST_OMP_CATALOG_EMPTY".into(),
                value: "1".into(),
            }],
        })
        .await;
    assert!(matches!(
        runner.next().await,
        Event::ModelCatalogLoaded { request_id: id, models, .. }
            if id == request_id && models.is_empty()
    ));
    runner.shutdown().await;
}

#[tokio::test]
async fn newer_omp_catalog_request_cancels_and_reaps_the_previous_command() {
    let directory = tempfile::tempdir().unwrap();
    let executable = fake_harness(directory.path());
    let stale_id = Uuid::new_v4();
    let current_id = Uuid::new_v4();
    let command = |request_id, environment| Command::ModelCatalogRefresh {
        context_id: None,
        purpose: Default::default(),
        request_id,
        harness: HarnessKind::Omp,
        executable: executable.to_string_lossy().into_owned(),
        cwd: directory.path().to_string_lossy().into_owned(),
        environment,
    };
    let mut runner = TestRunner::spawn();
    runner
        .send(command(
            stale_id,
            vec![EnvironmentVariable {
                name: "TEST_OMP_CATALOG_BLOCK".into(),
                value: "1".into(),
            }],
        ))
        .await;
    runner.send(command(current_id, Vec::new())).await;

    assert!(matches!(
        runner.next().await,
        Event::ModelCatalogLoaded { request_id, harness: HarnessKind::Omp, .. }
            if request_id == current_id
    ));
    runner.shutdown().await;
}

#[tokio::test]
async fn newer_catalog_requests_cancel_older_app_servers_without_stale_events() {
    let directory = tempfile::tempdir().unwrap();
    let executable = fake_harness(directory.path());
    let stale_id = Uuid::new_v4();
    let current_id = Uuid::new_v4();
    let command = |request_id, environment| Command::ModelCatalogRefresh {
        context_id: None,
        purpose: Default::default(),
        request_id,
        harness: HarnessKind::Codex,
        executable: executable.to_string_lossy().into_owned(),
        cwd: directory.path().to_string_lossy().into_owned(),
        environment,
    };
    let mut runner = TestRunner::spawn();
    runner
        .send(command(
            stale_id,
            vec![EnvironmentVariable {
                name: "TEST_CATALOG_BLOCK".into(),
                value: "1".into(),
            }],
        ))
        .await;
    runner.send(command(current_id, Vec::new())).await;

    assert!(matches!(
        runner.next().await,
        Event::ModelCatalogLoaded { request_id, .. } if request_id == current_id
    ));
    runner.shutdown().await;
}

#[tokio::test]
async fn catalog_request_errors_are_explicit_and_retriable() {
    let directory = tempfile::tempdir().unwrap();
    let executable = fake_harness(directory.path());
    let request_id = Uuid::new_v4();
    let mut runner = TestRunner::spawn();
    runner
        .send(Command::ModelCatalogRefresh {
            context_id: None,
            purpose: Default::default(),
            request_id,
            harness: HarnessKind::Codex,
            executable: executable.to_string_lossy().into_owned(),
            cwd: directory.path().to_string_lossy().into_owned(),
            environment: vec![EnvironmentVariable {
                name: "TEST_CATALOG_ERROR".into(),
                value: "1".into(),
            }],
        })
        .await;

    assert!(matches!(
        runner.next().await,
        Event::ModelCatalogFailed { request_id: id, message, .. }
            if id == request_id && message.contains("model/list unavailable")
    ));
    runner.shutdown().await;
}

#[tokio::test]
async fn runner_streams_fake_omp_and_uses_guarded_rpc_mode() {
    let directory = tempfile::tempdir().unwrap();
    let mut request = request(
        directory.path(),
        fake_harness(directory.path()),
        HarnessKind::Omp,
        "test prompt",
    );
    request.model = Some("deepseek/deepseek-v4-pro".into());
    request.environment.push(EnvironmentVariable {
        name: "TEST_PROVIDER_API_KEY".into(),
        value: "test-secret".into(),
    });
    let run_id = request.run_id;
    let mut runner = TestRunner::spawn();
    runner.send(Command::RunStart(request)).await;
    let events = runner.collect_run(run_id, RunStatus::Completed).await;
    runner.shutdown().await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::RunOutputDelta { text, .. } if text == "hello"))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::RunToolStarted { name, .. } if name == "read"))
    );
    assert!(events.iter().any(|event| matches!(event, Event::RunToolCompleted { output, is_error: false, .. } if output == "project")));
    assert!(
        events.iter().any(
            |event| matches!(event, Event::RunMessageCompleted { text, .. } if text == "done")
        )
    );
    let args = fs::read_to_string(directory.path().join("omp-args.txt")).unwrap();
    let args = args.lines().collect::<Vec<_>>();
    assert!(args.windows(2).any(|pair| pair == ["--mode", "rpc-ui"]));
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--approval-mode", "write"])
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--model", "deepseek/deepseek-v4-pro"])
    );
    assert!(!args.contains(&"test prompt"));
    assert_eq!(
        fs::read_to_string(directory.path().join("provider-env.txt")).unwrap(),
        "test-secret"
    );
}

#[tokio::test]
async fn runner_generates_titles_with_each_harness_in_a_safe_background_process() {
    for (harness, effort) in HarnessKind::ALL.into_iter().flat_map(|harness| {
        [ThinkingEffort::Default, ThinkingEffort::Low].map(|effort| (harness, effort))
    }) {
        let directory = tempfile::tempdir().unwrap();
        let executable = fake_harness(directory.path());
        let title_executable = directory
            .path()
            .join(format!("title-harness{}", std::env::consts::EXE_SUFFIX));
        fs::copy(&executable, &title_executable).unwrap();
        let mut request = request(
            directory.path(),
            executable,
            if harness == HarnessKind::Claude {
                HarnessKind::Codex
            } else {
                HarnessKind::Claude
            },
            "Please fix the authentication flow and add regression tests",
        );
        request.model = Some("conversation-model".into());
        request.environment.push(EnvironmentVariable {
            name: "TEST_PROVIDER_API_KEY".into(),
            value: "conversation-secret".into(),
        });
        request.title_generation = Some(nexus_protocol::TitleGenerationConfig {
            harness,
            executable: title_executable.to_string_lossy().into_owned(),
            model: Some("title-model".into()),
            effort,
            environment: vec![EnvironmentVariable {
                name: "TEST_PROVIDER_API_KEY".into(),
                value: "title-secret".into(),
            }],
        });
        let run_id = request.run_id;
        let task_id = request.task_id;
        let mut runner = TestRunner::spawn();
        runner.send(Command::RunStart(request)).await;

        let (_, title) = runner
            .collect_run_and_title(run_id, task_id, RunStatus::Completed)
            .await;
        runner.shutdown().await;

        assert_eq!(title, "Fix authentication flow");
        let args = fs::read_to_string(directory.path().join("title-args.txt")).unwrap();
        assert!(!args.contains("Please fix the authentication flow"));
        assert!(args.contains("--model\ntitle-model"));
        assert!(!args.contains("conversation-model"));
        assert!(!args.contains("high"));
        if effort.is_default() {
            assert!(!args.contains("--effort"));
            assert!(!args.contains("model_reasoning_effort"));
            assert!(!args.contains("--thinking"));
        } else {
            assert!(args.contains(match harness {
                HarnessKind::Claude => "--effort\nlow",
                HarnessKind::Codex => "--config\nmodel_reasoning_effort=\"low\"",
                HarnessKind::Omp => "--thinking\nlow",
            }));
        }
        assert_eq!(
            PathBuf::from(
                fs::read_to_string(directory.path().join("title-executable.txt")).unwrap()
            )
            .canonicalize()
            .unwrap(),
            title_executable.canonicalize().unwrap(),
        );
        assert_eq!(
            fs::read_to_string(directory.path().join("provider-env.txt")).unwrap(),
            "conversation-secret"
        );
        match harness {
            HarnessKind::Claude => {
                assert!(args.contains("--permission-mode\ndontAsk"));
                assert!(args.ends_with("--tools\n"));
            }
            HarnessKind::Codex => {
                assert!(args.contains("--sandbox\nread-only"));
                assert!(args.contains("--ignore-rules"));
                assert!(!args.contains("workspace-write"));
            }
            HarnessKind::Omp => {
                assert!(args.contains("--no-tools"));
                assert!(args.contains("--no-extensions"));
                assert!(args.contains("--no-rules"));
            }
        }
        assert!(
            fs::read_to_string(directory.path().join("title-prompt.txt"))
                .unwrap()
                .contains("Please fix the authentication flow")
        );
        assert_eq!(
            fs::read_to_string(directory.path().join("title-provider-env.txt")).unwrap(),
            "title-secret"
        );
    }
}

#[tokio::test]
async fn title_generation_failure_does_not_fail_the_conversation() {
    let binaries = tempfile::tempdir().unwrap();
    let executable = fake_harness(binaries.path());
    let directory = tempfile::tempdir().unwrap();
    let mut request = request(
        directory.path(),
        executable,
        HarnessKind::Claude,
        "keep the fallback title",
    );
    request.title_generation.as_mut().unwrap().executable = directory
        .path()
        .join("missing-harness")
        .to_string_lossy()
        .into_owned();
    let run_id = request.run_id;
    let mut runner = TestRunner::spawn();
    runner.send(Command::RunStart(request)).await;
    let events = runner.collect_run(run_id, RunStatus::Completed).await;
    runner.shutdown().await;
    assert!(!events.iter().any(|event| matches!(
        event,
        Event::RunFailed { .. } | Event::TaskTitleGenerated { .. }
    )));
    assert!(!directory.path().join("title-args.txt").exists());
}

#[tokio::test]
async fn steer_waits_for_all_tools_and_uses_native_receipts_in_the_same_run() {
    let binaries = tempfile::tempdir().unwrap();
    let executable = fake_harness(binaries.path());
    for harness in HarnessKind::ALL {
        for scenario in ["steer-tools", "steer-rejected", "steer-unconfirmed"] {
            if harness == HarnessKind::Claude && scenario == "steer-rejected" {
                continue;
            }
            let directory = tempfile::tempdir().unwrap();
            let request = request(directory.path(), executable.clone(), harness, scenario);
            let run_id = request.run_id;
            let message_id = Uuid::new_v4();
            let prompt = "corrected\n指令 \"quoted\"";
            let mut runner = TestRunner::spawn();
            runner.send(Command::RunStart(request)).await;
            loop {
                if matches!(runner.next().await, Event::RunToolStarted { tool_id, .. } if tool_id == "tool-2")
                {
                    break;
                }
            }
            runner
                .send(Command::RunSteer {
                    run_id,
                    message_id,
                    prompt: prompt.into(),
                })
                .await;
            runner.send(Command::RunnerHello).await;
            runner.expect_runner_ready().await;
            fs::write(directory.path().join("finish-first"), "ready").unwrap();
            loop {
                match runner.next().await {
                    Event::RunMessageCompleted { text, .. } if text == "first-tool-done" => break,
                    Event::RunInputAccepted { .. } | Event::RunExited { .. } => {
                        panic!("Steer must wait for the whole batch")
                    }
                    _ => {}
                }
            }
            let quiet_period = tokio::time::sleep(Duration::from_millis(100));
            tokio::pin!(quiet_period);
            loop {
                tokio::select! {
                    _ = &mut quiet_period => break,
                    event = runner.next() => assert!(
                        matches!(event, Event::TaskTitleGenerated { .. }),
                        "Steer must wait for the whole tool batch, got {event:?}"
                    ),
                }
            }
            assert!(!directory.path().join("steer-input.json").exists());
            fs::write(directory.path().join("finish-second"), "ready").unwrap();
            let expected = if scenario == "steer-unconfirmed" {
                RunStatus::Failed
            } else {
                RunStatus::Completed
            };
            let events = runner.collect_run(run_id, expected).await;
            assert!(
                events.iter().any(|event| match event {
                    Event::RunInputAccepted {
                        run_id: id,
                        message_id: received,
                    } => scenario == "steer-tools" && *id == run_id && *received == message_id,
                    Event::RunInputRejected {
                        run_id: id,
                        message_id: received,
                        ..
                    } => scenario != "steer-tools" && *id == run_id && *received == message_id,
                    _ => false,
                }),
                "missing native receipt for {harness} / {scenario}"
            );
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Event::RunStarted { .. }))
            );
            let frame: serde_json::Value = serde_json::from_str(
                &fs::read_to_string(directory.path().join("steer-input.json")).unwrap(),
            )
            .unwrap();
            match harness {
                HarnessKind::Codex => {
                    assert_eq!(frame["method"], "turn/steer");
                    assert_eq!(frame["params"]["expectedTurnId"], "turn-1");
                    assert_eq!(frame["params"]["input"][0]["text"], prompt);
                }
                HarnessKind::Claude => assert_eq!(frame["message"]["content"], prompt),
                HarnessKind::Omp => {
                    assert_eq!(frame["type"], "steer");
                    assert_eq!(frame["message"], prompt);
                }
            }
            runner.shutdown().await;
        }
    }
}

#[tokio::test]
async fn shutdown_reaps_a_blocked_title_process_tree() {
    let directory = tempfile::tempdir().unwrap();
    let mut request = request(
        directory.path(),
        fake_harness(directory.path()),
        HarnessKind::Codex,
        "generate a title",
    );
    request
        .title_generation
        .as_mut()
        .unwrap()
        .environment
        .push(EnvironmentVariable {
            name: "TEST_TITLE_BLOCK".into(),
            value: "1".into(),
        });
    let run_id = request.run_id;
    let mut runner = TestRunner::spawn();
    runner.send(Command::RunStart(request)).await;
    runner.collect_run(run_id, RunStatus::Completed).await;

    timeout(Duration::from_secs(5), async {
        while !directory.path().join("title-prompt.txt").is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    runner.shutdown().await;
}

#[tokio::test]
async fn unsent_steer_is_rejected_on_completion_or_cancellation() {
    let binaries = tempfile::tempdir().unwrap();
    let executable = fake_harness(binaries.path());
    for harness in HarnessKind::ALL {
        for cancel in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let request = request(
                directory.path(),
                executable.clone(),
                harness,
                "steer-no-tools",
            );
            let run_id = request.run_id;
            let message_id = Uuid::new_v4();
            let mut runner = TestRunner::spawn();
            runner.send(Command::RunStart(request)).await;
            loop {
                if matches!(runner.next().await, Event::RunMessageCompleted { text, .. } if text == "ready")
                {
                    break;
                }
            }
            runner
                .send(Command::RunSteer {
                    run_id,
                    message_id,
                    prompt: "keep queued".into(),
                })
                .await;
            runner.send(Command::RunnerHello).await;
            runner.expect_runner_ready().await;
            let expected = if cancel {
                runner.send(Command::RunCancel { run_id }).await;
                RunStatus::Cancelled
            } else {
                fs::write(directory.path().join("finish-turn"), "ready").unwrap();
                RunStatus::Completed
            };
            let events = runner.collect_run(run_id, expected).await;
            assert!(events.iter().any(|event| matches!(event,
                Event::RunInputRejected { run_id: id, message_id: received, .. } if *id == run_id && *received == message_id)));
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Event::RunInputAccepted { .. }))
            );
            runner.shutdown().await;
        }
    }
}

#[tokio::test]
async fn cancellation_and_shutdown_reap_the_harness_process_tree() {
    let directory = tempfile::tempdir().unwrap();
    let executable = fake_harness(directory.path());
    for shutdown in [false, true] {
        let request = request(
            directory.path(),
            executable.clone(),
            HarnessKind::Codex,
            "wait-for-cancel",
        );
        let run_id = request.run_id;
        let mut runner = TestRunner::spawn();
        runner.send(Command::RunStart(request)).await;
        loop {
            if matches!(runner.next().await, Event::RunMessageCompleted { text, .. } if text == "ready")
            {
                break;
            }
        }
        runner
            .send(if shutdown {
                Command::RunnerShutdown
            } else {
                Command::RunCancel { run_id }
            })
            .await;
        // 子进程继承 stdout；进程树未清理时，Runner 无法读到 EOF 并发出终态。
        runner.collect_run(run_id, RunStatus::Cancelled).await;
        if shutdown {
            runner.wait_for_exit().await;
        } else {
            runner.shutdown().await;
        }
    }
}
