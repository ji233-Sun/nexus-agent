use std::{path::Path, process::Command as SystemCommand};

pub(crate) fn is_git_dirty(path: &Path) -> bool {
    SystemCommand::new("git")
        .args(["status", "--porcelain"])
        .current_dir(path)
        .output()
        .map(|output| output.status.success() && !output.stdout.is_empty())
        .unwrap_or(false)
}
