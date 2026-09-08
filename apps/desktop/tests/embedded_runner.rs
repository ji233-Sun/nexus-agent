use std::{
    io::Write as _,
    process::{Command as ProcessCommand, Stdio},
};

use nexus_protocol::{Command, CommandEnvelope, Event, EventEnvelope, PROTOCOL_VERSION};

#[test]
fn desktop_reports_the_compiled_build_tag_without_starting_the_app() {
    let output = ProcessCommand::new(env!("CARGO_BIN_EXE_nexus-desktop"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        env!("NEXUS_RELEASE_TAG")
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn update_helper_waits_for_the_running_binary_then_installs_and_restarts() {
    use std::{
        fs,
        io::{BufRead as _, BufReader},
        time::{Duration, Instant},
    };

    let directory = tempfile::tempdir().unwrap();
    let parent = directory
        .path()
        .canonicalize()
        .unwrap()
        .join("安装 location");
    fs::create_dir(&parent).unwrap();
    let marker = parent.join("restarted-version.txt");
    let suffix = std::env::consts::EXE_SUFFIX;
    let executable = parent.join(format!("nexus-desktop{suffix}"));
    let runner = parent.join(format!("nexus-runner{suffix}"));
    let staging = parent.join(format!(".nexus-update-{}", uuid::Uuid::new_v4()));
    let root = "nexus-agent-test";
    let package = staging.join("unpacked").join(root);
    fs::create_dir_all(&package).unwrap();
    for (version, destination) in [("old", &parent), ("new", &package)] {
        let source = parent.join(format!("{version}.rs"));
        fs::write(&source, format!(
            "fn main() {{ if std::env::args().nth(1).as_deref() == Some(\"--parent\") {{ let _ = std::io::read_to_string(std::io::stdin()); }} else {{ std::fs::write({marker:?}, {version:?}).unwrap(); }} }}"
        )).unwrap();
        let output = ProcessCommand::new("rustc")
            .arg(&source)
            .arg("-o")
            .arg(destination.join(format!("nexus-desktop{suffix}")))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        fs::copy(
            destination.join(format!("nexus-desktop{suffix}")),
            destination.join(format!("nexus-runner{suffix}")),
        )
        .unwrap();
    }
    let original = fs::read(&executable).unwrap();
    let mut running = ProcessCommand::new(&executable)
        .arg("--parent")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut helper = ProcessCommand::new(env!("CARGO_BIN_EXE_nexus-desktop"))
        .arg("--nexus-apply-update")
        .arg(running.id().to_string())
        .arg(&staging)
        .arg(&executable)
        .arg(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(helper.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready.trim(), "ready");
    assert_eq!(fs::read(&executable).unwrap(), original);
    drop(running.stdin.take());
    assert!(running.wait().unwrap().success());
    let deadline = Instant::now() + Duration::from_secs(20);
    while helper.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = helper.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = helper.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fs::read_to_string(marker).unwrap(), "new");
    assert_eq!(fs::read(&executable).unwrap(), fs::read(runner).unwrap());
    assert!(!staging.exists());
}

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
