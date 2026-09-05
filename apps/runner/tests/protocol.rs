use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    time::Duration,
};

use nexus_domain::{HarnessKind, RunStatus, ThinkingEffort};
use nexus_protocol::{Command, CommandEnvelope, Event, EventEnvelope, StartRun};
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
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_nexus-runner"))
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
        run_id: Uuid::new_v4(),
        task_id: Uuid::new_v4(),
        cwd: directory.to_string_lossy().into_owned(),
        prompt: prompt.into(),
        harness,
        executable: executable.to_string_lossy().into_owned(),
        model: None,
        effort: ThinkingEffort::High,
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
async fn runner_streams_fake_codex_and_uses_non_interactive_mode() {
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
    assert_eq!(args.first().copied(), Some("exec"));
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--sandbox", "workspace-write"])
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--config", "model_reasoning_effort=\"high\""])
    );
    assert_eq!(args.last().copied(), Some("-"));
    assert!(!args.contains(&"test prompt"));
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
