use std::{path::PathBuf, sync::Arc};

use serde::Deserialize;

use crate::i18n::{Language, LocalizedText};

pub(crate) fn installed_tag() -> &'static str {
    env!("NEXUS_RELEASE_TAG")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateChannel {
    Release,
    Nightly,
}

impl UpdateChannel {
    pub(crate) fn from_setting(value: &str) -> Option<Self> {
        match value {
            "release" => Some(Self::Release),
            "nightly" => Some(Self::Nightly),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Nightly => "nightly",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Release => "Release",
            Self::Nightly => "Nightly",
        }
    }
}

impl Default for UpdateChannel {
    fn default() -> Self {
        if installed_tag().starts_with("nightly-") {
            Self::Nightly
        } else {
            Self::Release
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct UpdateAsset {
    pub(crate) name: String,
    pub(crate) browser_download_url: String,
    pub(crate) size: u64,
    pub(crate) digest: Option<String>,
}

#[derive(Debug)]
pub(crate) struct UpdatePackage {
    pub(crate) tag: String,
    pub(crate) notes: String,
    pub(crate) asset: UpdateAsset,
}

impl UpdatePackage {
    pub(crate) fn release_url(&self) -> String {
        format!(
            "https://github.com/ji233-Sun/nexus-agent/releases/tag/{}",
            self.tag
        )
    }
}

#[derive(Debug, Default)]
pub(crate) enum UpdateState {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Arc<UpdatePackage>),
    Downloading {
        package: Arc<UpdatePackage>,
        received: u64,
    },
    Ready {
        package: Arc<UpdatePackage>,
        path: PathBuf,
    },
    Installing(Arc<UpdatePackage>),
    Restarting(Arc<UpdatePackage>),
    Failed(LocalizedText),
}

impl UpdateState {
    pub(crate) fn is_busy(&self) -> bool {
        self.has_worker() || matches!(self, Self::Ready { .. } | Self::Restarting(_))
    }

    pub(crate) fn has_worker(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading { .. } | Self::Installing(_)
        )
    }

    pub(crate) fn is_installing(&self) -> bool {
        matches!(self, Self::Installing(_) | Self::Restarting(_))
    }

    pub(crate) fn package(&self) -> Option<&UpdatePackage> {
        match self {
            Self::Available(package)
            | Self::Downloading { package, .. }
            | Self::Ready { package, .. }
            | Self::Installing(package)
            | Self::Restarting(package) => Some(package),
            _ => None,
        }
    }

    pub(crate) fn message(&self, language: Language) -> String {
        match self {
            Self::Idle => language.text("尚未检查更新").into(),
            Self::Checking => language.text("正在检查更新…").into(),
            Self::UpToDate => language.text("此频道暂无可用更新。").into(),
            Self::Available(package) => language.format(
                "发现新版本 {tag}，点击更新后下载并自动安装。",
                &[("tag", package.tag.clone())],
            ),
            Self::Downloading { package, received } => language.format(
                "正在下载 {tag}：{progress}%",
                &[
                    ("tag", package.tag.clone()),
                    (
                        "progress",
                        (received.saturating_mul(100) / package.asset.size.max(1)).to_string(),
                    ),
                ],
            ),
            Self::Ready { package, .. } => language.format(
                "{tag} 已下载并通过校验，当前任务结束后将自动安装并重启。",
                &[("tag", package.tag.clone())],
            ),
            Self::Installing(package) => {
                language.format("正在准备安装 {tag}…", &[("tag", package.tag.clone())])
            }
            Self::Restarting(package) => {
                language.format("正在重启并安装 {tag}…", &[("tag", package.tag.clone())])
            }
            Self::Failed(error) => error.render(language).into(),
        }
    }
}

pub(crate) struct UpdateModel {
    pub(crate) channel: UpdateChannel,
    pub(crate) check_on_startup: bool,
    pub(crate) state: UpdateState,
}

impl Default for UpdateModel {
    fn default() -> Self {
        Self {
            channel: UpdateChannel::default(),
            check_on_startup: true,
            state: UpdateState::Idle,
        }
    }
}
