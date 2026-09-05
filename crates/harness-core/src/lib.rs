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
    let mut directories: Vec<PathBuf> = env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).collect())
        .unwrap_or_default();
    let home = if cfg!(windows) {
        env::var_os("USERPROFILE").or_else(|| env::var_os("HOME"))
    } else {
        env::var_os("HOME")
    };
    if let Some(home) = home {
        directories.push(PathBuf::from(home).join(".local/bin"));
    }
    if cfg!(windows) {
        if let Some(app_data) = env::var_os("APPDATA") {
            directories.push(PathBuf::from(app_data).join("npm"));
        }
    } else {
        if cfg!(target_os = "macos") {
            directories.push(PathBuf::from("/opt/homebrew/bin"));
        }
        directories.push(PathBuf::from("/usr/local/bin"));
    }
    let extensions =
        cfg!(windows).then(|| env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into()));
    resolve_in_paths(configured, directories, extensions.as_deref())
}

fn resolve_in_paths(
    configured: &str,
    directories: impl IntoIterator<Item = PathBuf>,
    extensions: Option<&str>,
) -> Option<PathBuf> {
    let configured_path = PathBuf::from(configured);
    let candidates = if configured_path.components().count() > 1 {
        vec![configured_path]
    } else {
        directories
            .into_iter()
            .map(|directory| directory.join(configured))
            .collect()
    };
    candidates
        .into_iter()
        .flat_map(|path| executable_candidates(path, extensions))
        .find(|path| is_executable_file(path))
}

fn executable_candidates(path: PathBuf, extensions: Option<&str>) -> Vec<PathBuf> {
    let mut candidates = vec![path.clone()];
    if path.extension().is_none()
        && let Some(extensions) = extensions
    {
        for extension in extensions.split(';').map(str::trim) {
            let extension = extension.to_ascii_lowercase();
            if [".exe", ".com", ".bat", ".cmd"].contains(&extension.as_str()) {
                let mut name = path.as_os_str().to_owned();
                name.push(extension);
                candidates.push(PathBuf::from(name));
            }
        }
    }
    candidates
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

#[cfg(test)]
mod tests {
    use super::*;

    fn executable(path: &Path) {
        std::fs::write(path, "fixture").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn finds_windows_executables_and_npm_shims_in_path_order() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        executable(&first.join("codex.cmd"));
        executable(&second.join("codex.exe"));
        assert_eq!(
            resolve_in_paths("codex", [first.clone(), second], Some(".EXE;.CMD")),
            Some(first.join("codex.cmd"))
        );
        assert_eq!(
            resolve_in_paths("codex.cmd", [first.clone()], Some(".EXE;.CMD")),
            Some(first.join("codex.cmd"))
        );
    }

    #[test]
    fn explicit_paths_support_extensions_without_searching_other_directories() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("claude.exe");
        executable(&path);
        let configured = directory.path().join("claude");
        assert_eq!(
            resolve_in_paths(&configured.to_string_lossy(), [], Some(".EXE;.CMD")),
            Some(path)
        );
        assert!(resolve_in_paths(&configured.to_string_lossy(), [], None).is_none());
        assert!(
            resolve_in_paths("missing", [directory.path().to_path_buf()], Some(".EXE")).is_none()
        );
    }

    #[test]
    fn directories_are_never_treated_as_executables() {
        let directory = tempfile::tempdir().unwrap();
        assert!(resolve_in_paths(&directory.path().to_string_lossy(), [], None).is_none());
    }
}
