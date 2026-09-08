use std::path::PathBuf;

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

#[derive(Debug, Default)]
pub(crate) enum UpdateState {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Downloading {
        tag: String,
        received: u64,
        total: u64,
    },
    Ready {
        tag: String,
        path: PathBuf,
    },
    Failed(LocalizedText),
}

impl UpdateState {
    pub(crate) fn is_busy(&self) -> bool {
        matches!(self, Self::Checking | Self::Downloading { .. })
    }

    pub(crate) fn message(&self, language: Language) -> String {
        match self {
            Self::Idle => language.text("尚未检查更新").into(),
            Self::Checking => language.text("正在检查更新…").into(),
            Self::UpToDate => language.text("此频道暂无可用更新。").into(),
            Self::Downloading {
                tag,
                received,
                total,
            } => language.format(
                "正在下载 {tag}：{progress}%",
                &[
                    ("tag", tag.clone()),
                    (
                        "progress",
                        (received.saturating_mul(100) / (*total).max(1)).to_string(),
                    ),
                ],
            ),
            Self::Ready { tag, .. } => language.format(
                "{tag} 已下载并通过校验。解压后退出应用，再替换安装。",
                &[("tag", tag.clone())],
            ),
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
