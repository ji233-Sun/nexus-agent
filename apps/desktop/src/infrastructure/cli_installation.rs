use std::env;

use anyhow::{Context as _, Result, ensure};

#[cfg(unix)]
use std::{
    fs,
    path::{Path, PathBuf},
};

#[cfg(unix)]
const PATH_START: &str = "# >>> Nexus Agent CLI >>>";
#[cfg(unix)]
const PATH_END: &str = "# <<< Nexus Agent CLI <<<";

pub(crate) fn install() -> Result<()> {
    let executable = env::current_exe().context("定位 Nexus Agent 可执行文件")?;
    let directory = executable.parent().context("CLI 没有父目录")?;
    #[cfg(unix)]
    {
        let user_directory = env::var_os("HOME").context("无法确定用户目录")?;
        let shell = env::var_os("SHELL").unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "/bin/zsh"
            } else {
                "/bin/bash"
            }
            .into()
        });
        let zsh_directory = env::var_os("ZDOTDIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let profiles = shell_profiles(
            Path::new(&user_directory),
            Path::new(&shell),
            zsh_directory.as_deref(),
        )?;
        install_unix(directory, &profiles)
    }
    #[cfg(target_os = "windows")]
    {
        ensure!(
            !directory.as_os_str().to_string_lossy().contains(';'),
            "应用路径包含 PATH 分隔符，请移动应用后重试"
        );
        let output = super::update_installation::powershell()
            .env("NEXUS_CLI_BIN", directory)
            .arg(include_str!("install_cli_windows.ps1"))
            .output()
            .context("配置用户 PATH")?;
        ensure!(
            output.status.success(),
            "无法配置用户 PATH：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(())
    }
}

#[cfg(unix)]
fn shell_profiles(
    user_directory: &Path,
    shell: &Path,
    zsh_directory: Option<&Path>,
) -> Result<Vec<PathBuf>> {
    ensure!(user_directory.is_absolute(), "用户目录必须是绝对路径");
    match shell.file_name().and_then(|name| name.to_str()) {
        Some("zsh") => {
            let directory = zsh_directory.unwrap_or(user_directory);
            ensure!(directory.is_absolute(), "ZDOTDIR 必须是绝对路径");
            Ok(vec![directory.join(".zshrc")])
        }
        Some("bash") => {
            let login = [".bash_profile", ".bash_login", ".profile"]
                .into_iter()
                .map(|name| user_directory.join(name))
                .find(|path| path.exists())
                .unwrap_or_else(|| user_directory.join(".profile"));
            Ok(vec![user_directory.join(".bashrc"), login])
        }
        Some("sh" | "dash") => Ok(vec![user_directory.join(".profile")]),
        _ => anyhow::bail!(
            "暂不支持自动配置 {} 的 PATH；支持 zsh、bash 和 sh。",
            shell.display()
        ),
    }
}

#[cfg(unix)]
fn profile_with_cli(content: &str, directory: &Path) -> Result<String> {
    ensure!(directory.is_absolute(), "应用目录必须是绝对路径");
    let directory = directory.to_str().context("应用路径不是有效的 Unicode")?;
    ensure!(
        !directory.contains(':'),
        "应用路径包含 PATH 分隔符，请移动应用后重试"
    );
    let quoted = format!("'{}'", directory.replace('\'', "'\\''"));
    let block = format!(
        "{PATH_START}\ncase \":$PATH:\" in *:{quoted}:*) ;; *) export PATH={quoted}:\"$PATH\" ;; esac\n{PATH_END}\n"
    );
    if let Some(start) = content.find(PATH_START) {
        ensure!(
            content.matches(PATH_START).count() == 1 && content.matches(PATH_END).count() == 1,
            "CLI PATH 配置段不完整，请检查 Shell 配置"
        );
        let end = content
            .find(PATH_END)
            .context("CLI PATH 配置段缺少结束标记")?;
        ensure!(end > start, "CLI PATH 配置段的标记顺序不正确");
        ensure!(
            (start == 0 || content.as_bytes()[start - 1] == b'\n')
                && content.as_bytes()[end - 1] == b'\n'
                && (content[start + PATH_START.len()..].starts_with('\n')
                    || content[start + PATH_START.len()..].starts_with("\r\n"))
                && (content[end + PATH_END.len()..].is_empty()
                    || content[end + PATH_END.len()..].starts_with('\n')
                    || content[end + PATH_END.len()..].starts_with("\r\n")),
            "CLI PATH 配置标记必须独占一行"
        );
        let mut end = end + PATH_END.len();
        if content[end..].starts_with("\r\n") {
            end += 2;
        } else if content[end..].starts_with('\n') {
            end += 1;
        }
        let mut updated = content.to_owned();
        updated.replace_range(start..end, &block);
        Ok(updated)
    } else {
        ensure!(!content.contains(PATH_END), "CLI PATH 配置段缺少开始标记");
        let separator = if content.is_empty() || content.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        Ok(format!("{content}{separator}{block}"))
    }
}

#[cfg(unix)]
fn install_unix(directory: &Path, profiles: &[PathBuf]) -> Result<()> {
    // Prepare every edit before writing, and replace only our marked PATH block.
    let mut changes = Vec::new();
    for profile in profiles {
        let content = match fs::read_to_string(profile) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("读取 Shell 配置：{}", profile.display()));
            }
        };
        let updated = profile_with_cli(&content, directory)?;
        if updated != content {
            changes.push((profile, updated));
        }
    }
    for (profile, updated) in changes {
        fs::create_dir_all(profile.parent().context("Shell 配置没有父目录")?)
            .context("创建 Shell 配置目录")?;
        fs::write(profile, updated)
            .with_context(|| format!("写入 Shell 配置：{}", profile.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::*;

    #[cfg(unix)]
    #[test]
    fn cli_installation_exposes_the_native_command_and_preserves_arguments() {
        use std::{os::unix::fs::PermissionsExt as _, process::Command};
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("Nexus Agent's $目录");
        fs::create_dir(&directory).unwrap();
        let executable = directory.join("nexus-desktop");
        fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let profile = temporary.path().join(".zshrc");
        fs::write(
            &profile,
            "# existing setup\nexport NEXUS_CLI_TEST=preserved",
        )
        .unwrap();
        install_unix(&directory, std::slice::from_ref(&profile)).unwrap();
        let output = Command::new("/bin/sh")
            .env("PATH", "/usr/bin:/bin")
            .args(["-c", ". \"$1\"; . \"$1\"; nexus-desktop . '项目 空格'; printf '%s\\n' \"$PATH\" \"$NEXUS_CLI_TEST\"", "cli-test"])
            .arg(&profile)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!(
                ".\n项目 空格\n{}:/usr/bin:/bin\npreserved\n",
                directory.display()
            )
        );
        #[cfg(target_os = "macos")]
        {
            let output = Command::new("/bin/zsh")
                .env("PATH", "/usr/bin:/bin")
                .env("ZDOTDIR", temporary.path())
                .args(["-d", "-i", "-c", "nexus-desktop . '项目 空格'"])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(String::from_utf8(output.stdout).unwrap(), ".\n项目 空格\n");
        }
    }

    #[cfg(unix)]
    #[test]
    fn cli_installation_is_repeatable_and_updates_only_its_own_configuration() {
        let temporary = tempfile::tempdir().unwrap();
        let profile = temporary.path().join(".bashrc");
        let original = "# user configuration\nexport EDITOR=vim\n";
        fs::write(&profile, original).unwrap();
        let first = temporary.path().join("first app");
        install_unix(&first, std::slice::from_ref(&profile)).unwrap();
        let installed = fs::read_to_string(&profile).unwrap();
        install_unix(&first, std::slice::from_ref(&profile)).unwrap();
        assert_eq!(fs::read_to_string(&profile).unwrap(), installed);
        fs::write(&profile, format!("{installed}# added afterwards\n")).unwrap();
        let moved = temporary.path().join("moved app");
        install_unix(&moved, std::slice::from_ref(&profile)).unwrap();
        let updated = fs::read_to_string(&profile).unwrap();
        assert!(updated.starts_with(original));
        assert!(updated.ends_with("# added afterwards\n"));
        assert_eq!(updated.matches(PATH_START).count(), 1);
        assert!(!updated.contains(first.to_str().unwrap()));
        assert!(updated.contains(moved.to_str().unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn cli_installation_rejects_invalid_configuration_before_writing_profiles() {
        let temporary = tempfile::tempdir().unwrap();
        let first = temporary.path().join(".bashrc");
        let second = temporary.path().join(".profile");
        fs::write(&first, "# unchanged\n").unwrap();
        let broken = format!("{PATH_START}\n# incomplete\n");
        fs::write(&second, &broken).unwrap();
        assert!(install_unix(temporary.path(), &[first.clone(), second.clone()]).is_err());
        assert_eq!(fs::read_to_string(&first).unwrap(), "# unchanged\n");
        assert_eq!(fs::read_to_string(&second).unwrap(), broken);
        assert!(profile_with_cli("", &temporary.path().join("invalid:path")).is_err());
        let malformed = format!("{PATH_START}\n{PATH_END} user text\n");
        assert!(profile_with_cli(&malformed, temporary.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn cli_installation_uses_shell_startup_files_and_respects_zdotdir() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path();
        let zsh_directory = directory.join("zsh-config");
        assert_eq!(
            shell_profiles(directory, Path::new("/bin/zsh"), Some(&zsh_directory)).unwrap(),
            vec![zsh_directory.join(".zshrc")]
        );
        assert_eq!(
            shell_profiles(directory, Path::new("/bin/bash"), None).unwrap(),
            vec![directory.join(".bashrc"), directory.join(".profile")]
        );
        fs::write(directory.join(".bash_profile"), "# login").unwrap();
        assert_eq!(
            shell_profiles(directory, Path::new("/bin/bash"), None).unwrap(),
            vec![directory.join(".bashrc"), directory.join(".bash_profile")]
        );
        assert!(shell_profiles(directory, Path::new("/bin/unsupported"), None).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn cli_installation_windows_script_parses_without_changing_the_registry() {
        let output = super::super::update_installation::powershell()
            .env("NEXUS_CLI_TEST_SCRIPT", include_str!("install_cli_windows.ps1"))
            .arg("$errors = $null; $tokens = $null; [void][System.Management.Automation.Language.Parser]::ParseInput($env:NEXUS_CLI_TEST_SCRIPT, [ref]$tokens, [ref]$errors); if ($errors.Count) { $errors | Out-String | Write-Error; exit 1 }")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
