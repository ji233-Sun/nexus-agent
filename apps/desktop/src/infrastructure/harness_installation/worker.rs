use super::*;

pub(super) async fn execute_request(
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
                    let http = reqwest_client::ReqwestClient::user_agent(concat!(
                        "Nexus-Agent/",
                        env!("CARGO_PKG_VERSION")
                    ))?;
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
                            let http = &http;
                            async move {
                                let (mut installation, latest) = tokio::join!(
                                    scan_one(harness, configured, environment, managers),
                                    latest_version(harness, http, &environment.cancel),
                                );
                                installation.latest_version = latest.map_err(|error| {
                                    LocalizedText::new(
                                        "最新版本检测失败：{error}",
                                        &[("error", format!("{error:#}"))],
                                    )
                                });
                                (harness, installation)
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

pub(super) async fn run_command(
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
