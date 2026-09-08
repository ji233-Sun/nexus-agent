use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::mpsc::{self, Receiver},
    time::Duration,
};

use anyhow::{Context as _, Result, bail, ensure};
use nexus_domain::HarnessKind;
use nexus_harness_core::{executable_search_paths, resolve_executable, resolve_in_paths};
use tokio::{io::AsyncReadExt as _, process::Command, sync::watch};

use crate::{
    i18n::LocalizedText,
    model::harness_installation::{
        HarnessInstallation, InstallMethod, InstallOption, MaintenanceCommand, MaintenanceRequest,
    },
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const MAX_OUTPUT: usize = 64 * 1024;

pub(crate) enum Event {
    Scanned(HarnessKind, HarnessInstallation),
    Finished {
        request: Option<MaintenanceRequest>,
        result: Result<(), LocalizedText>,
    },
}

pub(crate) struct Worker {
    pub(crate) events: Receiver<Event>,
    cancel: watch::Sender<bool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    pub(crate) fn cancel(&self) {
        let _ = self.cancel.send(true);
    }

    #[cfg(test)]
    pub(crate) fn test_channel() -> (mpsc::Sender<Event>, Self) {
        let (send, events) = mpsc::channel();
        let (cancel, _) = watch::channel(false);
        (
            send,
            Self {
                events,
                cancel,
                thread: None,
            },
        )
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(crate) fn documentation(harness: HarnessKind) -> &'static str {
    match harness {
        HarnessKind::Claude => "https://code.claude.com/docs/en/setup",
        HarnessKind::Codex => "https://developers.openai.com/codex/cli/",
        HarnessKind::Omp => "https://github.com/can1357/oh-my-pi#install",
    }
}

fn package(harness: HarnessKind) -> &'static str {
    match harness {
        HarnessKind::Claude => "@anthropic-ai/claude-code",
        HarnessKind::Codex => "@openai/codex",
        HarnessKind::Omp => "@oh-my-pi/pi-coding-agent",
    }
}

impl MaintenanceCommand {
    fn new(program: impl Into<PathBuf>, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            environment: BTreeMap::new(),
        }
    }

    fn with_env(mut self, key: &str, value: impl Into<String>) -> Self {
        self.environment.insert(key.into(), value.into());
        self
    }

    pub(crate) fn display(&self) -> String {
        let windows = cfg!(windows);
        let mut words = self
            .environment
            .iter()
            .map(|(key, value)| {
                if windows {
                    format!("$env:{key} = {};", quote(value, true))
                } else {
                    format!("{key}={}", quote(value, false))
                }
            })
            .collect::<Vec<_>>();
        if windows {
            words.push("&".into());
        }
        words.push(quote(&self.program.to_string_lossy(), windows));
        words.extend(self.args.iter().map(|arg| quote(arg, windows)));
        words.join(" ")
    }
}

fn quote(value: &str, windows: bool) -> String {
    if !windows
        && !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_./:@=-".contains(ch))
    {
        return value.into();
    }
    let escaped = if windows {
        value
            .chars()
            .map(|ch| {
                if matches!(ch, '\'' | '\u{2018}' | '\u{2019}') {
                    format!("{ch}{ch}")
                } else {
                    ch.to_string()
                }
            })
            .collect::<String>()
    } else {
        value.replace('\'', "'\\''")
    };
    format!("'{escaped}'")
}

struct Environment {
    os: &'static str,
    home: PathBuf,
    variables: BTreeMap<String, String>,
    tools: BTreeMap<String, PathBuf>,
    paths: Vec<PathBuf>,
    cancel: watch::Receiver<bool>,
}

impl Environment {
    fn current(cancel: watch::Receiver<bool>) -> Result<Self> {
        let home = env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .context("无法确定用户目录")?;
        let variables = [
            "VP_HOME",
            "BUN_INSTALL",
            "BUN_INSTALL_GLOBAL_DIR",
            "BUN_INSTALL_BIN",
            "PNPM_HOME",
            "VOLTA_HOME",
            "MISE_DATA_DIR",
            "ASDF_DATA_DIR",
            "SCOOP",
            "CODEX_HOME",
            "CODEX_INSTALL_DIR",
            "PI_INSTALL_DIR",
            "LOCALAPPDATA",
            "APPDATA",
            "PATHEXT",
        ]
        .into_iter()
        .filter_map(|key| {
            env::var(key)
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| (key.into(), value))
        })
        .collect();
        let mut tools: BTreeMap<String, PathBuf> = [
            "vp",
            "bun",
            "pnpm",
            "yarn",
            "npm",
            "brew",
            "winget",
            "scoop",
            "volta",
            "bash",
            "powershell",
            "dpkg-query",
            "rpm",
            "pacman",
            "apk",
        ]
        .into_iter()
        .filter_map(|name| {
            let resolved = resolve_executable(name).or_else(|| {
                (name == "scoop")
                    .then(|| resolve_executable("scoop.ps1"))
                    .flatten()
            });
            resolved.map(|path| (name.into(), path))
        })
        .collect();
        // Vite+ installs npm/pnpm/yarn proxy commands beside vp. Their global
        // operations belong to Vite+, not to three separate global stores.
        if let Some(vp) = tools.get("vp").cloned() {
            for name in ["npm", "pnpm", "yarn"] {
                if tools
                    .get(name)
                    .is_some_and(|program| same_path(program, &vp))
                {
                    let replacement = executable_search_paths()
                        .into_iter()
                        .filter_map(|directory| {
                            resolve_executable(&directory.join(name).to_string_lossy())
                        })
                        .find(|program| !same_path(program, &vp));
                    tools.remove(name);
                    if let Some(program) = replacement {
                        tools.insert(name.into(), program);
                    }
                }
            }
        }
        Ok(Self {
            os: env::consts::OS,
            home,
            variables,
            tools,
            paths: executable_search_paths(),
            cancel,
        })
    }

    fn home_for(&self, key: &str, fallback: &str) -> PathBuf {
        self.variables
            .get(key)
            .map(PathBuf::from)
            .unwrap_or_else(|| self.home.join(fallback))
    }

    fn resolve(&self, configured: &str) -> Option<PathBuf> {
        let extensions = (self.os == "windows").then(|| {
            self.variables
                .get("PATHEXT")
                .map(String::as_str)
                .unwrap_or(".COM;.EXE;.BAT;.CMD")
        });
        resolve_in_paths(configured, self.paths.iter().cloned(), extensions)
    }

    fn command(
        &self,
        name: &str,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Option<MaintenanceCommand> {
        Some(MaintenanceCommand::new(self.tools.get(name)?.clone(), args))
    }

    async fn output(&self, command: &MaintenanceCommand) -> Option<String> {
        run_command(command, self, PROBE_TIMEOUT)
            .await
            .ok()
            .map(|text| text.trim().into())
    }

    async fn query_path(&self, tool: &str, args: &[&str]) -> Option<PathBuf> {
        let value = self
            .output(&self.command(tool, args.iter().copied())?)
            .await?;
        let path = PathBuf::from(value);
        path.is_absolute().then_some(path)
    }
}

// These are local, read-only manager queries. No registry request is needed to scan.
struct Manager {
    method: InstallMethod,
    program: PathBuf,
    bin: PathBuf,
    global: Option<PathBuf>,
}

async fn managers(environment: &Environment) -> Vec<Manager> {
    let (npm, pnpm_root, pnpm_bin, yarn_root, yarn_bin, bun_bin) = tokio::join!(
        environment.query_path("npm", &["prefix", "-g"]),
        environment.query_path("pnpm", &["root", "-g"]),
        environment.query_path("pnpm", &["bin", "-g"]),
        environment.query_path("yarn", &["global", "dir"]),
        environment.query_path("yarn", &["global", "bin"]),
        environment.query_path("bun", &["pm", "bin", "-g"]),
    );
    let vp = environment.home_for("VP_HOME", ".vite-plus");
    let bun = environment.home_for("BUN_INSTALL", ".bun");
    let entries = [
        (
            InstallMethod::VitePlus,
            "vp",
            Some(vp.join("bin")),
            Some(vp.join("packages")),
        ),
        (
            InstallMethod::Bun,
            "bun",
            bun_bin.or_else(|| Some(bun.join("bin"))),
            Some(
                environment
                    .variables
                    .get("BUN_INSTALL_GLOBAL_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| bun.join("install/global")),
            ),
        ),
        (
            InstallMethod::Pnpm,
            "pnpm",
            pnpm_bin.or_else(|| environment.variables.get("PNPM_HOME").map(PathBuf::from)),
            pnpm_root.and_then(|path| path.parent().map(Path::to_path_buf)),
        ),
        (InstallMethod::Yarn, "yarn", yarn_bin, yarn_root),
        (
            InstallMethod::Npm,
            "npm",
            npm.as_ref().map(|prefix| {
                if environment.os == "windows" {
                    prefix.clone()
                } else {
                    prefix.join("bin")
                }
            }),
            npm,
        ),
    ];
    entries
        .into_iter()
        .filter_map(|(method, name, bin, global)| {
            Some(Manager {
                method,
                program: environment.tools.get(name)?.clone(),
                bin: bin?,
                global,
            })
        })
        .collect()
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = fs::canonicalize(left).unwrap_or_else(|_| left.into());
    let right = fs::canonicalize(right).unwrap_or_else(|_| right.into());
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn package_global(real: &Path, name: &str) -> Option<PathBuf> {
    let path = slash(real);
    let (prefix, package) = path.rsplit_once("/node_modules/")?;
    let platform_package = package
        .strip_prefix(&format!("{name}-"))
        .is_some_and(|suffix| {
            ["darwin-", "linux-", "win32-", "windows-"]
                .iter()
                .any(|platform| suffix.starts_with(platform))
        });
    (package.starts_with(&format!("{name}/")) || platform_package).then(|| PathBuf::from(prefix))
}

fn inside(path: &Path, root: &Path) -> bool {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.into());
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.into());
    if cfg!(windows) {
        let root = slash(&root).to_ascii_lowercase();
        let path = slash(&path).to_ascii_lowercase();
        path == root || path.starts_with(&format!("{root}/"))
    } else {
        path.starts_with(root)
    }
}

fn manager_owns(manager: &Manager, name: &str, executable: &Path, real: &Path) -> bool {
    let in_bin = executable
        .parent()
        .is_some_and(|parent| same_path(parent, &manager.bin));
    if manager.method == InstallMethod::VitePlus && in_bin && same_path(real, &manager.program) {
        return true;
    }
    let Some(root) = &manager.global else {
        return false;
    };
    if package_global(real, name).is_some() && inside(real, root) {
        return true;
    }
    // Sharing /usr/local/bin with Yarn or Bun is not proof of ownership.
    // Windows launchers instead need both the package and a matching shim.
    if in_bin
        && root
            .join("node_modules")
            .join(name)
            .join("package.json")
            .is_file()
    {
        let script = ["cmd", "bat", "ps1"].iter().any(|extension| {
            executable
                .extension()
                .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        });
        return (script
            && fs::read_to_string(executable)
                .is_ok_and(|text| text.replace('\\', "/").contains(name)))
            || (manager.method == InstallMethod::Bun
                && executable.with_extension("bunx").is_file());
    }
    false
}

// Recognize the store even if its manager is no longer on PATH. Only vp and
// Bun have explicit environment overrides for relocating an update safely;
// pnpm/Yarn updates additionally require their current global-store queries.
fn known_node_store(
    environment: &Environment,
    executable: &Path,
    real: &Path,
    name: &str,
) -> Option<(InstallMethod, PathBuf, PathBuf)> {
    let vp = environment.home_for("VP_HOME", ".vite-plus");
    if (inside(executable, &vp.join("bin")) && same_path(real, &vp.join("current/bin/vp")))
        || (inside(real, &vp.join("packages")) && package_global(real, name).is_some())
    {
        return Some((InstallMethod::VitePlus, vp.join("bin"), vp.join("packages")));
    }
    // A custom VP_HOME can also be recovered from its proxy's actual path.
    if let Some((root, _)) = slash(real).split_once("/current/bin/vp") {
        let root = PathBuf::from(root);
        if inside(executable, &root.join("bin")) {
            return Some((
                InstallMethod::VitePlus,
                root.join("bin"),
                root.join("packages"),
            ));
        }
    }
    let global = package_global(real, name)?;
    let global_path = slash(&global);
    if let Some((root, _)) = global_path.split_once("/install/global") {
        return Some((InstallMethod::Bun, PathBuf::from(root).join("bin"), global));
    }
    if global_path.contains("/pnpm/global/") || global_path.contains("/Library/pnpm/") {
        let (root, _) = global_path.split_once("/global/")?;
        let root = PathBuf::from(root);
        let bin = if root.join("bin").is_dir() {
            root.join("bin")
        } else {
            root
        };
        return Some((InstallMethod::Pnpm, bin, global));
    }
    if global_path.contains("/yarn/global") {
        return Some((
            InstallMethod::Yarn,
            environment.home.join(".yarn/bin"),
            global,
        ));
    }
    None
}

fn npm_prefix(real: &Path, name: &str) -> Option<PathBuf> {
    let global = package_global(real, name)?;
    if global.file_name()? != "lib" {
        return None;
    }
    let prefix = global.parent()?;
    let path = slash(prefix);
    if path.contains("/node_modules/")
        || (path.contains("/mise/installs/") && !path.contains("/mise/installs/node/"))
    {
        return None;
    }
    Some(prefix.into())
}

fn manager_command(
    harness: HarnessKind,
    manager: &Manager,
    global: Option<&Path>,
    bin: &Path,
) -> MaintenanceCommand {
    let name = package(harness);
    let latest = format!("{name}@latest");
    let path = |value: &Path| value.to_string_lossy().into_owned();
    match manager.method {
        InstallMethod::VitePlus => {
            MaintenanceCommand::new(&manager.program, ["install".into(), "-g".into(), latest])
                .with_env("VP_HOME", path(bin.parent().unwrap_or(bin)))
        }
        InstallMethod::Bun => {
            let mut command = MaintenanceCommand::new(
                &manager.program,
                ["add".into(), "-g".into(), "--trust".into(), latest],
            );
            if let Some(global) = global {
                command = command.with_env("BUN_INSTALL_GLOBAL_DIR", path(global));
            }
            command.with_env("BUN_INSTALL_BIN", path(bin))
        }
        InstallMethod::Pnpm => {
            // pnpm's queries prove this is its active global installation. Let
            // pnpm preserve the version-specific (v10/v11+) global directory layout.
            MaintenanceCommand::new(&manager.program, ["add".into(), "-g".into(), latest])
        }
        InstallMethod::Yarn => {
            let mut args = vec![
                "global".into(),
                "add".into(),
                latest,
                "--prefix".into(),
                path(bin.parent().unwrap_or(bin)),
            ];
            if let Some(global) = global {
                args.extend(["--global-folder".into(), path(global)]);
            }
            MaintenanceCommand::new(&manager.program, args)
        }
        InstallMethod::Npm => {
            let mut args = vec![
                "install".into(),
                "-g".into(),
                format!("--allow-scripts={name}"),
                latest,
            ];
            if let Some(prefix) = global {
                args.extend(["--prefix".into(), path(prefix)]);
            }
            MaintenanceCommand::new(&manager.program, args)
        }
        _ => unreachable!("JavaScript package manager"),
    }
}

fn native_install(harness: HarnessKind, environment: &Environment) -> Option<MaintenanceCommand> {
    let (unix, windows) = match harness {
        HarnessKind::Claude => (
            "curl -fsSL https://claude.ai/install.sh | bash",
            "& ([scriptblock]::Create((Invoke-RestMethod https://claude.ai/install.ps1)))",
        ),
        HarnessKind::Codex => (
            "curl -fsSL https://chatgpt.com/codex/install.sh | sh",
            "& ([scriptblock]::Create((Invoke-RestMethod https://chatgpt.com/codex/install.ps1)))",
        ),
        HarnessKind::Omp => (
            "curl -fsSL https://omp.sh/install | sh -s -- --binary",
            "& ([scriptblock]::Create((Invoke-RestMethod https://omp.sh/install.ps1))) -Binary",
        ),
    };
    let command = if environment.os == "windows" {
        environment.command(
            "powershell",
            [
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("$ErrorActionPreference = 'Stop'; {windows}"),
            ],
        )?
    } else {
        environment.command("bash", ["-o", "pipefail", "-c", unix])?
    };
    Some(if harness == HarnessKind::Codex {
        command.with_env("CODEX_NON_INTERACTIVE", "1")
    } else {
        command
    })
}

fn install_options(
    harness: HarnessKind,
    environment: &Environment,
    managers: &[Manager],
) -> Vec<InstallOption> {
    let mut options = native_install(harness, environment)
        .map(|command| InstallOption {
            method: InstallMethod::Native,
            command,
        })
        .into_iter()
        .collect::<Vec<_>>();
    options.extend(managers.iter().map(|manager| InstallOption {
        method: manager.method,
        command: manager_command(harness, manager, manager.global.as_deref(), &manager.bin),
    }));
    if let Some(command) = environment.command(
        "brew",
        match harness {
            HarnessKind::Claude => vec!["install", "--cask", "claude-code"],
            HarnessKind::Codex => vec!["install", "--cask", "codex"],
            HarnessKind::Omp => vec!["install", "can1357/tap/omp"],
        },
    ) && (environment.os == "macos" || harness == HarnessKind::Omp)
    {
        options.push(InstallOption {
            method: InstallMethod::Homebrew,
            command,
        });
    }
    if let Some(id) = winget_id(harness)
        && let Some(command) = environment.command(
            "winget",
            [
                "install",
                "--id",
                id,
                "--exact",
                "--source",
                "winget",
                "--accept-package-agreements",
                "--accept-source-agreements",
                "--disable-interactivity",
            ],
        )
    {
        options.push(InstallOption {
            method: InstallMethod::WinGet,
            command,
        });
    }
    options
}

fn winget_id(harness: HarnessKind) -> Option<&'static str> {
    match harness {
        HarnessKind::Claude => Some("Anthropic.ClaudeCode"),
        HarnessKind::Codex => Some("OpenAI.Codex"),
        HarnessKind::Omp => None,
    }
}

fn homebrew_owner(real: &Path, harness: HarnessKind) -> Option<(PathBuf, String, bool)> {
    let path = slash(real);
    let (prefix, rest, cask) = path
        .split_once("/Caskroom/")
        .map(|(p, r)| (p, r, true))
        .or_else(|| path.split_once("/Cellar/").map(|(p, r)| (p, r, false)))?;
    let mut parts = rest.split('/');
    let name = parts.next()?;
    parts.next()?;
    parts.next()?;
    let expected = match harness {
        HarnessKind::Claude => "claude-code",
        HarnessKind::Codex => "codex",
        HarnessKind::Omp => "omp",
    };
    (name == expected
        || name == format!("{expected}@latest")
        || name == format!("{expected}@stable"))
    .then(|| (prefix.into(), name.into(), cask))
}

async fn ownership(
    harness: HarnessKind,
    executable: &Path,
    real: &Path,
    environment: &Environment,
    managers: &[Manager],
) -> (LocalizedText, Option<MaintenanceCommand>) {
    let entry = slash(executable);
    let target = slash(real);
    let name = package(harness);
    // Immutable, source, and application-bundled installs must never be overwritten.
    for (root, label) in [
        (
            environment
                .home_for("MISE_DATA_DIR", ".local/share/mise")
                .join("shims"),
            "mise（请使用 mise 管理版本）",
        ),
        (
            environment.home_for("ASDF_DATA_DIR", ".asdf").join("shims"),
            "asdf（请使用 asdf 管理版本）",
        ),
    ] {
        if inside(executable, &root) {
            return (label.into(), None);
        }
    }
    for (pattern, label) in [
        ("/nix/store/", "Nix（请更新声明式配置或 Profile）"),
        ("/mise/installs/", "mise（请使用 mise 管理版本）"),
        ("/mise/shims/", "mise（请使用 mise 管理版本）"),
        ("/.asdf/", "asdf（请使用 asdf 管理版本）"),
        (".app/Contents/", "应用内置（请更新所属应用）"),
    ] {
        if (entry.contains(pattern) || target.contains(pattern))
            && !(pattern == "/mise/installs/" && target.contains("/mise/installs/node/"))
        {
            return (label.into(), None);
        }
    }
    for manager in managers
        .iter()
        .filter(|manager| manager.method != InstallMethod::Npm)
    {
        if manager_owns(manager, name, executable, real) {
            let global = if manager.method == InstallMethod::Bun {
                package_global(real, name).or_else(|| manager.global.clone())
            } else {
                manager.global.clone()
            };
            return (
                manager.method.label().into(),
                Some(manager_command(
                    harness,
                    manager,
                    global.as_deref(),
                    &manager.bin,
                )),
            );
        }
    }
    if let Some((method, bin, global)) = known_node_store(environment, executable, real, name) {
        let command = match method {
            InstallMethod::VitePlus | InstallMethod::Bun => {
                let tool = if method == InstallMethod::VitePlus {
                    "vp"
                } else {
                    "bun"
                };
                resolve_executable(&bin.join(tool).to_string_lossy())
                    .or_else(|| environment.tools.get(tool).cloned())
                    .map(|program| {
                        manager_command(
                            harness,
                            &Manager {
                                method,
                                program,
                                bin: bin.clone(),
                                global: Some(global.clone()),
                            },
                            Some(&global),
                            &bin,
                        )
                    })
            }
            _ => None,
        };
        return (method.label().into(), command);
    }
    if let Some(prefix) = npm_prefix(real, name).or_else(|| {
        let parent = executable.parent()?;
        (environment.os == "windows"
            && managers.iter().any(|manager| {
                manager.method == InstallMethod::Npm
                    && manager_owns(manager, name, executable, real)
            }))
        .then(|| parent.to_path_buf())
    }) {
        // Use npm from the same Node installation where possible; --prefix pins
        // the actual destination even if another Node manager is first on PATH.
        let bin = if environment.os == "windows" {
            prefix.clone()
        } else {
            prefix.join("bin")
        };
        let program = resolve_executable(&bin.join("npm").to_string_lossy())
            .or_else(|| environment.tools.get("npm").cloned());
        let command = program.map(|program| {
            manager_command(
                harness,
                &Manager {
                    method: InstallMethod::Npm,
                    program,
                    bin: bin.clone(),
                    global: Some(prefix.clone()),
                },
                Some(&prefix),
                &bin,
            )
        });
        return ("npm".into(), command);
    }
    if let Some((prefix, name, cask)) = homebrew_owner(real, harness) {
        let brew = resolve_executable(&prefix.join("bin/brew").to_string_lossy())
            .or_else(|| environment.tools.get("brew").cloned());
        let mut command = None;
        if let Some(brew) = brew {
            let probe = MaintenanceCommand::new(&brew, ["--prefix"]);
            if environment
                .output(&probe)
                .await
                .is_some_and(|value| same_path(&prefix, Path::new(&value)))
            {
                let mut args = vec!["upgrade".into()];
                if cask {
                    args.push("--cask".into());
                }
                args.push(name);
                command = Some(MaintenanceCommand::new(brew, args));
            }
        }
        return ("Homebrew".into(), command);
    }
    if let Some(id) = winget_id(harness)
        && target.to_ascii_lowercase().contains(&format!(
            "/microsoft/winget/packages/{}_",
            id.to_ascii_lowercase()
        ))
    {
        return (
            "WinGet".into(),
            environment.command(
                "winget",
                [
                    "upgrade",
                    "--id",
                    id,
                    "--exact",
                    "--source",
                    "winget",
                    "--accept-package-agreements",
                    "--accept-source-agreements",
                    "--disable-interactivity",
                ],
            ),
        );
    }
    let scoop = environment.home_for("SCOOP", "scoop");
    let scoop_name = match harness {
        HarnessKind::Claude => "claude-code",
        HarnessKind::Codex => "codex",
        HarnessKind::Omp => "omp",
    };
    if inside(real, &scoop.join("apps").join(scoop_name)) {
        return (
            "Scoop".into(),
            environment.command("scoop", ["update", scoop_name]),
        );
    }
    let volta = environment.home_for("VOLTA_HOME", ".volta");
    if inside(executable, &volta.join("bin")) || inside(real, &volta.join("tools/image/packages")) {
        return (
            "Volta".into(),
            environment.command("volta", ["install".into(), format!("{name}@latest")]),
        );
    }
    let native = match harness {
        HarnessKind::Claude => {
            inside(real, &environment.home.join(".local/share/claude/versions"))
                || (environment.os == "windows"
                    && same_path(executable, &environment.home.join(".local/bin/claude.exe")))
        }
        HarnessKind::Codex => inside(
            real,
            &environment
                .home_for("CODEX_HOME", ".codex")
                .join("packages/standalone"),
        ),
        HarnessKind::Omp => {
            let default = if environment.os == "windows" {
                environment
                    .variables
                    .get("LOCALAPPDATA")
                    .map(|path| PathBuf::from(path).join("omp"))
                    .unwrap_or_else(|| environment.home.join("AppData/Local/omp"))
            } else {
                environment.home.join(".local/bin")
            };
            let directory = environment
                .variables
                .get("PI_INSTALL_DIR")
                .map(PathBuf::from)
                .unwrap_or(default);
            executable
                .parent()
                .is_some_and(|parent| same_path(parent, &directory))
                && same_path(executable, real)
        }
    };
    if native {
        let command = if harness == HarnessKind::Codex {
            // A user may explicitly select a pinned version inside releases/.
            if slash(executable)
                .to_ascii_lowercase()
                .contains("/packages/standalone/releases/")
            {
                None
            } else {
                native_install(harness, environment).map(|command| {
                    command
                        .with_env(
                            "CODEX_INSTALL_DIR",
                            executable.parent().unwrap_or(executable).to_string_lossy(),
                        )
                        .with_env(
                            "CODEX_HOME",
                            environment
                                .home_for("CODEX_HOME", ".codex")
                                .to_string_lossy(),
                        )
                })
            }
        } else {
            Some(MaintenanceCommand::new(executable, ["update"]))
        };
        return ("官方安装器".into(), command);
    }
    if environment.os == "linux" {
        for (tool, args, label) in [
            ("dpkg-query", vec!["-S"], "apt / dpkg"),
            ("rpm", vec!["-qf", "--queryformat", "%{NAME}"], "dnf / rpm"),
            ("pacman", vec!["-Qoq"], "pacman"),
            ("apk", vec!["info", "--who-owns"], "apk"),
        ] {
            if let Some(mut command) = environment.command(tool, args) {
                command.args.push(real.display().to_string());
                if environment
                    .output(&command)
                    .await
                    .is_some_and(|output| !output.is_empty())
                {
                    return (
                        LocalizedText::new(
                            "系统包管理器：{manager}（请在终端更新）",
                            &[("manager", label.into())],
                        ),
                        None,
                    );
                }
            }
        }
    }
    ("自定义 / 未知来源".into(), None)
}

fn installed_version(output: &str) -> Option<String> {
    output.split_whitespace().find_map(|word| {
        let word = word.trim_matches(|ch| matches!(ch, '(' | ')' | '[' | ']' | ','));
        let word = word.strip_prefix("omp/").unwrap_or(word);
        semver::Version::parse(word.trim_start_matches('v'))
            .ok()
            .map(|version| version.to_string())
    })
}

fn real_command(executable: &Path) -> PathBuf {
    // Scoop's .exe launcher has a .shim sidecar rather than a symlink.
    if let Ok(sidecar) = fs::read_to_string(executable.with_extension("shim"))
        && sidecar.len() <= 8192
        && let Some(path) = sidecar
            .lines()
            .find_map(|line| line.trim().strip_prefix("path = "))
    {
        let path = PathBuf::from(path.trim_matches('"'));
        if path.is_absolute() && path.is_file() {
            return fs::canonicalize(&path).unwrap_or(path);
        }
    }
    fs::canonicalize(executable).unwrap_or_else(|_| executable.into())
}

async fn scan_one(
    harness: HarnessKind,
    configured: String,
    environment: &Environment,
    managers: &[Manager],
) -> HarnessInstallation {
    let direct = environment.resolve(&configured);
    let executable = direct.clone().or_else(|| {
        (configured == harness.default_executable())
            .then(|| {
                managers.iter().find_map(|manager| {
                    environment.resolve(&manager.bin.join(&configured).to_string_lossy())
                })
            })
            .flatten()
    });
    let discovered_from_manager = direct.is_none() && executable.is_some();
    let mut installation = HarnessInstallation {
        configured,
        executable,
        discovered_from_manager,
        version: None,
        source: "未安装".into(),
        diagnostic: None,
        update: None,
        install_options: Vec::new(),
    };
    if let Some(executable) = &installation.executable {
        let (source, update) = ownership(
            harness,
            executable,
            &real_command(executable),
            environment,
            managers,
        )
        .await;
        installation.source = source;
        installation.update = update;
        let command = MaintenanceCommand::new(executable, ["--version"]);
        match run_command(&command, environment, PROBE_TIMEOUT)
            .await
            .and_then(|output| installed_version(&output).context("命令未返回有效版本号"))
        {
            Ok(version) => installation.version = Some(version),
            Err(error) => {
                installation.diagnostic = Some(LocalizedText::new(
                    "版本检测失败：{error}",
                    &[("error", error.to_string())],
                ))
            }
        }
        if installation.update.is_none() && installation.diagnostic.is_none() {
            installation.diagnostic = Some("请使用原安装方式更新，或打开安装文档查看说明。".into());
        }
    } else if installation.configured == harness.default_executable() {
        installation.install_options = install_options(harness, environment, managers);
    } else {
        installation.diagnostic = Some("自定义路径不存在。请修正路径，或恢复命令名后安装。".into());
    }
    installation
}

async fn execute_request(
    request: &MaintenanceRequest,
    environment: &Environment,
    managers: &[Manager],
) -> Result<()> {
    let current = scan_one(
        request.harness,
        request.configured.clone(),
        environment,
        managers,
    )
    .await;
    let command = match request.method {
        Some(method) => current
            .install_options
            .iter()
            .find(|option| option.method == method)
            .map(|option| &option.command),
        None => current.update.as_ref(),
    };
    ensure!(
        current.executable == request.executable && command == Some(&request.command),
        "安装来源已变化，请重新扫描后再试。"
    );
    run_command(&request.command, environment, INSTALL_TIMEOUT).await?;
    Ok(())
}

pub(crate) fn spawn(
    configured: Vec<(HarnessKind, String)>,
    request: Option<MaintenanceRequest>,
) -> Result<Worker> {
    let (send, events) = mpsc::channel();
    let (cancel, cancellation) = watch::channel(false);
    let thread = std::thread::Builder::new()
        .name("harness-installation".into())
        .spawn(move || {
            let result = (|| -> Result<()> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                let environment = Environment::current(cancellation)?;
                runtime.block_on(async {
                    let managers = managers(&environment).await;
                    if let Some(request) = &request {
                        execute_request(request, &environment, &managers).await?;
                    }
                    let refreshed = if request.is_some() {
                        Some(self::managers(&environment).await)
                    } else {
                        None
                    };
                    let results = futures_util::future::join_all(configured.into_iter().map(
                        |(harness, configured)| {
                            let environment = &environment;
                            let managers = refreshed.as_deref().unwrap_or(&managers);
                            async move {
                                (
                                    harness,
                                    scan_one(harness, configured, environment, managers).await,
                                )
                            }
                        },
                    ))
                    .await;
                    ensure!(!*environment.cancel.borrow(), "操作已取消");
                    let mut verified = request.is_none();
                    for (harness, installation) in results {
                        if request
                            .as_ref()
                            .is_some_and(|request| request.harness == harness)
                        {
                            verified = installation.version.is_some();
                        }
                        let _ = send.send(Event::Scanned(harness, installation));
                    }
                    ensure!(
                        verified,
                        "命令已结束，但无法验证 Harness 版本。请检查诊断后重试。"
                    );
                    Ok(())
                })
            })();
            let result = result.map_err(|error| {
                LocalizedText::new(
                    "Harness 管理失败：{error}",
                    &[("error", format!("{error:#}"))],
                )
            });
            let _ = send.send(Event::Finished { request, result });
        })?;
    Ok(Worker {
        events,
        cancel,
        thread: Some(thread),
    })
}

async fn read_output(mut stream: impl tokio::io::AsyncRead + Unpin) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Ok(output);
        }
        output.extend_from_slice(&buffer[..count]);
        if output.len() > MAX_OUTPUT {
            output.drain(..output.len() - MAX_OUTPUT);
        }
    }
}

async fn run_command(
    spec: &MaintenanceCommand,
    environment: &Environment,
    duration: Duration,
) -> Result<String> {
    ensure!(!*environment.cancel.borrow(), "操作已取消");
    let mut command = Command::new(&spec.program);
    command.args(&spec.args);
    #[cfg(windows)]
    if spec
        .program
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ps1"))
    {
        command = Command::new("powershell.exe");
        command
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&spec.program)
            .args(&spec.args);
    }
    let mut paths = spec
        .program
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .collect::<Vec<_>>();
    paths.extend(environment.paths.iter().cloned());
    command
        .current_dir(&environment.home)
        .env("NO_COLOR", "1")
        .env_remove("FORCE_COLOR")
        .env("PATH", env::join_paths(paths)?)
        .envs(&spec.environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    nexus_runner::configure_child_process(&mut command);
    let mut child = command
        .spawn()
        .with_context(|| format!("启动 {}", spec.program.display()))?;
    let pid = child.id().context("无法获取子进程 ID")?;
    let stdout = child.stdout.take().context("无法读取命令输出")?;
    let stderr = child.stderr.take().context("无法读取命令诊断")?;
    let mut cancellation = environment.cancel.clone();
    let result = tokio::select! {
        result = tokio::time::timeout(duration, async {
            tokio::join!(child.wait(), read_output(stdout), read_output(stderr))
        }) => result.ok(),
        _ = cancellation.changed() => None,
    };
    let Some((status, stdout, stderr)) = result else {
        let _ = nexus_runner::terminate_child_process(&mut child, pid).await;
        bail!(if *cancellation.borrow() {
            "操作已取消"
        } else {
            "命令执行超时"
        });
    };
    let status = status?;
    let stdout = String::from_utf8_lossy(&stdout?).into_owned();
    let stderr = String::from_utf8_lossy(&stderr?).into_owned();
    ensure!(
        status.success(),
        "{} ({status})\n{}",
        spec.program.display(),
        if stderr.trim().is_empty() {
            &stdout
        } else {
            &stderr
        }
    );
    Ok(if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Language;

    fn fixture() -> (tempfile::TempDir, watch::Sender<bool>, Environment) {
        let directory = tempfile::tempdir().unwrap();
        let (cancel, cancellation) = watch::channel(false);
        let home = directory.path().to_path_buf();
        let environment = Environment {
            os: env::consts::OS,
            home: home.clone(),
            variables: BTreeMap::new(),
            tools: BTreeMap::new(),
            paths: vec![home.join("bin"), home.join(".local/bin")],
            cancel: cancellation,
        };
        (directory, cancel, environment)
    }

    fn file(path: &Path, content: &str) -> PathBuf {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path.into()
    }

    fn manager(
        environment: &Environment,
        method: InstallMethod,
        tool: &str,
        bin: &str,
        global: &str,
    ) -> Manager {
        Manager {
            method,
            program: file(&environment.home.join("tools").join(tool), "fixture"),
            bin: environment.home.join(bin),
            global: Some(environment.home.join(global)),
        }
    }

    fn source(source: &LocalizedText) -> &str {
        source.render(Language::Chinese)
    }

    #[test]
    fn version_detection_rejects_success_messages_without_a_version() {
        for (output, version) in [
            ("2.1.263 (Claude Code)\n", "2.1.263"),
            ("codex-cli 0.110.0", "0.110.0"),
            ("omp/18.1.11\n", "18.1.11"),
            ("v3.20.1", "3.20.1"),
        ] {
            assert_eq!(installed_version(output).as_deref(), Some(version));
        }
        assert!(installed_version("installation successful").is_none());
        assert!(installed_version("").is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn installation_into_a_non_path_npm_prefix_becomes_discoverable() {
        let (_directory, _cancel, environment) = fixture();
        let mut npm = manager(
            &environment,
            InstallMethod::Npm,
            "npm",
            "custom-npm/bin",
            "custom-npm",
        );
        npm.program = file(
            &environment.home.join("tools/npm"),
            "#!/bin/sh\n/bin/mkdir -p custom-npm/bin custom-npm/lib/node_modules/@openai/codex/bin\nprintf '#!/bin/sh\\necho codex-cli 1.2.3\\n' > custom-npm/lib/node_modules/@openai/codex/bin/codex\n/bin/chmod +x custom-npm/lib/node_modules/@openai/codex/bin/codex\n/bin/ln -s ../lib/node_modules/@openai/codex/bin/codex custom-npm/bin/codex\n",
        );
        let managers = [npm];
        let before = scan_one(HarnessKind::Codex, "codex".into(), &environment, &managers).await;
        assert!(before.executable.is_none());
        let request = MaintenanceRequest {
            harness: HarnessKind::Codex,
            configured: "codex".into(),
            executable: None,
            method: Some(InstallMethod::Npm),
            command: before.install_options[0].command.clone(),
        };
        execute_request(&request, &environment, &managers)
            .await
            .unwrap();
        let after = scan_one(HarnessKind::Codex, "codex".into(), &environment, &managers).await;
        assert_eq!(after.version.as_deref(), Some("1.2.3"));
        assert!(after.discovered_from_manager);
        assert!(after.install_options.is_empty());
        assert!(
            execute_request(&request, &environment, &managers)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn vp_proxy_and_isolated_packages_keep_their_vp_home() {
        for home in [".vite-plus", "custom location/vite"] {
            let (_directory, _cancel, mut environment) = fixture();
            let root = environment.home.join(home);
            environment
                .variables
                .insert("VP_HOME".into(), root.display().to_string());
            let vp = file(&root.join("current/bin/vp"), "fixture");
            environment.tools.insert("vp".into(), vp.clone());
            let entry = root.join("bin/codex");
            let managers = vec![Manager {
                method: InstallMethod::VitePlus,
                program: vp.clone(),
                bin: root.join("bin"),
                global: Some(root.join("packages")),
            }];
            for real in [
                vp,
                root.join("packages/@openai/codex/lib/node_modules/@openai/codex/bin/codex.js"),
            ] {
                let (owner, command) =
                    ownership(HarnessKind::Codex, &entry, &real, &environment, &managers).await;
                assert_eq!(source(&owner), "Vite+ (vp)");
                let command = command.unwrap();
                assert_eq!(command.args, ["install", "-g", "@openai/codex@latest"]);
                assert_eq!(command.environment["VP_HOME"], root.display().to_string());
            }
        }
    }

    #[tokio::test]
    async fn bun_custom_store_preserves_both_store_and_binary_directory() {
        let (_directory, _cancel, environment) = fixture();
        let bun = manager(
            &environment,
            InstallMethod::Bun,
            "bun",
            "custom-bin",
            "custom-global",
        );
        let real = bun
            .global
            .as_ref()
            .unwrap()
            .join("node_modules/@oh-my-pi/pi-coding-agent/dist/cli.js");
        let entry = bun.bin.join("omp");
        let (owner, command) =
            ownership(HarnessKind::Omp, &entry, &real, &environment, &[bun]).await;
        assert_eq!(source(&owner), "Bun");
        let command = command.unwrap();
        assert_eq!(
            command.args,
            ["add", "-g", "--trust", "@oh-my-pi/pi-coding-agent@latest"]
        );
        assert_eq!(
            command.environment["BUN_INSTALL_GLOBAL_DIR"],
            environment.home.join("custom-global").display().to_string()
        );
        assert_eq!(
            command.environment["BUN_INSTALL_BIN"],
            environment.home.join("custom-bin").display().to_string()
        );
    }

    #[tokio::test]
    async fn pnpm_global_layouts_and_yarn_custom_stores_use_their_own_manager() {
        for (method, tool, bin, global, package_path) in [
            (
                InstallMethod::Pnpm,
                "pnpm",
                "Library/pnpm",
                "Library/pnpm/global/5",
                "node_modules/.pnpm/@openai+codex@1/node_modules/@openai/codex/bin/codex.js",
            ),
            (
                InstallMethod::Pnpm,
                "pnpm",
                ".local/share/pnpm/bin",
                ".local/share/pnpm/global/v11",
                "store/package/node_modules/@openai/codex/bin/codex.js",
            ),
            (
                InstallMethod::Yarn,
                "yarn",
                "custom-yarn/bin",
                "custom-yarn-global",
                "node_modules/@openai/codex/bin/codex.js",
            ),
        ] {
            let (_directory, _cancel, environment) = fixture();
            let manager = manager(&environment, method, tool, bin, global);
            let real = manager.global.as_ref().unwrap().join(package_path);
            let entry = manager.bin.join("codex");
            let program = manager.program.clone();
            let (owner, command) =
                ownership(HarnessKind::Codex, &entry, &real, &environment, &[manager]).await;
            assert_eq!(source(&owner), method.label());
            let command = command.unwrap();
            assert_eq!(command.program, program);
            assert!(command.args.contains(&"@openai/codex@latest".into()));
            if method == InstallMethod::Yarn {
                assert_eq!(&command.args[..2], ["global", "add"]);
                assert!(
                    command
                        .args
                        .contains(&environment.home.join(global).display().to_string())
                );
            } else {
                assert_eq!(command.args, ["add", "-g", "@openai/codex@latest"]);
            }
        }
    }

    #[tokio::test]
    async fn shared_bin_directory_never_implies_yarn_or_bun_ownership() {
        let (_directory, _cancel, mut environment) = fixture();
        let npm = file(&environment.home.join("node/bin/npm"), "fixture");
        environment.tools.insert("npm".into(), npm.clone());
        let managers = [
            manager(
                &environment,
                InstallMethod::Yarn,
                "yarn",
                "bin",
                "yarn/global",
            ),
            manager(
                &environment,
                InstallMethod::Bun,
                "bun",
                "bin",
                "bun/install/global",
            ),
        ];
        let entry = environment.home.join("bin/codex");
        let real = environment
            .home
            .join("node/lib/node_modules/@openai/codex/bin/codex.js");
        let (owner, command) =
            ownership(HarnessKind::Codex, &entry, &real, &environment, &managers).await;
        assert_eq!(source(&owner), "npm");
        assert_eq!(command.unwrap().program, npm);
        let (owner, command) =
            ownership(HarnessKind::Codex, &entry, &entry, &environment, &managers).await;
        assert_eq!(source(&owner), "自定义 / 未知来源");
        assert!(command.is_none());
        let unrelated = file(
            &environment.home.join(".vite-plus/bin/codex"),
            "custom binary",
        );
        assert!(
            ownership(
                HarnessKind::Codex,
                &unrelated,
                &unrelated,
                &environment,
                &[]
            )
            .await
            .1
            .is_none()
        );
    }

    #[test]
    fn npm_prefix_covers_node_version_managers_and_native_optional_packages() {
        for prefix in [
            "/home/u/.nvm/versions/node/v24.0.0",
            "/home/u/fnm/node-versions/v24/installation",
            "/opt/homebrew/Cellar/node/24.0.0",
            "/home/u/.local/share/mise/installs/node/24.0.0",
            "/custom prefix",
        ] {
            for package_path in [
                "@anthropic-ai/claude-code/cli.js",
                "@anthropic-ai/claude-code-darwin-arm64/claude",
            ] {
                assert_eq!(
                    npm_prefix(
                        &PathBuf::from(format!("{prefix}/lib/node_modules/{package_path}")),
                        package(HarnessKind::Claude)
                    ),
                    Some(prefix.into())
                );
            }
        }
        for path in [
            "/project/node_modules/@openai/codex/bin/codex.js",
            "/project/node_modules/x/lib/node_modules/@openai/codex/bin/codex.js",
            "/home/u/.local/share/mise/installs/npm-openai-codex/1/lib/node_modules/@openai/codex/bin/codex.js",
        ] {
            assert!(npm_prefix(Path::new(path), package(HarnessKind::Codex)).is_none());
        }
    }

    #[tokio::test]
    async fn npm_windows_shims_need_the_proven_global_prefix() {
        let (_directory, _cancel, mut environment) = fixture();
        environment.os = "windows";
        let npm = manager(
            &environment,
            InstallMethod::Npm,
            "npm.cmd",
            "AppData/Roaming/npm",
            "AppData/Roaming/npm",
        );
        environment.tools.insert("npm".into(), npm.program.clone());
        let entry = file(
            &npm.bin.join("codex.cmd"),
            "@echo off\r\nnode node_modules/@openai/codex/bin/codex.js\r\n",
        );
        file(
            &npm.bin.join("node_modules/@openai/codex/package.json"),
            "{}",
        );
        let prefix = npm.bin.clone();
        let unrelated = file(&prefix.join("codex.exe"), "custom binary");
        assert!(
            ownership(
                HarnessKind::Codex,
                &unrelated,
                &unrelated,
                &environment,
                std::slice::from_ref(&npm)
            )
            .await
            .1
            .is_none()
        );
        let (owner, command) =
            ownership(HarnessKind::Codex, &entry, &entry, &environment, &[npm]).await;
        assert_eq!(source(&owner), "npm");
        let command = command.unwrap();
        assert_eq!(
            &command.args[command.args.len() - 2..],
            ["--prefix", &prefix.display().to_string()]
        );
        let project_entry = file(&environment.home.join("project/codex.cmd"), "fixture");
        file(
            &environment
                .home
                .join("project/node_modules/@openai/codex/package.json"),
            "{}",
        );
        assert!(
            ownership(
                HarnessKind::Codex,
                &project_entry,
                &project_entry,
                &environment,
                &[]
            )
            .await
            .1
            .is_none()
        );
    }

    #[test]
    fn homebrew_recognizes_formula_and_cask_without_claiming_the_node_runtime() {
        for (path, harness, expected, cask) in [
            (
                "/opt/homebrew/Caskroom/claude-code@latest/2.1/claude",
                HarnessKind::Claude,
                "claude-code@latest",
                true,
            ),
            (
                "/usr/local/Caskroom/codex/0.100/codex",
                HarnessKind::Codex,
                "codex",
                true,
            ),
            (
                "/home/linuxbrew/.linuxbrew/Cellar/omp/3.0/bin/omp",
                HarnessKind::Omp,
                "omp",
                false,
            ),
        ] {
            let owner = homebrew_owner(Path::new(path), harness).unwrap();
            assert_eq!(owner.1, expected);
            assert_eq!(owner.2, cask);
        }
        assert!(
            homebrew_owner(
                Path::new("/opt/homebrew/Cellar/node/24/bin/codex"),
                HarnessKind::Codex
            )
            .is_none()
        );
        assert!(
            homebrew_owner(
                Path::new("/opt/homebrew/Cellar/mise/1/bin/mise"),
                HarnessKind::Codex
            )
            .is_none()
        );
    }

    #[tokio::test]
    async fn native_installations_use_native_updaters_and_keep_codex_location() {
        let (_directory, _cancel, mut environment) = fixture();
        environment.os = "linux";
        environment
            .tools
            .insert("bash".into(), environment.home.join("bin/bash"));
        for (harness, entry, real) in [
            (
                HarnessKind::Claude,
                ".local/bin/claude",
                ".local/share/claude/versions/2.1",
            ),
            (
                HarnessKind::Codex,
                ".local/bin/codex",
                ".codex/packages/standalone/releases/1.0/bin/codex",
            ),
            (HarnessKind::Omp, ".local/bin/omp", ".local/bin/omp"),
        ] {
            let (owner, command) = ownership(
                harness,
                &environment.home.join(entry),
                &environment.home.join(real),
                &environment,
                &[],
            )
            .await;
            assert_eq!(source(&owner), "官方安装器");
            let command = command.unwrap();
            if harness == HarnessKind::Codex {
                assert_eq!(
                    command.environment["CODEX_INSTALL_DIR"],
                    environment.home.join(".local/bin").display().to_string()
                );
                assert_eq!(command.environment["CODEX_NON_INTERACTIVE"], "1");
            } else {
                assert_eq!(command.args, ["update"]);
            }
        }
    }

    #[tokio::test]
    async fn unknown_pinned_project_and_declarative_installs_are_manual() {
        let (_directory, _cancel, environment) = fixture();
        for path in [
            "project/node_modules/@openai/codex/bin/codex.js",
            "project/codex",
            ".local/share/mise/shims/codex",
            ".asdf/shims/codex",
            "nix/store/hash-codex/bin/codex",
            "Codex.app/Contents/Resources/codex",
            ".codex/packages/standalone/releases/1/bin/codex",
        ] {
            let path = environment.home.join(path);
            assert!(
                ownership(HarnessKind::Codex, &path, &path, &environment, &[])
                    .await
                    .1
                    .is_none()
            );
        }
    }

    #[tokio::test]
    async fn winget_and_scoop_updates_target_the_detected_package() {
        let (_directory, _cancel, mut environment) = fixture();
        environment.os = "windows";
        environment
            .tools
            .insert("winget".into(), environment.home.join("winget.exe"));
        environment
            .tools
            .insert("scoop".into(), environment.home.join("scoop.ps1"));
        for (harness, id) in [
            (HarnessKind::Claude, "Anthropic.ClaudeCode"),
            (HarnessKind::Codex, "OpenAI.Codex"),
        ] {
            let path = environment.home.join(format!(
                "AppData/Local/Microsoft/WinGet/Packages/{id}_source/cli.exe"
            ));
            let (owner, command) = ownership(harness, &path, &path, &environment, &[]).await;
            assert_eq!(source(&owner), "WinGet");
            assert!(command.unwrap().args.contains(&id.into()));
        }
        let real = file(
            &environment.home.join("scoop/apps/codex/current/codex.exe"),
            "fixture",
        );
        let entry = file(&environment.home.join("scoop/shims/codex.exe"), "shim");
        file(
            &entry.with_extension("shim"),
            &format!("path = \"{}\"\n", real.display()),
        );
        let (owner, command) = ownership(
            HarnessKind::Codex,
            &entry,
            &real_command(&entry),
            &environment,
            &[],
        )
        .await;
        assert_eq!(source(&owner), "Scoop");
        assert_eq!(command.unwrap().args, ["update", "codex"]);
    }

    #[tokio::test]
    async fn missing_tools_offer_installation_but_missing_overrides_need_correction() {
        let (_directory, _cancel, environment) = fixture();
        let npm = manager(&environment, InstallMethod::Npm, "npm", "bin", "prefix");
        let managers = [npm];
        let installation =
            scan_one(HarnessKind::Codex, "codex".into(), &environment, &managers).await;
        assert!(installation.executable.is_none());
        assert_eq!(installation.install_options.len(), 1);
        assert_eq!(installation.install_options[0].method, InstallMethod::Npm);
        let installation = scan_one(
            HarnessKind::Codex,
            environment.home.join("missing/codex").display().to_string(),
            &environment,
            &managers,
        )
        .await;
        assert!(installation.install_options.is_empty());
        assert!(installation.diagnostic.is_some());
    }

    #[test]
    fn native_install_commands_use_pipefail_and_force_omp_binary_mode() {
        let (_directory, _cancel, mut environment) = fixture();
        environment.tools.insert("bash".into(), "/bin/bash".into());
        environment
            .tools
            .insert("powershell".into(), "powershell.exe".into());
        for os in ["macos", "linux", "windows"] {
            environment.os = os;
            let command = native_install(HarnessKind::Omp, &environment).unwrap();
            assert!(command.args.last().unwrap().contains(if os == "windows" {
                "-Binary"
            } else {
                "--binary"
            }));
            if os != "windows" {
                assert_eq!(&command.args[..3], ["-o", "pipefail", "-c"]);
            }
        }
        assert_eq!(quote("a'b $x", false), "'a'\\''b $x'");
        assert_eq!(quote("C:\\a'b $x", true), "'C:\\a''b $x'");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execution_rechecks_source_and_rescans_the_actual_updated_version() {
        let (_directory, _cancel, environment) = fixture();
        let version = environment.home.join("version");
        file(&version, "1.0.0\n");
        let cli = file(
            &environment.home.join(".local/bin/omp"),
            "#!/bin/sh\nif [ \"$1\" = update ]; then echo 2.0.0 > version; else /bin/cat version; fi\n",
        );
        let initial = scan_one(HarnessKind::Omp, "omp".into(), &environment, &[]).await;
        assert_eq!(initial.version.as_deref(), Some("1.0.0"));
        let request = MaintenanceRequest {
            harness: HarnessKind::Omp,
            configured: "omp".into(),
            executable: Some(cli),
            method: None,
            command: initial.update.unwrap(),
        };
        let mut stale = request.clone();
        stale.command.args = vec!["unapproved-command".into()];
        assert!(execute_request(&stale, &environment, &[]).await.is_err());
        assert_eq!(fs::read_to_string(&version).unwrap().trim(), "1.0.0");
        execute_request(&request, &environment, &[]).await.unwrap();
        assert_eq!(
            scan_one(HarnessKind::Omp, "omp".into(), &environment, &[])
                .await
                .version
                .as_deref(),
            Some("2.0.0")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runner_passes_arguments_literally_and_preserves_failure_output() {
        let (_directory, _cancel, environment) = fixture();
        let cli = file(
            &environment.home.join("space and 'quotes'/fake"),
            "#!/bin/sh\nprintf '%s' \"$1\"\n",
        );
        let command = MaintenanceCommand::new(cli, ["$(touch unexpected); & literal"]);
        assert_eq!(
            run_command(&command, &environment, PROBE_TIMEOUT)
                .await
                .unwrap(),
            "$(touch unexpected); & literal"
        );
        assert!(!environment.home.join("unexpected").exists());
        let cli = file(
            &environment.home.join("failure"),
            "#!/bin/sh\necho useful-diagnostic >&2\nexit 7\n",
        );
        let error = run_command(
            &MaintenanceCommand::new(cli, [] as [&str; 0]),
            &environment,
            PROBE_TIMEOUT,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("useful-diagnostic"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_and_cancel_terminate_descendants_and_drain_large_output() {
        let (_directory, cancel, environment) = fixture();
        let noisy = file(
            &environment.home.join("noisy"),
            "#!/bin/sh\n/usr/bin/yes x | /usr/bin/head -c 100000\nprintf tail\n",
        );
        let output = run_command(
            &MaintenanceCommand::new(noisy, [] as [&str; 0]),
            &environment,
            PROBE_TIMEOUT,
        )
        .await
        .unwrap();
        assert_eq!(output.len(), MAX_OUTPUT);
        assert!(output.ends_with("tail"));
        let sleeper = file(
            &environment.home.join("sleeper"),
            "#!/bin/sh\n(/bin/sleep 0.3; echo leaked > leaked) &\nwait\n",
        );
        let spec = MaintenanceCommand::new(sleeper, [] as [&str; 0]);
        assert!(
            run_command(&spec, &environment, Duration::from_millis(80))
                .await
                .unwrap_err()
                .to_string()
                .contains("超时")
        );
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!environment.home.join("leaked").exists());
        let run = run_command(&spec, &environment, PROBE_TIMEOUT);
        let cancellation = async {
            tokio::time::sleep(Duration::from_millis(80)).await;
            cancel.send(true).unwrap();
        };
        let (result, ()) = tokio::join!(run, cancellation);
        assert!(result.unwrap_err().to_string().contains("取消"));
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!environment.home.join("leaked").exists());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_batch_shims_execute_without_a_console() {
        let (_directory, _cancel, environment) = fixture();
        let cli = file(
            &environment.home.join("space directory/codex.cmd"),
            "@echo off\r\necho codex 1.0\r\n",
        );
        assert_eq!(
            run_command(
                &MaintenanceCommand::new(cli, ["--version"]),
                &environment,
                PROBE_TIMEOUT
            )
            .await
            .unwrap()
            .trim(),
            "codex 1.0"
        );
    }
}
