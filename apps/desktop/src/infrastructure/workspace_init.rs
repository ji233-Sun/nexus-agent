use anyhow::{Context as _, Result, bail};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt as _, sync::watch};

pub(crate) fn run(
    cwd: &Path,
    script: &str,
    cancel: watch::Receiver<bool>,
    output: impl Fn(String) + Sync,
) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut command = if cfg!(windows) {
            let mut command = tokio::process::Command::new("cmd.exe");
            command.args(["/D", "/S", "/C", script]);
            command
        } else {
            let mut command = tokio::process::Command::new("/bin/sh");
            command.args(["-c", script]);
            command
        };
        command.current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        nexus_runner::configure_child_process(&mut command);
        let mut child = command.spawn().context("启动项目初始化命令")?;
        let pid = child.id().context("读取初始化进程 ID")?;
        let stdout = child.stdout.take().context("读取初始化输出")?;
        let stderr = child.stderr.take().context("读取初始化诊断")?;
        let mut cancel = cancel;
        let result = tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(900), async {
                tokio::join!(child.wait(), read_output(stdout, &output), read_output(stderr, &output))
            }) => result.ok(),
            _ = cancel.changed() => None,
        };
        let Some((status, stdout, stderr)) = result else {
            nexus_runner::terminate_child_process(&mut child, pid).await?;
            bail!("初始化已取消或超过 15 分钟，请检查日志后重试");
        };
        stdout?;
        stderr?;
        let status = status?;
        anyhow::ensure!(status.success(), "初始化命令执行失败：{status}");
        Ok(())
    })
}

async fn read_output(
    mut stream: impl tokio::io::AsyncRead + Unpin,
    output: &impl Fn(String),
) -> std::io::Result<()> {
    let mut bytes = [0; 8192];
    let mut pending = Vec::new();
    loop {
        let count = stream.read(&mut bytes).await?;
        if count == 0 {
            if !pending.is_empty() {
                output(String::from_utf8_lossy(&pending).into_owned());
            }
            return Ok(());
        }
        pending.extend_from_slice(&bytes[..count]);
        // Keep partial UTF-8 characters between pipe reads.
        let valid = match std::str::from_utf8(&pending) {
            Ok(_) => pending.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => pending.len(),
        };
        if valid > 0 {
            output(String::from_utf8_lossy(&pending[..valid]).into_owned());
            pending.drain(..valid);
        }
    }
}
