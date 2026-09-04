use std::{
    env,
    io::{BufRead as _, BufReader, Write as _},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow};
use nexus_protocol::Command as RunnerCommand;
use nexus_protocol::{CommandEnvelope, EventEnvelope};

pub struct RunnerClient {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    events: mpsc::Receiver<EventEnvelope>,
}

impl RunnerClient {
    pub fn spawn() -> Result<Self> {
        let mut command = runner_command()?;
        let executable = command.get_program().to_string_lossy().into_owned();
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("启动 Runner：{executable}"))?;
        let stdin = child.stdin.take().context("获取 Runner stdin")?;
        let stdout = child.stdout.take().context("获取 Runner stdout")?;
        let stderr = child.stderr.take().context("获取 Runner stderr")?;
        let (events_tx, events) = mpsc::sync_channel(512);
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(event) = serde_json::from_str::<EventEnvelope>(&line) {
                    let _ = events_tx.send(event);
                }
            }
        });
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                eprintln!("runner: {line}");
            }
        });

        Ok(Self {
            child,
            stdin: Arc::new(Mutex::new(stdin)),
            events,
        })
    }

    pub fn send(&self, command: CommandEnvelope) -> Result<()> {
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| anyhow!("Runner 写入锁已损坏"))?;
        serde_json::to_writer(&mut *stdin, &command).context("编码 Runner 命令")?;
        stdin.write_all(b"\n").context("写入 Runner 命令")?;
        stdin.flush().context("刷新 Runner 命令")?;
        Ok(())
    }

    pub fn drain_events(&self) -> Vec<EventEnvelope> {
        let mut events = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            events.push(event);
        }
        events
    }
}

impl Drop for RunnerClient {
    fn drop(&mut self) {
        let _ = self.send(CommandEnvelope::new(RunnerCommand::RunnerShutdown));
        for _ in 0..10 {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn runner_command() -> Result<Command> {
    if let Some(path) = env::var_os("NEXUS_RUNNER_PATH") {
        let path = std::path::PathBuf::from(path);
        if path.is_file() {
            return Ok(Command::new(path));
        }
        return Err(anyhow!(
            "NEXUS_RUNNER_PATH 指向的文件不存在：{}",
            path.display()
        ));
    }

    let current = env::current_exe().context("定位 Desktop 可执行文件")?;
    let mut command = Command::new(current);
    command.arg(crate::RUNNER_MODE_ARG);
    Ok(command)
}
