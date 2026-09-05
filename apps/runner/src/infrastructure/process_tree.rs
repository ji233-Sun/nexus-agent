use std::{io, process::ExitStatus};

use tokio::process::{Child, Command};

#[cfg(unix)]
use {
    nix::{
        sys::signal::{Signal, killpg},
        unistd::Pid,
    },
    std::time::Duration,
    tokio::time::timeout,
};

pub(super) fn configure(command: &mut Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW：桌面应用不弹出控制台。
}

pub(super) async fn cancel(child: &mut Child, pid: u32) -> io::Result<ExitStatus> {
    #[cfg(unix)]
    {
        if pid > 0 {
            let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGINT);
        }
        if let Ok(status) = timeout(Duration::from_secs(3), child.wait()).await {
            return status;
        }
    }
    terminate(child, pid).await;
    child.wait().await
}

pub(super) async fn terminate(child: &mut Child, pid: u32) {
    #[cfg(unix)]
    if pid > 0 {
        let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
    }
    #[cfg(windows)]
    if pid > 0 {
        // GUI 启动的进程不保证拥有控制台，使用系统工具结束对应进程树。
        let taskkill = std::env::var_os("SystemRoot")
            .map(std::path::PathBuf::from)
            .map(|root| root.join("System32/taskkill.exe"))
            .unwrap_or_else(|| "taskkill.exe".into());
        let _ = Command::new(taskkill)
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }
    let _ = child.start_kill();
}
