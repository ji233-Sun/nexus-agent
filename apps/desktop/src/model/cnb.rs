use crate::i18n::LocalizedText;
use serde::Deserialize;
use std::path::PathBuf;
use uuid::Uuid;

pub(crate) const PAGE_SIZE: usize = 30;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum IssueFilter {
    #[default]
    Open,
    Closed,
}

impl IssueFilter {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Open => "未关闭",
            Self::Closed => "已关闭",
        }
    }

    pub(crate) fn state(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Cli {
    pub(crate) path: PathBuf,
    pub(crate) version: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct User {
    pub(crate) username: String,
    pub(crate) nickname: String,
}

impl User {
    pub(crate) fn name(&self) -> &str {
        if self.nickname.is_empty() {
            &self.username
        } else {
            &self.nickname
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Label {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) color: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Issue {
    pub(crate) number: String,
    pub(crate) title: String,
    pub(crate) state: String,
    #[serde(default)]
    pub(crate) body: String,
    #[serde(default)]
    pub(crate) author: User,
    #[serde(default)]
    pub(crate) assignees: Vec<User>,
    #[serde(default)]
    pub(crate) labels: Vec<Label>,
    #[serde(default)]
    pub(crate) priority: String,
    #[serde(default)]
    pub(crate) comment_count: u64,
    #[serde(default)]
    pub(crate) created_at: String,
    #[serde(default)]
    pub(crate) updated_at: String,
}

#[derive(Debug)]
pub(crate) struct IssuePage {
    pub(crate) issues: Vec<Issue>,
    pub(crate) total: usize,
}

pub(crate) struct CnbModel {
    pub(crate) enabled: bool,
    pub(crate) opened: bool,
    pub(crate) cli: Option<Cli>,
    pub(crate) repository: Option<String>,
    pub(crate) detection_request: Option<Uuid>,
    pub(crate) detection_error: Option<LocalizedText>,
    pub(crate) filter: IssueFilter,
    pub(crate) page: usize,
    pub(crate) total: usize,
    pub(crate) issues: Vec<Issue>,
    pub(crate) list_request: Option<Uuid>,
    pub(crate) list_error: Option<LocalizedText>,
    pub(crate) detail_number: Option<String>,
    pub(crate) detail: Option<Issue>,
    pub(crate) detail_request: Option<Uuid>,
    pub(crate) detail_error: Option<LocalizedText>,
}

impl Default for CnbModel {
    fn default() -> Self {
        Self {
            enabled: true,
            opened: false,
            cli: None,
            repository: None,
            detection_request: None,
            detection_error: None,
            filter: IssueFilter::Open,
            page: 1,
            total: 0,
            issues: Vec::new(),
            list_request: None,
            list_error: None,
            detail_number: None,
            detail: None,
            detail_request: None,
            detail_error: None,
        }
    }
}
