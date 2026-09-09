use std::{
    ffi::OsString,
    fs,
    io::{BufRead as _, BufReader, Write as _},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

#[cfg(not(target_os = "windows"))]
use std::time::Instant;

use anyhow::{Context as _, Result, bail, ensure};

use crate::model::updates::UpdatePackage;

pub(crate) const APPLY_UPDATE_ARG: &str = "--nexus-apply-update";
pub(crate) const UPDATE_ERROR_ARG: &str = "--nexus-update-error";

struct StagingDirectory {
    path: PathBuf,
    handed_off: bool,
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if !self.handed_off {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[derive(Debug)]
struct Replacement {
    source: PathBuf,
    target: PathBuf,
    backup: PathBuf,
}

fn bundle_for_executable(executable: &Path) -> Result<&Path> {
    let macos = executable
        .parent()
        .context("Missing executable directory")?;
    let contents = macos
        .parent()
        .context("Missing application Contents directory")?;
    let bundle = contents.parent().context("Missing application bundle")?;
    ensure!(
        macos.file_name().is_some_and(|name| name == "MacOS")
            && contents.file_name().is_some_and(|name| name == "Contents")
            && bundle
                .extension()
                .is_some_and(|extension| extension == "app"),
        "Automatic installation requires running the extracted Nexus Agent.app"
    );
    Ok(bundle)
}

fn install_parent<'a>(os: &str, executable: &'a Path) -> Result<&'a Path> {
    let target = if os == "macos" {
        bundle_for_executable(executable)?
    } else {
        executable
    };
    target.parent().context("Missing installation directory")
}

fn replacements(
    os: &str,
    executable: &Path,
    staging: &Path,
    root: &str,
) -> Result<Vec<Replacement>> {
    let unpacked = staging.join("unpacked").join(root);
    let pairs = if os == "macos" {
        vec![(
            unpacked.join("Nexus Agent.app"),
            bundle_for_executable(executable)?.to_path_buf(),
        )]
    } else {
        let suffix = if os == "windows" { ".exe" } else { "" };
        let runner = format!("nexus-runner{suffix}");
        vec![
            (
                unpacked.join(format!("nexus-desktop{suffix}")),
                executable.to_path_buf(),
            ),
            (
                unpacked.join(&runner),
                install_parent(os, executable)?.join(runner),
            ),
        ]
    };
    Ok(pairs
        .into_iter()
        .enumerate()
        .map(|(index, (source, target))| Replacement {
            source,
            target,
            backup: staging.join(format!("backup-{index}")),
        })
        .collect())
}

#[cfg(not(target_os = "windows"))]
fn tar() -> Command {
    #[cfg(target_os = "macos")]
    let executable = PathBuf::from("/usr/bin/tar");
    #[cfg(target_os = "linux")]
    let executable = PathBuf::from("tar");
    let mut command = Command::new(executable);
    hide_console(&mut command);
    command
}

#[cfg(target_os = "windows")]
pub(super) fn powershell() -> Command {
    let executable =
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut command = Command::new(executable);
    hide_console(&mut command);
    command.args(["-NoProfile", "-NonInteractive", "-Command"]);
    command
}

fn hide_console(command: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    #[cfg(not(target_os = "windows"))]
    let _ = command;
}

fn checked_output(command: &mut Command) -> Result<String> {
    let output = command
        .output()
        .with_context(|| format!("Run {:?}", command.get_program()))?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8(output.stdout).context("Invalid archive listing")
}

#[cfg(any(not(target_os = "windows"), test))]
fn validate_archive_listing(listing: &str, details: &str, root: &str) -> Result<()> {
    ensure!(!listing.trim().is_empty(), "The update archive is empty");
    for name in listing.lines() {
        let name = name.replace('\\', "/");
        let name = name.trim_end_matches('/');
        ensure!(
            !name.starts_with('/')
                && !name.contains(':')
                && name
                    .split('/')
                    .all(|part| !part.is_empty() && part != "." && part != "..")
                && (name == root
                    || name.starts_with(&format!("{root}/"))
                    || name == "__MACOSX"
                    || name == format!("__MACOSX/{root}")
                    || name.starts_with(&format!("__MACOSX/{root}/"))),
            "Unexpected path in update archive: {name}"
        );
    }
    // Published packages contain only regular files and directories, including macOS metadata.
    ensure!(
        details.lines().count() == listing.lines().count()
            && details
                .lines()
                .all(|line| line.starts_with('-') || line.starts_with('d')),
        "Links and special files are not allowed in update archives"
    );
    Ok(())
}

fn unpack(archive: &Path, staging: &Path, root: &str) -> Result<()> {
    #[cfg(not(target_os = "windows"))]
    {
        let listing = checked_output(tar().arg("-tf").arg(archive))?;
        let details = checked_output(tar().arg("-tvf").arg(archive))?;
        validate_archive_listing(&listing, &details, root)?;
    }
    let destination = staging.join("unpacked");
    fs::create_dir(&destination)?;
    #[cfg(target_os = "macos")]
    checked_output(
        Command::new("/usr/bin/ditto")
            .args(["-x", "-k"])
            .arg(archive)
            .arg(&destination),
    )?;
    #[cfg(target_os = "linux")]
    checked_output(tar().arg("-xf").arg(archive).arg("-C").arg(&destination))?;
    #[cfg(target_os = "windows")]
    checked_output(
        powershell()
            .arg(include_str!("update_windows.ps1"))
            .env("NEXUS_UPDATE_ARCHIVE", archive)
            .env("NEXUS_UPDATE_DESTINATION", &destination)
            .env("NEXUS_UPDATE_ROOT", root),
    )?;
    Ok(())
}

fn validate_sources(os: &str, executable: &Path, staging: &Path, root: &str) -> Result<()> {
    for replacement in replacements(os, executable, staging, root)? {
        let expected = if os == "macos" {
            replacement
                .source
                .join("Contents/MacOS")
                .join(executable.file_name().unwrap())
        } else {
            replacement.source
        };
        ensure!(
            fs::symlink_metadata(&expected)?.file_type().is_file(),
            "Missing program in update package: {}",
            expected.display()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            ensure!(
                fs::metadata(&expected)?.permissions().mode() & 0o111 != 0,
                "Update program is not executable"
            );
        }
    }
    Ok(())
}

pub(crate) fn prepare_and_launch(package: &UpdatePackage, archive: &Path) -> Result<()> {
    let digest = package
        .asset
        .digest
        .as_deref()
        .and_then(|value| value.strip_prefix("sha256:"))
        .context("Missing update checksum")?;
    ensure!(
        super::updates::cached_package_matches(archive, package.asset.size, digest)?,
        "The downloaded update has changed; check for updates and download it again"
    );
    let executable = std::env::current_exe()?.canonicalize()?;
    let os = std::env::consts::OS;
    let parent = install_parent(os, &executable)?;
    let staging_path = parent.join(format!(".nexus-update-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&staging_path).with_context(|| format!("Cannot write to the installation directory {}. Move the app to a writable location before updating", parent.display()))?;
    let mut staging = StagingDirectory {
        path: staging_path,
        handed_off: false,
    };
    let root = package
        .asset
        .name
        .strip_suffix(".tar.gz")
        .or_else(|| package.asset.name.strip_suffix(".zip"))
        .context("Unsupported update archive")?;
    unpack(archive, &staging.path, root)?;
    validate_sources(os, &executable, &staging.path, root)?;

    let directory = super::paths::data_directory()?.join("updates");
    fs::create_dir_all(&directory)?;
    let helper = directory.join(format!("update-helper{}", std::env::consts::EXE_SUFFIX));
    fs::copy(&executable, &helper).context("Prepare the update helper")?;
    let log = fs::File::create(directory.join("installation.log"))?;
    let mut command = Command::new(helper);
    hide_console(&mut command);
    let mut child = command
        .arg(APPLY_UPDATE_ARG)
        .arg(std::process::id().to_string())
        .arg(&staging.path)
        .arg(&executable)
        .arg(root)
        .current_dir(&directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(log)
        .spawn()
        .context("Start the update helper")?;
    let stdout = child.stdout.take().context("Read update helper status")?;
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout).read_line(&mut line).map(|_| line);
        let _ = send.send(result);
    });
    if !matches!(receive.recv_timeout(Duration::from_secs(15)), Ok(Ok(line)) if line.trim() == "ready")
    {
        let _ = child.kill();
        let _ = child.wait();
        bail!(
            "The update helper did not start. See {}",
            directory.join("installation.log").display()
        );
    }
    staging.handed_off = true;
    Ok(())
}

fn wait_for_parent(parent: u32) -> Result<()> {
    ensure!(
        parent > 1 && parent != std::process::id(),
        "Invalid update parent process"
    );
    #[cfg(target_os = "windows")]
    {
        checked_output(powershell()
            .arg(format!("$p = Get-Process -Id {parent} -ErrorAction SilentlyContinue; if ($p) {{ $p | Wait-Process -Timeout 60 -ErrorAction Stop }}; exit 0")))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Command::new("/bin/kill")
            .args(["-0", &parent.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?
            .success()
        {
            ensure!(
                Instant::now() < deadline,
                "Application did not exit within 60 seconds"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Ok(())
}

fn replace_and_restart(
    replacements: &[Replacement],
    restart: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let mut changed = Vec::new();
    let result = (|| {
        for replacement in replacements {
            ensure!(
                !replacement.backup.try_exists()?,
                "An update backup already exists: {}",
                replacement.backup.display()
            );
            let backed_up = replacement.target.try_exists()?;
            if backed_up {
                fs::rename(&replacement.target, &replacement.backup)?;
            }
            changed.push((replacement, backed_up, false));
            fs::rename(&replacement.source, &replacement.target)?;
            changed.last_mut().unwrap().2 = true;
        }
        restart()
    })();
    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        for (replacement, backed_up, installed) in changed.into_iter().rev() {
            if installed && let Err(error) = fs::rename(&replacement.target, &replacement.source) {
                rollback_errors.push(error.to_string());
                continue;
            }
            if backed_up && let Err(error) = fs::rename(&replacement.backup, &replacement.target) {
                rollback_errors.push(error.to_string());
            }
        }
        ensure!(
            rollback_errors.is_empty(),
            "{error:#}; restore failed: {}",
            rollback_errors.join("; ")
        );
        return Err(error.context("Update failed; the previous installation was restored"));
    }
    Ok(())
}

fn restart(executable: &Path, error: Option<&str>) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("/usr/bin/open");
        command
            .arg("-n")
            .arg(bundle_for_executable(executable)?)
            .arg("--args");
        command
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = Command::new(executable);
    if let Some(error) = error {
        command.arg(UPDATE_ERROR_ARG).arg(error);
    }
    command
        .current_dir(
            executable
                .parent()
                .context("Missing executable directory")?,
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "macos")]
    ensure!(
        command.status()?.success(),
        "Could not reopen the application"
    );
    #[cfg(not(target_os = "macos"))]
    command
        .spawn()
        .context("Could not restart the application")?;
    Ok(())
}

pub(crate) fn apply_from_args(arguments: impl IntoIterator<Item = OsString>) -> Result<()> {
    let arguments: Vec<_> = arguments.into_iter().collect();
    ensure!(arguments.len() == 4, "Invalid update helper arguments");
    let parent: u32 = arguments[0]
        .to_str()
        .context("Invalid parent process")?
        .parse()?;
    let staging = PathBuf::from(&arguments[1]);
    let executable = PathBuf::from(&arguments[2]);
    let root = arguments[3].to_str().context("Invalid package directory")?;
    let os = std::env::consts::OS;
    ensure!(
        parent > 1 && parent != std::process::id(),
        "Invalid update parent process"
    );
    ensure!(
        staging.is_absolute() && executable.is_absolute(),
        "Update paths must be absolute"
    );
    ensure!(
        staging.parent() == Some(install_parent(os, &executable)?),
        "Update staging must be beside the installation"
    );
    ensure!(
        fs::symlink_metadata(&staging)?.file_type().is_dir(),
        "Invalid update staging directory"
    );
    ensure!(
        staging
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix(".nexus-update-"))
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()),
        "Invalid update staging directory"
    );
    ensure!(
        root.starts_with("nexus-agent-")
            && root
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".-+".contains(&byte)),
        "Invalid update package directory"
    );
    validate_sources(os, &executable, &staging, root)?;
    println!("ready");
    std::io::stdout().flush()?;
    wait_for_parent(parent)?;
    let replacements = replacements(os, &executable, &staging, root)?;
    match replace_and_restart(&replacements, || restart(&executable, None)) {
        Ok(()) => {
            let _ = fs::remove_dir_all(staging);
            Ok(())
        }
        Err(error) => {
            let message = format!("{error:#}. Installation files: {}", staging.display());
            eprintln!("{message}");
            restart(&executable, Some(&message))?;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_the_mac_bundle_or_paired_binaries_without_replacing_the_parent_directory() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path();
        let staging = parent.join("staging");
        let executable = parent.join("Renamed Nexus.app/Contents/MacOS/nexus-desktop");
        let mac = replacements("macos", &executable, &staging, "package").unwrap();
        assert_eq!(mac.len(), 1);
        assert_eq!(mac[0].target, parent.join("Renamed Nexus.app"));
        assert_eq!(
            mac[0].source,
            staging.join("unpacked/package/Nexus Agent.app")
        );
        assert!(install_parent("macos", &parent.join("nexus-desktop")).is_err());
        for (os, suffix) in [("windows", ".exe"), ("linux", "")] {
            let executable = parent.join(format!("nexus-desktop{suffix}"));
            let entries = replacements(os, &executable, &staging, "package").unwrap();
            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].target, executable);
            assert_eq!(
                entries[1].target,
                parent.join(format!("nexus-runner{suffix}"))
            );
        }
    }

    #[test]
    fn archives_reject_traversal_other_roots_and_links_before_extraction() {
        validate_archive_listing(
            "package/\npackage/Nexus Agent.app/Contents/MacOS/nexus-desktop\n__MACOSX/package/._README.md\n",
            "drwxr-xr-x directory\n-rwxr-xr-x executable\n-rw-r--r-- metadata\n",
            "package",
        ).unwrap();
        for name in [
            "/package/file",
            "package/../outside",
            "package/./file",
            "package//file",
            "C:/package/file",
            "other/file",
            "__MACOSX/../file",
            "package\\..\\outside",
        ] {
            assert!(
                validate_archive_listing(name, "-rw-r--r-- file", "package").is_err(),
                "{name}"
            );
        }
        for details in [
            "lrwxr-xr-x link",
            "hrw-r--r-- hardlink",
            "prw-r--r-- pipe",
            "",
        ] {
            assert!(validate_archive_listing("package/file", details, "package").is_err());
        }
    }

    fn replacement_fixture(parent: &Path) -> Vec<Replacement> {
        ["desktop", "runner"]
            .into_iter()
            .map(|name| {
                let entry = Replacement {
                    source: parent.join(format!("new-{name}")),
                    target: parent.join(name),
                    backup: parent.join(format!("old-{name}")),
                };
                fs::write(&entry.source, format!("new {name}")).unwrap();
                fs::write(&entry.target, format!("old {name}")).unwrap();
                entry
            })
            .collect()
    }

    #[test]
    fn installs_both_binaries_before_restart_and_preserves_unrelated_files() {
        let directory = tempfile::tempdir().unwrap();
        let entries = replacement_fixture(directory.path());
        let data = directory.path().join("user-data.sqlite");
        fs::write(&data, "user data").unwrap();
        replace_and_restart(&entries, || {
            assert_eq!(fs::read(&entries[0].target).unwrap(), b"new desktop");
            assert_eq!(fs::read(&entries[1].target).unwrap(), b"new runner");
            Ok(())
        })
        .unwrap();
        assert_eq!(fs::read(&data).unwrap(), b"user data");
        assert_eq!(fs::read(&entries[0].backup).unwrap(), b"old desktop");
    }

    #[test]
    fn a_partial_replacement_or_failed_restart_restores_both_binaries() {
        for fail_restart in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let entries = replacement_fixture(directory.path());
            if !fail_restart {
                fs::remove_file(&entries[1].source).unwrap();
            }
            assert!(
                replace_and_restart(&entries, || {
                    assert!(
                        fail_restart,
                        "must not restart after a failed file replacement"
                    );
                    bail!("restart failed")
                })
                .is_err()
            );
            assert_eq!(fs::read(&entries[0].target).unwrap(), b"old desktop");
            assert_eq!(fs::read(&entries[1].target).unwrap(), b"old runner");
            assert_eq!(fs::read(&entries[0].source).unwrap(), b"new desktop");
        }
    }

    #[test]
    fn directory_replacement_rolls_back_the_complete_mac_bundle() {
        let directory = tempfile::tempdir().unwrap();
        let entry = Replacement {
            source: directory.path().join("new.app"),
            target: directory.path().join("Nexus Agent.app"),
            backup: directory.path().join("old.app"),
        };
        for (path, version) in [(&entry.source, "new"), (&entry.target, "old")] {
            fs::create_dir(path).unwrap();
            fs::write(path.join("version"), version).unwrap();
        }
        let entries = [entry];
        assert!(replace_and_restart(&entries, || bail!("restart failed")).is_err());
        assert_eq!(fs::read(entries[0].target.join("version")).unwrap(), b"old");
    }

    #[test]
    fn preparation_cleans_staging_unless_it_has_been_handed_to_the_helper() {
        let directory = tempfile::tempdir().unwrap();
        for handed_off in [false, true] {
            let path = directory.path().join(handed_off.to_string());
            fs::create_dir(&path).unwrap();
            drop(StagingDirectory {
                path: path.clone(),
                handed_off,
            });
            assert_eq!(path.exists(), handed_off);
        }
    }

    #[test]
    fn extracts_and_validates_the_native_published_archive_layout() {
        let directory = tempfile::tempdir().unwrap();
        let base = directory
            .path()
            .canonicalize()
            .unwrap()
            .join("安装 location");
        fs::create_dir(&base).unwrap();
        let root = "nexus-agent-test";
        let published = base.join("published").join(root);
        let staging = base.join("staging");
        let os = std::env::consts::OS;
        let executable = if os == "macos" {
            base.join("Nexus Agent.app/Contents/MacOS/nexus-desktop")
        } else {
            base.join(format!("nexus-desktop{}", std::env::consts::EXE_SUFFIX))
        };
        let programs = if os == "macos" {
            vec![published.join("Nexus Agent.app/Contents/MacOS/nexus-desktop")]
        } else {
            ["nexus-desktop", "nexus-runner"]
                .into_iter()
                .map(|name| published.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
                .collect()
        };
        for program in programs {
            fs::create_dir_all(program.parent().unwrap()).unwrap();
            fs::write(&program, "program").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let archive = base.join(if os == "linux" {
            "update.tar.gz"
        } else {
            "update.zip"
        });
        #[cfg(target_os = "macos")]
        checked_output(
            Command::new("/usr/bin/ditto")
                .args(["-c", "-k", "--sequesterRsrc", "--keepParent"])
                .arg(&published)
                .arg(&archive),
        )
        .unwrap();
        #[cfg(target_os = "linux")]
        checked_output(
            tar()
                .arg("-czf")
                .arg(&archive)
                .arg("-C")
                .arg(published.parent().unwrap())
                .arg(root),
        )
        .unwrap();
        #[cfg(target_os = "windows")]
        checked_output(powershell()
            .arg("$ErrorActionPreference = 'Stop'; Add-Type -AssemblyName System.IO.Compression.FileSystem; [System.IO.Compression.ZipFile]::CreateFromDirectory($env:NEXUS_UPDATE_SOURCE, $env:NEXUS_UPDATE_ARCHIVE, [System.IO.Compression.CompressionLevel]::Optimal, $true)")
            .env("NEXUS_UPDATE_SOURCE", &published)
            .env("NEXUS_UPDATE_ARCHIVE", &archive)).unwrap();
        fs::create_dir(&staging).unwrap();
        unpack(&archive, &staging, root).unwrap();
        validate_sources(os, &executable, &staging, root).unwrap();
        #[cfg(target_os = "windows")]
        {
            let invalid_archive = base.join("invalid.zip");
            checked_output(powershell()
                .arg("$ErrorActionPreference = 'Stop'; Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem; $zip = [System.IO.Compression.ZipFile]::Open($env:NEXUS_UPDATE_ARCHIVE, [System.IO.Compression.ZipArchiveMode]::Create); try { [void]$zip.CreateEntry('../outside') } finally { $zip.Dispose() }")
                .env("NEXUS_UPDATE_ARCHIVE", &invalid_archive)).unwrap();
            let invalid_staging = base.join("invalid-staging");
            fs::create_dir(&invalid_staging).unwrap();
            assert!(unpack(&invalid_archive, &invalid_staging, root).is_err());
            assert!(!base.join("outside").exists());
        }
    }
}
