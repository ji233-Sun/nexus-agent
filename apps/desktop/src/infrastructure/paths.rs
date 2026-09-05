use std::{env, ffi::OsString, path::PathBuf};

use anyhow::{Result, anyhow};

pub(super) fn data_directory() -> Result<PathBuf> {
    data_directory_for(env::consts::OS, |key| env::var_os(key))
}

fn data_directory_for(
    os: &str,
    mut variable: impl FnMut(&str) -> Option<OsString>,
) -> Result<PathBuf> {
    let mut path = |key| {
        variable(key)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let directory = match os {
        "macos" => path("HOME").map(|home| home.join("Library/Application Support/Nexus Agent")),
        "linux" => path("XDG_DATA_HOME")
            .filter(|directory| directory.is_absolute())
            .or_else(|| path("HOME").map(|home| home.join(".local/share")))
            .map(|base| base.join("nexus-agent")),
        "windows" => path("LOCALAPPDATA")
            .or_else(|| path("USERPROFILE").map(|home| home.join("AppData/Local")))
            .map(|base| base.join("Nexus Agent")),
        _ => return Err(anyhow!("不支持的操作系统：{os}")),
    };
    directory.ok_or_else(|| anyhow!("无法确定应用数据目录"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(os: &str, values: &[(&str, PathBuf)]) -> Result<PathBuf> {
        data_directory_for(os, |key| {
            values
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.as_os_str().to_owned())
        })
    }

    #[test]
    fn uses_native_data_directories_on_all_three_platforms() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let data = directory.path().join("data");
        let values = [
            ("HOME", home.clone()),
            ("USERPROFILE", home.clone()),
            ("XDG_DATA_HOME", data.clone()),
            ("LOCALAPPDATA", data.clone()),
        ];
        assert_eq!(
            resolve("macos", &values).unwrap(),
            home.join("Library/Application Support/Nexus Agent")
        );
        assert_eq!(resolve("linux", &values).unwrap(), data.join("nexus-agent"));
        assert_eq!(
            resolve("windows", &values).unwrap(),
            data.join("Nexus Agent")
        );
    }

    #[test]
    fn falls_back_to_home_and_ignores_relative_xdg_paths() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().to_path_buf();
        let values = [
            ("HOME", home.clone()),
            ("USERPROFILE", home.clone()),
            ("XDG_DATA_HOME", PathBuf::from("relative")),
        ];
        assert_eq!(
            resolve("linux", &values).unwrap(),
            home.join(".local/share/nexus-agent")
        );
        assert_eq!(
            resolve("windows", &values).unwrap(),
            home.join("AppData/Local/Nexus Agent")
        );
        assert!(resolve("macos", &[]).is_err());
        assert!(resolve("linux", &[]).is_err());
        assert!(resolve("windows", &[]).is_err());
    }
}
