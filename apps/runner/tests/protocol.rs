#![cfg(unix)]

use std::{
    fs,
    io::{BufRead as _, BufReader, Write},
    os::unix::fs::PermissionsExt as _,
    process::{Command as ProcessCommand, Stdio},
};

use nexus_domain::{HarnessKind, RunStatus, ThinkingEffort};
use nexus_protocol::{Command, CommandEnvelope, Event, EventEnvelope, StartRun};
use uuid::Uuid;

#[test]
fn runner_streams_fake_claude_and_forwards_model_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let fake_claude = directory.path().join("claude");
    fs::write(
        &fake_claude,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$PWD/args.txt\"\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}}'\nprintf '%s\\n' '{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]}}'\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake_claude).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_claude, permissions).unwrap();

    let mut runner = ProcessCommand::new(env!("CARGO_BIN_EXE_nexus-runner"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = runner.stdin.take().unwrap();
    let stdout = runner.stdout.take().unwrap();
    let run_id = Uuid::new_v4();
    send(
        &mut stdin,
        Command::RunStart(StartRun {
            run_id,
            task_id: Uuid::new_v4(),
            cwd: directory.path().to_string_lossy().into_owned(),
            prompt: "test prompt".into(),
            harness: HarnessKind::Claude,
            executable: fake_claude.to_string_lossy().into_owned(),
            model: Some("opus".into()),
            effort: ThinkingEffort::XHigh,
        }),
    );

    let mut saw_started = false;
    let mut saw_delta = false;
    let mut saw_message = false;
    for line in BufReader::new(stdout).lines() {
        let event: EventEnvelope = serde_json::from_str(&line.unwrap()).unwrap();
        match event.event {
            Event::RunStarted { run_id: id, .. } if id == run_id => saw_started = true,
            Event::RunOutputDelta { run_id: id, text } if id == run_id && text == "hello" => {
                saw_delta = true;
            }
            Event::RunMessageCompleted { run_id: id, text } if id == run_id && text == "hello" => {
                saw_message = true;
            }
            Event::RunExited {
                run_id: id,
                status: RunStatus::Completed,
                ..
            } if id == run_id => break,
            _ => {}
        }
    }
    send(&mut stdin, Command::RunnerShutdown);
    drop(stdin);
    assert!(runner.wait().unwrap().success());
    assert!(saw_started && saw_delta && saw_message);

    let args = fs::read_to_string(directory.path().join("args.txt")).unwrap();
    assert!(
        args.lines()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|pair| pair == ["--model", "opus"])
    );
    assert!(
        args.lines()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|pair| pair == ["--effort", "xhigh"])
    );
    assert!(!args.contains("test prompt"));
}

#[test]
fn runner_streams_fake_codex_and_uses_non_interactive_mode() {
    let directory = tempfile::tempdir().unwrap();
    let fake_codex = directory.path().join("codex");
    fs::write(
        &fake_codex,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$PWD/codex-args.txt\"\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"thread-1\"}'\nprintf '%s\\n' '{\"type\":\"item.started\",\"item\":{\"id\":\"item-1\",\"type\":\"command_execution\",\"command\":\"pwd\",\"aggregated_output\":\"\",\"exit_code\":null,\"status\":\"in_progress\"}}'\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"id\":\"item-1\",\"type\":\"command_execution\",\"command\":\"pwd\",\"aggregated_output\":\"project\",\"exit_code\":0,\"status\":\"completed\"}}'\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"id\":\"item-2\",\"type\":\"agent_message\",\"text\":\"done\"}}'\nprintf '%s\\n' '{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,\"cached_input_tokens\":0,\"output_tokens\":1,\"reasoning_output_tokens\":0}}'\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake_codex).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_codex, permissions).unwrap();

    let mut runner = ProcessCommand::new(env!("CARGO_BIN_EXE_nexus-runner"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = runner.stdin.take().unwrap();
    let stdout = runner.stdout.take().unwrap();
    let run_id = Uuid::new_v4();
    send(
        &mut stdin,
        Command::RunStart(StartRun {
            run_id,
            task_id: Uuid::new_v4(),
            cwd: directory.path().to_string_lossy().into_owned(),
            prompt: "test prompt".into(),
            harness: HarnessKind::Codex,
            executable: fake_codex.to_string_lossy().into_owned(),
            model: None,
            effort: ThinkingEffort::High,
        }),
    );

    let mut saw_started = false;
    let mut saw_tool_started = false;
    let mut saw_tool_completed = false;
    let mut saw_message = false;
    for line in BufReader::new(stdout).lines() {
        let event: EventEnvelope = serde_json::from_str(&line.unwrap()).unwrap();
        match event.event {
            Event::RunStarted { run_id: id, .. } if id == run_id => saw_started = true,
            Event::RunToolStarted {
                run_id: id, name, ..
            } if id == run_id && name == "Command" => saw_tool_started = true,
            Event::RunToolCompleted {
                run_id: id,
                output,
                is_error: false,
                ..
            } if id == run_id && output == "project" => saw_tool_completed = true,
            Event::RunMessageCompleted { run_id: id, text } if id == run_id && text == "done" => {
                saw_message = true;
            }
            Event::RunExited {
                run_id: id,
                status: RunStatus::Completed,
                ..
            } if id == run_id => break,
            _ => {}
        }
    }
    send(&mut stdin, Command::RunnerShutdown);
    drop(stdin);
    assert!(runner.wait().unwrap().success());
    assert!(saw_started && saw_tool_started && saw_tool_completed && saw_message);

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
    assert!(!args.iter().any(|arg| arg.contains("test prompt")));
}

fn send(stdin: &mut impl Write, command: Command) {
    serde_json::to_writer(&mut *stdin, &CommandEnvelope::new(command)).unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();
}
