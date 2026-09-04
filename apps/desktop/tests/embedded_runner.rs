#![cfg(unix)]

use std::{
    io::Write as _,
    process::{Command as ProcessCommand, Stdio},
};

use nexus_protocol::{Command, CommandEnvelope, Event, EventEnvelope, PROTOCOL_VERSION};

#[test]
fn desktop_serves_the_current_runner_protocol() {
    let mut child = ProcessCommand::new(env!("CARGO_BIN_EXE_nexus-desktop"))
        .arg("--nexus-runner")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    serde_json::to_writer(&mut stdin, &CommandEnvelope::new(Command::RunnerHello)).unwrap();
    stdin.write_all(b"\n").unwrap();
    serde_json::to_writer(&mut stdin, &CommandEnvelope::new(Command::RunnerShutdown)).unwrap();
    stdin.write_all(b"\n").unwrap();
    drop(stdin);

    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let line = String::from_utf8(output.stdout).unwrap();
    let event: EventEnvelope = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(event.protocol_version, PROTOCOL_VERSION);
    assert!(matches!(event.event, Event::RunnerReady));
}
