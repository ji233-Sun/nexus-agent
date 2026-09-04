use std::{
    env,
    path::{Path, PathBuf},
};

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub stdin: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecodedEvent {
    TextDelta(String),
    MessageCompleted(String),
    ToolStarted {
        id: String,
        name: String,
        summary: String,
    },
    ToolCompleted {
        id: String,
        output: String,
        is_error: bool,
    },
    Status(String),
    Error(String),
}

pub trait LineDecoder: Send {
    fn decode_line(&mut self, line: &str) -> Result<Vec<DecodedEvent>, serde_json::Error>;
}

pub fn resolve_executable(configured: &str) -> Option<PathBuf> {
    let configured_path = PathBuf::from(configured);
    if configured_path.components().count() > 1 {
        return is_executable_file(&configured_path).then_some(configured_path);
    }

    if let Some(path) = env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|path| path.join(configured))
            .find(|path| is_executable_file(path))
    }) {
        return Some(path);
    }

    let mut candidates = vec![
        PathBuf::from("/opt/homebrew/bin").join(configured),
        PathBuf::from("/usr/local/bin").join(configured),
    ];
    if let Some(home) = env::var_os("HOME") {
        candidates.insert(0, PathBuf::from(home).join(".local/bin").join(configured));
    }
    candidates.into_iter().find(|path| is_executable_file(path))
}

pub fn summarize_json(value: &Value) -> String {
    let raw = value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string());
    summarize_text(&raw)
}

pub fn summarize_text(text: &str) -> String {
    const MAX_CHARS: usize = 400;
    if text.chars().count() <= MAX_CHARS {
        text.to_owned()
    } else {
        let mut summary: String = text.chars().take(MAX_CHARS).collect();
        summary.push('…');
        summary
    }
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}
