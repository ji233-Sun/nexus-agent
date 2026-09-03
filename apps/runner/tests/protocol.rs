#![cfg(unix)]

use std::{
    fs,
    io::{BufRead as _, BufReader, Write},
    os::unix::fs::PermissionsExt as _,
    process::{Command as ProcessCommand, Stdio},
};

use nexus_domain::{ClaudeModel, RunStatus, ThinkingEffort};
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
            executable: fake_claude.to_string_lossy().into_owned(),
            model: ClaudeModel::Opus,
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

fn send(stdin: &mut impl Write, command: Command) {
    serde_json::to_writer(&mut *stdin, &CommandEnvelope::new(command)).unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();
}
