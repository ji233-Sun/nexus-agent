use std::{
    env,
    path::{Path, PathBuf},
};

use nexus_domain::{UserAskAnswer, UserAskQuestion};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCatalogError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for ModelCatalogError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("模型目录探测已取消"),
            Self::Failed(message) => formatter.write_str(message),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub stdin: String,
}

// Interactive frames may contain an API key during in-memory authentication.
#[derive(Clone, PartialEq)]
pub struct InputFrame(pub Value);

impl std::fmt::Debug for InputFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InputFrame([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserAskRequest {
    pub native_request_id: String,
    pub questions: Vec<UserAskQuestion>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalOption {
    pub label: String,
    pub response: InputFrame,
}

// Native response frames stay in the runner; the UI can only select offered options.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalPrompt {
    pub id: String,
    pub title: String,
    pub details: String,
    pub options: Vec<ApprovalOption>,
    pub cancel: InputFrame,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecodedEvent {
    WriteStdin(InputFrame),
    UserAskRequested(UserAskRequest),
    UserAskFinished {
        native_request_id: String,
        status: nexus_domain::UserAskStatus,
        message: Option<String>,
    },
    ApprovalRequested(ApprovalPrompt),
    ApprovalResolved(String),
    SessionStarted(String),
    InputAccepted(String),
    InputRejected {
        id: String,
        message: String,
    },
    TurnCompleted,
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
    fn steer(&mut self, message_id: &str, prompt: &str) -> Option<InputFrame>;
    fn answer_user_ask(
        &mut self,
        _native_request_id: &str,
        _answers: &[UserAskAnswer],
    ) -> Option<InputFrame> {
        None
    }
}

pub fn resolve_executable(configured: &str) -> Option<PathBuf> {
    let extensions =
        cfg!(windows).then(|| env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into()));
    resolve_in_paths(configured, executable_search_paths(), extensions.as_deref())
}

// GUI launches may not inherit the shell's PATH. Use the same search directories
// for discovery and child processes so Node/Bun shebangs can also find their runtime.
pub fn executable_search_paths() -> Vec<PathBuf> {
    search_paths(env::consts::OS, |key| env::var_os(key))
}

fn search_paths(
    os: &str,
    mut variable: impl FnMut(&str) -> Option<std::ffi::OsString>,
) -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = variable("PATH")
        .map(|paths| env::split_paths(&paths).collect())
        .unwrap_or_default();
    let mut path = |key| {
        variable(key)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let home = if os == "windows" {
        path("USERPROFILE").or_else(|| path("HOME"))
    } else {
        path("HOME")
    };
    for (key, suffix) in [
        ("VP_HOME", "bin"),
        ("BUN_INSTALL", "bin"),
        ("PNPM_HOME", ""),
        (
            "NPM_CONFIG_PREFIX",
            if os == "windows" { "" } else { "bin" },
        ),
        ("VOLTA_HOME", "bin"),
        ("NVM_BIN", ""),
        ("FNM_MULTISHELL_PATH", "bin"),
        ("MISE_DATA_DIR", "shims"),
        ("ASDF_DATA_DIR", "shims"),
        ("PI_INSTALL_DIR", ""),
        ("CODEX_INSTALL_DIR", ""),
    ] {
        if let Some(directory) = path(key) {
            directories.push(directory.join(suffix));
        }
    }
    if let Some(home) = home {
        for suffix in [
            ".local/bin",
            ".vite-plus/bin",
            ".bun/bin",
            ".local/share/pnpm",
            "Library/pnpm",
            ".npm-global/bin",
            ".yarn/bin",
            ".config/yarn/global/node_modules/.bin",
            ".volta/bin",
            ".local/share/mise/shims",
            ".asdf/shims",
            ".nvm/current/bin",
            ".fnm/aliases/default/bin",
            ".local/share/fnm/aliases/default/bin",
            "Library/Application Support/fnm/aliases/default/bin",
            ".nix-profile/bin",
        ] {
            directories.push(home.join(suffix));
        }
    }
    if os == "windows" {
        if let Some(app_data) = path("APPDATA") {
            directories.push(app_data.join("npm"));
        }
        if let Some(local) = path("LOCALAPPDATA") {
            directories.push(local.join("pnpm"));
            directories.push(local.join("omp"));
            directories.push(local.join("Microsoft/WinGet/Links"));
            directories.push(local.join("Microsoft/WindowsApps"));
        }
        if let Some(scoop) =
            path("SCOOP").or_else(|| path("USERPROFILE").map(|home| home.join("scoop")))
        {
            directories.push(scoop.join("shims"));
        }
    } else {
        if os == "macos" {
            directories.push(PathBuf::from("/opt/homebrew/bin"));
        }
        directories.extend(
            [
                "/home/linuxbrew/.linuxbrew/bin",
                "/usr/local/bin",
                "/usr/bin",
                "/bin",
            ]
            .map(PathBuf::from),
        );
    }
    let mut seen = std::collections::HashSet::new();
    directories
        .retain(|directory| !directory.as_os_str().is_empty() && seen.insert(directory.clone()));
    directories
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
    let mut candidates = Vec::new();
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
    // npm writes an extensionless Unix script beside its Windows .cmd shim.
    candidates.push(path);
    candidates
}

// Keep tool payloads intact; previews are a presentation concern.
pub fn tool_content(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
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
        executable(&first.join("codex"));
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
    fn gui_search_finds_global_managers_and_honors_custom_homes() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path();
        let vp = home.join("custom-vp");
        for os in ["macos", "linux", "windows"] {
            let paths = search_paths(os, |key| match key {
                "HOME" | "USERPROFILE" => Some(home.as_os_str().into()),
                "VP_HOME" => Some(vp.as_os_str().into()),
                "PNPM_HOME" => Some(home.join("custom-pnpm").into_os_string()),
                "APPDATA" => Some(home.join("AppData/Roaming").into_os_string()),
                "LOCALAPPDATA" => Some(home.join("AppData/Local").into_os_string()),
                _ => None,
            });
            for suffix in [
                ".local/bin",
                ".vite-plus/bin",
                ".bun/bin",
                ".volta/bin",
                ".asdf/shims",
                ".nix-profile/bin",
            ] {
                assert!(paths.contains(&home.join(suffix)), "{os}: {suffix}");
            }
            assert!(paths.contains(&vp.join("bin")));
            assert!(paths.contains(&home.join("custom-pnpm")));
            if os == "windows" {
                assert!(paths.contains(&home.join("AppData/Roaming/npm")));
                assert!(paths.contains(&home.join("AppData/Local/Microsoft/WinGet/Links")));
                assert!(paths.contains(&home.join("scoop/shims")));
            }
        }
        std::fs::create_dir_all(vp.join("bin")).unwrap();
        executable(&vp.join("bin/codex"));
        let paths = search_paths("linux", |key| {
            (key == "VP_HOME").then(|| vp.as_os_str().into())
        });
        assert_eq!(
            resolve_in_paths("codex", paths, None),
            Some(vp.join("bin/codex"))
        );
    }

    #[test]
    fn inherited_path_precedes_fallbacks_without_duplicates_or_empty_entries() {
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().join(".bun/bin");
        let paths = search_paths("linux", |key| match key {
            "HOME" => Some(directory.path().as_os_str().into()),
            "PATH" => Some(env::join_paths([&bin, &bin]).unwrap()),
            "PNPM_HOME" => Some("".into()),
            _ => None,
        });
        assert_eq!(paths.first(), Some(&bin));
        assert_eq!(paths.iter().filter(|path| **path == bin).count(), 1);
        assert!(paths.iter().all(|path| !path.as_os_str().is_empty()));
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
