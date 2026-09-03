use std::{
    env,
    io::{BufRead as _, BufReader, Write as _},
    path::PathBuf,
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
        let executable = runner_executable()?;
        let mut child = Command::new(&executable)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("启动 Runner：{}", executable.display()))?;
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

fn runner_executable() -> Result<PathBuf> {
    if let Some(path) = env::var_os("NEXUS_RUNNER_PATH") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(anyhow!(
            "NEXUS_RUNNER_PATH 指向的文件不存在：{}",
            path.display()
        ));
    }

    let current = env::current_exe().context("定位 Desktop 可执行文件")?;
    let candidate = current
        .parent()
        .context("定位 Desktop 可执行文件目录")?
        .join(format!("nexus-runner{}", env::consts::EXE_SUFFIX));
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(anyhow!(
            "未找到 Runner：{}。请先运行 `cargo build --workspace`，或设置 NEXUS_RUNNER_PATH。",
            candidate.display()
        ))
    }
}
