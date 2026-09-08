use std::{collections::BTreeMap, path::PathBuf};

use nexus_domain::HarnessKind;

use crate::i18n::LocalizedText;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InstallMethod {
    Native,
    VitePlus,
    Bun,
    Pnpm,
    Yarn,
    Npm,
    Homebrew,
    WinGet,
}

impl InstallMethod {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Native => "官方安装器",
            Self::VitePlus => "Vite+ (vp)",
            Self::Bun => "Bun",
            Self::Pnpm => "pnpm",
            Self::Yarn => "Yarn",
            Self::Npm => "npm",
            Self::Homebrew => "Homebrew",
            Self::WinGet => "WinGet",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MaintenanceCommand {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) environment: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub(crate) struct InstallOption {
    pub(crate) method: InstallMethod,
    pub(crate) command: MaintenanceCommand,
}

#[derive(Clone, Debug)]
pub(crate) struct HarnessInstallation {
    pub(crate) configured: String,
    pub(crate) executable: Option<PathBuf>,
    pub(crate) discovered_from_manager: bool,
    pub(crate) version: Option<String>,
    pub(crate) source: LocalizedText,
    pub(crate) diagnostic: Option<LocalizedText>,
    pub(crate) update: Option<MaintenanceCommand>,
    pub(crate) install_options: Vec<InstallOption>,
}

#[derive(Clone, Debug)]
pub(crate) struct MaintenanceRequest {
    pub(crate) harness: HarnessKind,
    pub(crate) configured: String,
    pub(crate) executable: Option<PathBuf>,
    pub(crate) method: Option<InstallMethod>,
    pub(crate) command: MaintenanceCommand,
}

#[derive(Default)]
pub(crate) struct HarnessManager {
    pub(crate) installations: BTreeMap<HarnessKind, HarnessInstallation>,
    pub(crate) busy: bool,
    pub(crate) operating: Option<HarnessKind>,
    pub(crate) message: Option<LocalizedText>,
}
