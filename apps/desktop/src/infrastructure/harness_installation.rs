mod worker;
use worker::run_command;
pub(crate) use worker::spawn;

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::mpsc::{self, Receiver},
    time::Duration,
};

use anyhow::{Context as _, Result, bail, ensure};
use gpui_kit::http_client::{AsyncBody, HttpClient, HttpRequestExt as _, Request};
use nexus_domain::HarnessKind;
use nexus_harness_core::{executable_search_paths, resolve_executable, resolve_in_paths};
use smol::io::AsyncReadExt as _;
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
    harness.info().documentation
}

fn package(harness: HarnessKind) -> &'static str {
    harness.info().npm_package
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
            "KIMI_INSTALL_DIR",
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
            "uv",
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
        // OpenCode 官方只提供 Unix 安装脚本，Windows 走 npm / Scoop / Chocolatey。
        HarnessKind::Opencode if environment.os == "windows" => return None,
        HarnessKind::Opencode => ("curl -fsSL https://opencode.ai/install | bash", ""),
        HarnessKind::Pi
        | HarnessKind::Qoder
        | HarnessKind::QoderCn
        | HarnessKind::Codebuddy
        // dsh 和 Command Code 只有 npm 分发，没有官方安装脚本。
        | HarnessKind::Deepseek
        | HarnessKind::CommandCode => {
            return None;
        }
        HarnessKind::Kimi => (
            "curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash",
            "& ([scriptblock]::Create((Invoke-RestMethod https://code.kimi.com/kimi-code/install.ps1)))",
        ),
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
    if harness == HarnessKind::Kimi {
        return native_install(harness, environment)
            .map(|command| InstallOption {
                method: InstallMethod::Native,
                command,
            })
            .into_iter()
            .collect();
    }
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
            HarnessKind::Opencode => vec!["install", "opencode"],
            HarnessKind::Pi
            | HarnessKind::Kimi
            | HarnessKind::Qoder
            | HarnessKind::QoderCn
            | HarnessKind::Codebuddy
            | HarnessKind::Deepseek
            | HarnessKind::CommandCode => {
                vec![]
            }
        },
    ) && matches!(
        harness,
        HarnessKind::Claude | HarnessKind::Codex | HarnessKind::Omp | HarnessKind::Opencode
    ) && (environment.os == "macos"
        || matches!(harness, HarnessKind::Omp | HarnessKind::Opencode))
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
        HarnessKind::Omp
        | HarnessKind::Pi
        | HarnessKind::Kimi
        | HarnessKind::Qoder
        | HarnessKind::QoderCn
        | HarnessKind::Codebuddy
        | HarnessKind::Opencode
        | HarnessKind::Deepseek
        | HarnessKind::CommandCode => None,
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
        HarnessKind::Pi
        | HarnessKind::Kimi
        | HarnessKind::Qoder
        | HarnessKind::QoderCn
        | HarnessKind::Codebuddy
        | HarnessKind::Deepseek
        | HarnessKind::CommandCode => {
            return None;
        }
        HarnessKind::Claude => "claude-code",
        HarnessKind::Codex => "codex",
        HarnessKind::Omp => "omp",
        HarnessKind::Opencode => "opencode",
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
        HarnessKind::Pi => "pi",
        HarnessKind::Kimi => "kimi",
        HarnessKind::Qoder => "qoder",
        HarnessKind::QoderCn => "qodercn",
        HarnessKind::Codebuddy => "codebuddy",
        HarnessKind::Opencode => "opencode",
        HarnessKind::Deepseek => "deepseek",
        HarnessKind::CommandCode => "command-code",
    };
    if matches!(
        harness,
        HarnessKind::Claude | HarnessKind::Codex | HarnessKind::Omp | HarnessKind::Opencode
    ) && inside(real, &scoop.join("apps").join(scoop_name))
    {
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
        HarnessKind::Pi
        | HarnessKind::Qoder
        | HarnessKind::QoderCn
        | HarnessKind::Codebuddy
        | HarnessKind::Deepseek
        | HarnessKind::CommandCode => false,
        HarnessKind::Kimi => inside(
            real,
            &environment
                .home_for("KIMI_INSTALL_DIR", ".kimi-code")
                .join("bin"),
        ),
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
        HarnessKind::Opencode => inside(real, &environment.home.join(".opencode").join("bin")),
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
        } else if matches!(harness, HarnessKind::Kimi | HarnessKind::Opencode) {
            // `upgrade` resolves the native install source and updates in place.
            Some(MaintenanceCommand::new(executable, ["upgrade"]))
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

async fn latest_version(
    harness: HarnessKind,
    http: &dyn HttpClient,
    cancellation: &watch::Receiver<bool>,
) -> Result<String> {
    ensure!(!*cancellation.borrow(), "操作已取消");
    let mut cancellation = cancellation.clone();
    let kimi = harness == HarnessKind::Kimi;
    let lookup = async {
        let url = if kimi {
            // Kimi Code's installer publishes the latest version as plain text.
            "https://code.kimi.com/kimi-code/latest".into()
        } else {
            format!(
                "https://registry.npmjs.org/{}/latest",
                package(harness).replace('/', "%2F")
            )
        };
        let mut response = http
            .send(
                Request::get(url)
                    .header("Accept", "application/json")
                    .timeout(PROBE_TIMEOUT)
                    .body(AsyncBody::default())?,
            )
            .await?;
        ensure!(
            response.status().is_success(),
            "package registry returned HTTP {}",
            response.status()
        );
        let mut body = Vec::new();
        response
            .body_mut()
            .take((MAX_OUTPUT + 1) as u64)
            .read_to_end(&mut body)
            .await?;
        ensure!(body.len() <= MAX_OUTPUT, "package metadata is too large");
        let version = if kimi {
            std::str::from_utf8(&body)?.trim().to_owned()
        } else {
            serde_json::from_slice::<serde_json::Value>(&body)?["version"]
                .as_str()
                .context("package metadata has no version")?
                .to_owned()
        };
        Ok(semver::Version::parse(&version)?.to_string())
    };
    tokio::select! {
        result = tokio::time::timeout(PROBE_TIMEOUT, lookup) => {
            result.context("最新版本检测超时")?
        }
        _ = cancellation.changed() => bail!("操作已取消"),
    }
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
        latest_version: Err("尚未扫描".into()),
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

#[cfg(test)]
mod tests;
