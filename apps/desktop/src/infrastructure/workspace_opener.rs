use crate::i18n::{Language, LocalizedText};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenTarget {
    FileManager,
    VsCode,
    Ghostty,
}

impl OpenTarget {
    pub(crate) const ALL: [Self; 3] = [Self::FileManager, Self::VsCode, Self::Ghostty];

    pub(crate) fn supported(self, os: &str) -> bool {
        matches!(os, "macos" | "linux") || (os == "windows" && self != Self::Ghostty)
    }

    pub(crate) fn label(self, language: Language) -> &'static str {
        match self {
            Self::FileManager => match std::env::consts::OS {
                "macos" => "Finder",
                "windows" => "Explorer",
                _ => language.text("文件管理器"),
            },
            Self::VsCode => "VS Code",
            Self::Ghostty => "Ghostty",
        }
    }
}

pub(crate) async fn open(
    target: OpenTarget,
    directory: Option<String>,
) -> Result<(), LocalizedText> {
    let directory = directory
        .filter(|directory| !directory.is_empty())
        .ok_or_else(|| LocalizedText::from("尚未选择工作目录。"))?;
    let path = PathBuf::from(&directory);
    let command = smol::unblock(move || prepare(target, &path)).await?;
    launch(command).await.map_err(|error| {
        LocalizedText::translated(|language| {
            language.format(
                "无法使用 {app} 打开“{path}”。请确认应用已安装且可正常启动。{error}",
                &[
                    ("app", target.label(language).into()),
                    ("path", directory.clone()),
                    ("error", error.clone()),
                ],
            )
        })
    })
}

fn prepare(target: OpenTarget, directory: &Path) -> Result<Command, LocalizedText> {
    if !directory.is_dir() {
        return Err(LocalizedText::new(
            "工作目录不存在或无法访问：{path}",
            &[("path", directory.display().to_string())],
        ));
    }
    let directory =
        std::path::absolute(directory).map_err(|error| LocalizedText::from(error.to_string()))?;
    let spec = command(target, std::env::consts::OS, &directory)?;
    let program = spec.get_program().to_str().expect("fixed application name");
    let executable = match (std::env::consts::OS, target) {
        #[cfg(target_os = "windows")]
        ("windows", OpenTarget::VsCode) => windows_vscode(),
        ("linux", OpenTarget::VsCode) => nexus_harness_core::resolve_executable(program)
            .or_else(|| nexus_harness_core::resolve_executable("/snap/bin/code")),
        _ => nexus_harness_core::resolve_executable(program),
    }
    .ok_or_else(|| {
        LocalizedText::translated(|language| {
            language.format(
                "未找到 {app} 的启动程序（{program}）。请先安装应用并确保其在 PATH 中。",
                &[
                    ("app", target.label(language).into()),
                    ("program", program.into()),
                ],
            )
        })
    })?;
    let mut command = Command::new(executable);
    command.args(spec.get_args()).current_dir(directory);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    Ok(command)
}

fn command(target: OpenTarget, os: &str, directory: &Path) -> Result<Command, LocalizedText> {
    if !target.supported(os) {
        return Err(LocalizedText::translated(|language| {
            language.format(
                "当前平台不支持使用 {app} 打开工作目录。",
                &[("app", target.label(language).into())],
            )
        }));
    }
    // Windows project paths come from canonicalize(), but shell apps expect DOS/UNC paths.
    let directory = if os == "windows" {
        let path = directory.to_string_lossy();
        if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
            PathBuf::from(format!(r"\\{unc}"))
        } else if let Some(path) = path.strip_prefix(r"\\?\") {
            PathBuf::from(path)
        } else {
            directory.to_path_buf()
        }
    } else {
        directory.to_path_buf()
    };
    let mut command = match (os, target) {
        ("macos", _) => {
            let mut command = Command::new("/usr/bin/open");
            command.args([
                "-a",
                match target {
                    OpenTarget::FileManager => "Finder",
                    OpenTarget::VsCode => "Visual Studio Code",
                    OpenTarget::Ghostty => "Ghostty",
                },
                "--",
            ]);
            // Native folder-open events also carry the directory to an existing app.
            command.arg(&directory);
            command
        }
        ("windows", OpenTarget::FileManager) => {
            let mut command = Command::new("explorer.exe");
            command.arg(&directory);
            command
        }
        (_, OpenTarget::VsCode) => {
            let mut command = Command::new(if os == "windows" { "Code.exe" } else { "code" });
            command.arg("--new-window").arg(&directory);
            command
        }
        ("linux", OpenTarget::Ghostty) => {
            let mut command = Command::new("ghostty");
            // An existing GTK instance can otherwise inherit its previous terminal's cwd.
            let mut argument = std::ffi::OsString::from("--working-directory=");
            argument.push(&directory);
            command.arg("--gtk-single-instance=false").arg(argument);
            command
        }
        ("linux", OpenTarget::FileManager) => {
            let mut command = Command::new("xdg-open");
            command.arg(&directory);
            command
        }
        _ => unreachable!("unsupported targets were rejected above"),
    };
    command.current_dir(directory);
    Ok(command)
}

#[cfg(target_os = "windows")]
fn windows_vscode() -> Option<PathBuf> {
    // Do not execute code.cmd: cmd.exe would interpret characters in folder names.
    nexus_harness_core::resolve_executable("Code.exe")
        .or_else(|| {
            let cli = nexus_harness_core::resolve_executable("code")?;
            let executable = cli.parent()?.parent()?.join("Code.exe");
            executable.is_file().then_some(executable)
        })
        .or_else(|| {
            [
                ("LOCALAPPDATA", "Programs/Microsoft VS Code/Code.exe"),
                ("ProgramFiles", "Microsoft VS Code/Code.exe"),
                ("ProgramFiles(x86)", "Microsoft VS Code/Code.exe"),
            ]
            .into_iter()
            .filter_map(|(key, suffix)| {
                std::env::var_os(key).map(|root| PathBuf::from(root).join(suffix))
            })
            .find(|path| path.is_file())
        })
}

async fn launch(command: Command) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let explorer = Path::new(command.get_program())
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("explorer.exe"));
    let mut command = smol::process::Command::from(command);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    #[cfg(target_os = "windows")]
    if explorer {
        // Explorer hands off to the desktop shell; its exit code is not a launch result.
        return Ok(());
    }
    // Some launchers exit immediately; others stay alive with the application.
    // async-process reaps a dropped child without terminating the external app.
    let status = smol::future::race(async { child.status().await.map(Some) }, async {
        smol::Timer::after(Duration::from_secs(2)).await;
        Ok(None)
    })
    .await
    .map_err(|error| error.to_string())?;
    match status {
        Some(status) if !status.success() => Err(status.to_string()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn workspace_opener_passes_directories_as_literal_arguments_on_each_platform() {
        let directory = Path::new("/项目目录/a b &'$();%NAME%#[]");
        for (os, target, program, prefix) in [
            (
                "macos",
                OpenTarget::FileManager,
                "/usr/bin/open",
                vec!["-a", "Finder", "--"],
            ),
            (
                "macos",
                OpenTarget::VsCode,
                "/usr/bin/open",
                vec!["-a", "Visual Studio Code", "--"],
            ),
            (
                "macos",
                OpenTarget::Ghostty,
                "/usr/bin/open",
                vec!["-a", "Ghostty", "--"],
            ),
            ("windows", OpenTarget::FileManager, "explorer.exe", vec![]),
            (
                "windows",
                OpenTarget::VsCode,
                "Code.exe",
                vec!["--new-window"],
            ),
            ("linux", OpenTarget::FileManager, "xdg-open", vec![]),
            ("linux", OpenTarget::VsCode, "code", vec!["--new-window"]),
        ] {
            let command = command(target, os, directory).unwrap();
            assert_eq!(command.get_program(), program);
            let expected = prefix
                .into_iter()
                .map(OsStr::new)
                .chain([directory.as_os_str()])
                .collect::<Vec<_>>();
            assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
            assert_eq!(command.get_current_dir(), Some(directory));
        }
        let ghostty = command(OpenTarget::Ghostty, "linux", directory).unwrap();
        assert_eq!(ghostty.get_program(), "ghostty");
        assert_eq!(
            ghostty.get_args().collect::<Vec<_>>(),
            [
                OsStr::new("--gtk-single-instance=false"),
                OsStr::new("--working-directory=/项目目录/a b &'$();%NAME%#[]"),
            ]
        );
        for os in ["windows", "unsupported"] {
            let error = command(OpenTarget::Ghostty, os, directory).unwrap_err();
            assert!(error.render(Language::English).contains("not supported"));
        }
    }

    #[test]
    fn workspace_opener_rejects_missing_and_non_directory_paths() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("file");
        std::fs::write(&file, "test").unwrap();
        for path in [file, directory.path().join("missing")] {
            let error = prepare(OpenTarget::FileManager, &path).unwrap_err();
            assert!(
                error
                    .render(Language::English)
                    .contains("does not exist or cannot be accessed")
            );
            assert!(
                error
                    .render(Language::English)
                    .contains(path.to_str().unwrap())
            );
        }
        let error = smol::block_on(open(OpenTarget::FileManager, None)).unwrap_err();
        assert_eq!(
            error.render(Language::English),
            "No working directory selected."
        );
    }

    #[test]
    fn workspace_opener_converts_windows_verbatim_paths_for_external_apps() {
        for (input, expected) in [
            (r"\\?\C:\项目 a & %NAME%", r"C:\项目 a & %NAME%"),
            (r"\\?\UNC\server\share\项目 a", r"\\server\share\项目 a"),
        ] {
            for target in [OpenTarget::FileManager, OpenTarget::VsCode] {
                let command = command(target, "windows", Path::new(input)).unwrap();
                assert_eq!(command.get_args().last(), Some(OsStr::new(expected)));
            }
        }
    }

    #[test]
    fn workspace_opener_reports_spawn_and_nonzero_exit_failures() {
        let directory = tempfile::tempdir().unwrap();
        let error = smol::block_on(launch(Command::new(directory.path().join("missing"))));
        assert!(error.is_err());
        #[cfg(unix)]
        {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", "exit 7"]);
            assert!(smol::block_on(launch(command)).unwrap_err().contains('7'));
        }
    }

    #[cfg(unix)]
    #[test]
    fn workspace_opener_launch_preserves_literal_arguments_and_working_directory() {
        use std::os::unix::fs::PermissionsExt as _;
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("目录 a b &'$();%NAME%#[]");
        std::fs::create_dir(&directory).unwrap();
        let directory = directory.canonicalize().unwrap();
        let executable = temporary.path().join("fake-opener");
        let output = temporary.path().join("arguments");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$@\" > \"$NEXUS_TEST_OUTPUT\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let spec = command(OpenTarget::Ghostty, "linux", &directory).unwrap();
        let mut child = Command::new(executable);
        child
            .args(spec.get_args())
            .current_dir(&directory)
            .env("NEXUS_TEST_OUTPUT", &output);
        smol::block_on(launch(child)).unwrap();
        assert_eq!(
            std::fs::read_to_string(output).unwrap(),
            format!(
                "{}\n--gtk-single-instance=false\n--working-directory={}\n",
                directory.display(),
                directory.display(),
            )
        );
    }
}
