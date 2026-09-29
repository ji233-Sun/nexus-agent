#[cfg(unix)]
use super::worker::execute_request;
use super::*;
use crate::i18n::Language;
use gpui_kit::http_client::{FakeHttpClient, RequestTimeout, Response};

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
    use std::io::Write as _;

    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut file = fs::File::create(path).unwrap();
    file.lock().unwrap();
    file.write_all(content.as_bytes()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    drop(file);

    // A concurrent fork can inherit the writable handle. Wait for every copy
    // to close before executing the script, avoiding ETXTBSY on Linux.
    fs::File::open(path).unwrap().lock_shared().unwrap();
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
        ("1.18.29\n", "1.18.29"),
    ] {
        assert_eq!(installed_version(output).as_deref(), Some(version));
    }
    assert!(installed_version("installation successful").is_none());
    assert!(installed_version("").is_none());
}

#[test]
fn roadmap_installers_use_official_packages_and_kimi_uses_its_own_installer() {
    let (_directory, _cancel, mut environment) = fixture();
    environment.tools.insert("bash".into(), "/bin/bash".into());
    environment
        .tools
        .insert("powershell".into(), "powershell.exe".into());
    let npm = manager(&environment, InstallMethod::Npm, "npm", "bin", "prefix");
    for (harness, package) in [
        (HarnessKind::Pi, "@earendil-works/pi-coding-agent@latest"),
        (HarnessKind::Qoder, "@qoder-ai/qodercli@latest"),
        (HarnessKind::QoderCn, "@qodercn-ai/qoderclicn@latest"),
        (HarnessKind::Codebuddy, "@tencent-ai/codebuddy-code@latest"),
    ] {
        let options = install_options(harness, &environment, std::slice::from_ref(&npm));
        assert_eq!(options.len(), 1);
        assert!(options[0].command.args.contains(&package.into()));
    }
    // Kimi Code ships a standalone installer; npm must not be offered.
    let options = install_options(HarnessKind::Kimi, &environment, &[npm]);
    assert_eq!(options.len(), 1);
    assert_eq!(options[0].method, InstallMethod::Native);
    let command = options[0].command.args.last().unwrap();
    assert!(command.contains(if cfg!(windows) {
        "code.kimi.com/kimi-code/install.ps1"
    } else {
        "code.kimi.com/kimi-code/install.sh"
    }));
}

#[test]
fn opencode_uses_its_unix_installer_and_the_npm_package_on_windows() {
    let (_directory, _cancel, mut environment) = fixture();
    environment.tools.insert("bash".into(), "/bin/bash".into());
    let npm = manager(&environment, InstallMethod::Npm, "npm", "bin", "prefix");
    let options = install_options(
        HarnessKind::Opencode,
        &environment,
        std::slice::from_ref(&npm),
    );
    let native = options
        .iter()
        .find(|option| option.method == InstallMethod::Native);
    match native {
        // OpenCode 只有 Unix 安装脚本，Windows 上必须回退到包管理器。
        None => assert_eq!(env::consts::OS, "windows"),
        Some(option) => assert!(
            option
                .command
                .args
                .last()
                .unwrap()
                .contains("opencode.ai/install")
        ),
    }
    assert!(
        options
            .iter()
            .any(|option| option.command.args.contains(&"opencode-ai@latest".into()))
    );
}

#[tokio::test]
async fn latest_version_reads_the_latest_tag_for_each_harness() {
    let (_cancel, cancellation) = watch::channel(false);
    for (harness, path, version) in [
        (
            HarnessKind::Claude,
            "/@anthropic-ai%2Fclaude-code/latest",
            "2.1.263",
        ),
        (HarnessKind::Codex, "/@openai%2Fcodex/latest", "0.153.4"),
        (HarnessKind::Qoder, "/@qoder-ai%2Fqodercli/latest", "1.1.48"),
        (
            HarnessKind::QoderCn,
            "/@qodercn-ai%2Fqoderclicn/latest",
            "1.1.48",
        ),
        (
            HarnessKind::Codebuddy,
            "/@tencent-ai%2Fcodebuddy-code/latest",
            "2.147.0",
        ),
        (HarnessKind::Opencode, "/opencode-ai/latest", "1.18.31"),
        (HarnessKind::Kimi, "/kimi-code/latest", "0.42.0"),
        (
            HarnessKind::Pi,
            "/@earendil-works%2Fpi-coding-agent/latest",
            "0.85.1",
        ),
        (
            HarnessKind::Omp,
            "/@oh-my-pi%2Fpi-coding-agent/latest",
            "18.1.14",
        ),
    ] {
        let http = FakeHttpClient::create(move |request| async move {
            assert_eq!(
                request.uri().host(),
                Some(if harness == HarnessKind::Kimi {
                    "code.kimi.com"
                } else {
                    "registry.npmjs.org"
                })
            );
            assert_eq!(request.uri().path(), path);
            assert_eq!(
                request.extensions().get::<RequestTimeout>().unwrap().0,
                PROBE_TIMEOUT
            );
            Ok(Response::builder().body(if harness == HarnessKind::Kimi {
                format!("{version}\n").into()
            } else {
                serde_json::json!({"version":version}).to_string().into()
            })?)
        });
        assert_eq!(
            latest_version(harness, http.as_ref(), &cancellation)
                .await
                .unwrap(),
            version
        );
    }
}

#[tokio::test]
async fn latest_version_rejects_failed_or_invalid_registry_responses() {
    let (_cancel, cancellation) = watch::channel(false);
    for (status, body) in [
        (503, "unavailable".into()),
        (429, "rate limited".into()),
        (200, "not JSON".into()),
        (200, "{}".into()),
        (200, r#"{"version":"invalid"}"#.into()),
        (200, r#"{"version":123}"#.into()),
        (200, "x".repeat(MAX_OUTPUT + 1)),
    ] {
        let http = FakeHttpClient::create(move |_| {
            let body = body.clone();
            async move { Ok(Response::builder().status(status).body(body.into())?) }
        });
        assert!(
            latest_version(HarnessKind::Claude, http.as_ref(), &cancellation)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn latest_version_cancels_an_unresponsive_registry_request() {
    let (cancel, cancellation) = watch::channel(false);
    let http = FakeHttpClient::create(move |_| {
        let cancel = cancel.clone();
        async move {
            cancel.send(true).unwrap();
            std::future::pending().await
        }
    });
    let error = tokio::time::timeout(
        Duration::from_secs(1),
        latest_version(HarnessKind::Codex, http.as_ref(), &cancellation),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(error.to_string().contains("取消"));
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
    let (owner, command) = ownership(HarnessKind::Omp, &entry, &real, &environment, &[bun]).await;
    assert_eq!(source(&owner), "Bun");
    let command = command.unwrap();
    assert_eq!(
        command.args,
        ["add", "-g", "--trust", "@oh-my-pi/pi-coding-agent@latest"]
    );
    assert_eq!(
        Path::new(&command.environment["BUN_INSTALL_GLOBAL_DIR"]),
        environment.home.join("custom-global")
    );
    assert_eq!(
        Path::new(&command.environment["BUN_INSTALL_BIN"]),
        environment.home.join("custom-bin")
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
        (
            HarnessKind::Kimi,
            ".kimi-code/bin/kimi",
            ".kimi-code/bin/kimi",
        ),
        (
            HarnessKind::Opencode,
            ".opencode/bin/opencode",
            ".opencode/bin/opencode",
        ),
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
        } else if matches!(harness, HarnessKind::Kimi | HarnessKind::Opencode) {
            assert_eq!(command.args, ["upgrade"]);
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
    let installation = scan_one(HarnessKind::Codex, "codex".into(), &environment, &managers).await;
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
    // A loaded CI runner can transiently fail to spawn the child or make
    // it exit before the probe elapses, so retry before accepting failure.
    let mut timed_out = false;
    for _ in 0..10 {
        match run_command(&spec, &environment, Duration::from_millis(80)).await {
            Err(error) if error.to_string().contains("超时") => {
                timed_out = true;
                break;
            }
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    assert!(timed_out, "命令应在 80ms 超时");
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
