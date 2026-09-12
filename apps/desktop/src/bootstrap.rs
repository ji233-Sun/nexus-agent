use crate::{
    infrastructure::{fonts, runner_client::RunnerClient, storage::Storage, update_installation},
    presenter::{Presenter, RunnerPort},
    view::{NexusView, theme},
};
use anyhow::{Context as _, ensure};
use gpui_kit::component::Root;
use gpui_kit::{
    App, AppContext as _, Bounds, Styled as _, TitlebarOptions, WindowBounds, WindowOptions, point,
    px, rgba, size,
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub(crate) const RUNNER_MODE_ARG: &str = "--nexus-runner";
const APP_MODE_ARG: &str = "--nexus-app";

fn project_argument(
    arguments: impl IntoIterator<Item = OsString>,
) -> anyhow::Result<Option<PathBuf>> {
    let mut arguments = arguments.into_iter();
    let Some(first) = arguments.next() else {
        return Ok(None);
    };
    let path = if first == "--" {
        arguments.next().context("请在 -- 后指定项目目录")?
    } else {
        ensure!(
            !first.to_string_lossy().starts_with('-'),
            "未知选项：{}；使用 --help 查看帮助",
            first.to_string_lossy()
        );
        first
    };
    ensure!(arguments.next().is_none(), "只能指定一个项目目录");
    let path = PathBuf::from(path);
    let canonical = path
        .canonicalize()
        .with_context(|| format!("无法打开项目目录：{}", path.display()))?;
    ensure!(canonical.is_dir(), "项目路径不是目录：{}", path.display());
    Ok(Some(canonical))
}

fn desktop_command(executable: &Path, project_path: Option<&Path>) -> Command {
    #[cfg(target_os = "macos")]
    let mut command = if let Ok(bundle) = update_installation::bundle_for_executable(executable) {
        let mut command = Command::new("/usr/bin/open");
        command.arg("-n").arg(bundle).arg("--args");
        command
    } else {
        Command::new(executable)
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = Command::new(executable);
    command.arg(APP_MODE_ARG);
    if let Some(path) = project_path {
        command.arg(path);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        unsafe extern "C" {
            fn setsid() -> i32;
        }
        // Only the async-signal-safe setsid syscall runs between fork and exec.
        unsafe {
            command.pre_exec(|| {
                if setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0000_0008 | 0x0000_0200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    }
    command
}

fn launch_desktop(executable: &Path, project_path: Option<&Path>) -> anyhow::Result<()> {
    let mut command = desktop_command(executable, project_path);
    #[cfg(target_os = "macos")]
    if command.get_program() == "/usr/bin/open" {
        // Wait for LaunchServices to accept the request, never for the app to exit.
        ensure!(
            command.status().context("打开 Nexus Agent")?.success(),
            "无法打开 Nexus Agent.app"
        );
        return Ok(());
    }
    command.spawn().context("打开 Nexus Agent")?;
    Ok(())
}

fn create_presenter(update_error: Option<String>) -> Presenter {
    let (storage, storage_error) = match Storage::open_default() {
        Ok(storage) => (storage, None),
        Err(error) => (
            Storage::open(Path::new(":memory:")).expect("open fallback database"),
            Some(format!(
                "无法打开本地数据库，历史记录仅在本次运行有效：{error}"
            )),
        ),
    };

    let runner = RunnerClient::spawn().map(|runner| Box::new(runner) as Box<dyn RunnerPort>);
    let mut presenter = Presenter::new(storage, runner, storage_error);
    presenter.scan_harness_installations();
    if let Some(error) = update_error {
        presenter.report_update_error(error);
    } else if presenter.model().updates.check_on_startup {
        presenter.check_for_updates();
    }
    presenter
}

pub(crate) fn run() -> anyhow::Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let argument = arguments.next();
    if argument
        .as_ref()
        .is_some_and(|arg| arg == update_installation::APPLY_UPDATE_ARG)
    {
        return update_installation::apply_from_args(arguments);
    }
    #[cfg(target_os = "windows")]
    if argument.as_ref().is_none_or(|arg| {
        arg != RUNNER_MODE_ARG
            && arg != APP_MODE_ARG
            && arg != update_installation::UPDATE_ERROR_ARG
    }) {
        // The GUI subsystem does not inherit a terminal console automatically.
        // Attach only for public CLI invocations; launching from Explorer stays silent.
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn AttachConsole(process_id: u32) -> i32;
        }
        unsafe {
            AttachConsole(u32::MAX);
        }
    }
    if argument
        .as_ref()
        .is_some_and(|arg| arg == "--version" || arg == "-V")
    {
        println!("{}", crate::model::updates::installed_tag());
        return Ok(());
    }
    if argument.as_ref().is_some_and(|arg| arg == RUNNER_MODE_ARG) {
        return nexus_runner::run();
    }
    if argument
        .as_ref()
        .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        println!(
            "用法：nexus-desktop [目录]\n\n  nexus-desktop .       打开当前目录的项目\n  nexus-desktop <目录>  打开指定目录的项目\n  nexus-desktop        启动桌面应用\n\n打开应用后立即返回终端，关闭终端不影响应用运行。\n\n  --help, -h           显示帮助\n  --version, -V        显示版本\n  -- <目录>           打开以 - 开头的目录"
        );
        return Ok(());
    }
    let (update_error, project_path) = if argument
        .as_ref()
        .is_some_and(|arg| arg == update_installation::UPDATE_ERROR_ARG)
    {
        (
            arguments
                .next()
                .map(|error| error.to_string_lossy().into_owned()),
            None,
        )
    } else if argument.as_ref().is_some_and(|arg| arg == APP_MODE_ARG) {
        (None, project_argument(arguments)?)
    } else {
        let project_path = project_argument(argument.into_iter().chain(arguments))?;
        let executable = std::env::current_exe().context("定位 Nexus Agent 可执行文件")?;
        return launch_desktop(&executable, project_path.as_deref());
    };

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .with_http_client(std::sync::Arc::new(
            reqwest_client::ReqwestClient::user_agent(concat!(
                "Nexus-Agent/",
                env!("CARGO_PKG_VERSION")
            ))?,
        ))
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            let font_errors = fonts::directory()
                .and_then(|directory| fonts::load(&directory, cx.text_system()))
                .unwrap_or_else(|error| vec![format!("{error:#}")]);
            theme::configure_theme(cx);
            let window_background = cx.global::<theme::ResolvedAppearance>().window_background();
            let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
            cx.spawn(async move |cx| {
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: (!cfg!(target_os = "macos")).then(|| "Nexus Agent".into()),
                        appears_transparent: cfg!(target_os = "macos"),
                        traffic_light_position: cfg!(target_os = "macos")
                            .then(|| point(px(18.), px(18.))),
                    }),
                    window_background,
                    window_min_size: Some(size(px(1_040.), px(680.))),
                    ..Default::default()
                };
                cx.open_window(options, |window, cx| {
                    let mut presenter = create_presenter(update_error);
                    if !font_errors.is_empty() {
                        presenter.report_font_load_error(font_errors.join("\n"));
                    }
                    if let Some(path) = project_path {
                        presenter.open_project(&path);
                    }
                    let view = cx.new(|cx| NexusView::new(presenter, window, cx));
                    cx.new(|cx| Root::new(view, window, cx).bg(rgba(0x00000000)))
                })?;
                Ok::<_, anyhow::Error>(())
            })
            .detach();
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn desktop_command_opens_the_app_bundle_and_forwards_the_project() {
        let executable = Path::new("/Applications/Nexus Agent.app/Contents/MacOS/nexus-desktop");
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("项目 with spaces");
        std::fs::create_dir(&project).unwrap();
        let project = project_argument([project.into_os_string()]).unwrap();
        for project in [None, project.as_deref()] {
            let command = desktop_command(executable, project);
            assert_eq!(command.get_program(), "/usr/bin/open");
            let mut expected: Vec<OsString> = [
                "-n",
                "/Applications/Nexus Agent.app",
                "--args",
                APP_MODE_ARG,
            ]
            .into_iter()
            .map(Into::into)
            .collect();
            if let Some(project) = project {
                expected.push(project.as_os_str().to_owned());
            }
            assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
        }
    }

    #[cfg(unix)]
    #[test]
    fn desktop_launch_returns_while_the_app_runs_without_terminal_streams_or_session() {
        use std::{
            fs,
            os::unix::fs::PermissionsExt as _,
            time::{Duration, Instant},
        };
        unsafe extern "C" {
            fn getsid(pid: i32) -> i32;
        }
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("my project's $目录");
        fs::create_dir(&project).unwrap();
        for (index, project) in [None, Some(project.as_path())].into_iter().enumerate() {
            let executable = directory.path().join(format!("fake app {index}"));
            let arguments = executable.with_extension("args");
            let pid_file = executable.with_extension("pid");
            fs::write(
                &executable,
                "#!/bin/sh\nset -eu\nfor fd in 0 1 2; do [ \"/dev/fd/$fd\" -ef /dev/null ]; done\nprintf '%s\\n' \"$@\" > \"$0.args\"\nprintf '%s\\n' \"$$\" > \"$0.pid\"\nexec /bin/sleep 5\n",
            )
            .unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
            launch_desktop(&executable, project).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let pid: i32 = loop {
                if let Ok(pid) = fs::read_to_string(&pid_file)
                    && let Ok(pid) = pid.trim().parse()
                {
                    break pid;
                }
                assert!(
                    Instant::now() < deadline,
                    "App did not start with detached stdio"
                );
                std::thread::sleep(Duration::from_millis(10));
            };
            // A live session leader proves the launcher did not wait for app exit.
            let session = unsafe { getsid(pid) };
            let _ = Command::new("/bin/kill").arg(pid.to_string()).status();
            assert_eq!(session, pid, "App must have its own session");
            let expected = match project {
                Some(project) => format!("{APP_MODE_ARG}\n{}\n", project.display()),
                None => format!("{APP_MODE_ARG}\n"),
            };
            assert_eq!(fs::read_to_string(&arguments).unwrap(), expected);
        }
    }

    #[test]
    fn desktop_launch_reports_a_missing_executable() {
        let directory = tempfile::tempdir().unwrap();
        assert!(launch_desktop(&directory.path().join("missing-app"), None).is_err());
    }

    #[test]
    fn project_arguments_preserve_plain_desktop_launch_and_resolve_current_directory() {
        assert_eq!(project_argument([]).unwrap(), None);
        assert_eq!(
            project_argument([".".into()]).unwrap(),
            Some(std::env::current_dir().unwrap().canonicalize().unwrap())
        );
    }

    #[test]
    fn project_arguments_open_explicit_directory_including_spaces() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("my project");
        std::fs::create_dir(&project).unwrap();
        assert_eq!(
            project_argument([project.into_os_string()]).unwrap(),
            Some(directory.path().join("my project").canonicalize().unwrap())
        );
    }

    #[test]
    fn project_arguments_reject_missing_paths_files_options_and_multiple_directories() {
        let directory = tempfile::tempdir().unwrap();
        let file = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
        assert!(project_argument([directory.path().join("missing").into_os_string()]).is_err());
        assert!(project_argument([file.path().as_os_str().to_owned()]).is_err());
        assert!(project_argument(["--unknown".into()]).is_err());
        assert!(project_argument(["--".into()]).is_err());
        assert!(project_argument([".".into(), ".".into()]).is_err());
        assert_eq!(
            project_argument(["--".into(), directory.path().as_os_str().to_owned()]).unwrap(),
            Some(directory.path().canonicalize().unwrap())
        );
    }
}
