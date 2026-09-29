//! Non-interactive CLI probes with caller-owned deadlines.

use std::{
    ffi::OsStr,
    io,
    process::{Output, Stdio},
};
use tokio::{
    process::Command,
    time::{Instant, timeout_at},
};

pub fn command(executable: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(executable);
    super::hide_console_window(command.as_std_mut());
    command.stdin(Stdio::null()).kill_on_drop(true);
    command
}

/// A shared deadline lets multi-step probes keep one total time budget.
/// Dropping a timed-out output future also terminates its child process.
pub async fn output(command: &mut Command, deadline: Instant) -> io::Result<Output> {
    timeout_at(deadline, command.output())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "CLI probe timed out"))?
}
