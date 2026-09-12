use crate::i18n::LocalizedText;
use serde::Deserialize;
use std::{fmt::Write as _, path::PathBuf};
use uuid::Uuid;

pub(crate) const PAGE_SIZE: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IssueProvider {
    Cnb,
    GitHub,
}

impl IssueProvider {
    pub(crate) const ALL: [Self; 2] = [Self::Cnb, Self::GitHub];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Cnb => "cnb",
            Self::GitHub => "github",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Cnb => "CNB",
            Self::GitHub => "GitHub",
        }
    }

    pub(crate) fn host(self) -> &'static str {
        match self {
            Self::Cnb => "cnb.cool",
            Self::GitHub => "github.com",
        }
    }

    pub(crate) fn executable(self) -> &'static str {
        match self {
            Self::Cnb => "cnb",
            Self::GitHub => "gh",
        }
    }

    pub(crate) fn login_command(self) -> &'static str {
        match self {
            Self::Cnb => "cnb login",
            Self::GitHub => "gh auth login --hostname github.com",
        }
    }

    pub(crate) fn documentation(self) -> &'static str {
        match self {
            Self::Cnb => "https://docs.cnb.cool/en/develops/cnb-cli.html",
            Self::GitHub => "https://cli.github.com/manual/",
        }
    }

    pub(crate) fn repository_url(self, repository: &str) -> String {
        format!("https://{}/{repository}", self.host())
    }

    pub(crate) fn issue_url(self, repository: &str, number: &str) -> String {
        let separator = if self == Self::Cnb { "/-" } else { "" };
        format!(
            "{}{separator}/issues/{number}",
            self.repository_url(repository)
        )
    }
}

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

impl Issue {
    pub(crate) fn chat_prompt(
        &self,
        provider: IssueProvider,
        repository: &str,
        comments: &[Comment],
    ) -> String {
        let mut prompt = format!(
            "请处理以下 {} Issue，并验证结果。\n\n# {}\n\n{}\n\n",
            provider.name(),
            self.title,
            provider.issue_url(repository, &self.number)
        );
        for (label, value) in [
            ("仓库", repository.to_owned()),
            ("状态", self.state.clone()),
            (
                "作者",
                format!("{} (@{})", self.author.name(), self.author.username),
            ),
            (
                "处理人",
                self.assignees
                    .iter()
                    .map(|user| user.username.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            (
                "标签",
                self.labels
                    .iter()
                    .map(|label| label.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            ("优先级", self.priority.clone()),
            ("创建时间", self.created_at.clone()),
            ("更新时间", self.updated_at.clone()),
        ] {
            let _ = writeln!(prompt, "- {label}：{value}");
        }
        let _ = write!(
            prompt,
            "\n## 描述\n\n{}\n\n## 评论（{}）\n",
            self.body,
            comments.len()
        );
        for comment in comments {
            let _ = write!(
                prompt,
                "\n### {} (@{}) · {} · 评论 {}\n\n{}\n",
                comment.author.name(),
                comment.author.username,
                comment.created_at,
                comment.id,
                comment.body
            );
        }
        prompt
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Comment {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) body: String,
    #[serde(default)]
    pub(crate) author: User,
    #[serde(default)]
    pub(crate) created_at: String,
    #[serde(default)]
    pub(crate) statuses: Option<CommentStatuses>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct CommentStatuses {
    #[serde(default)]
    npc: Vec<NpcStatusGroup>,
}

#[derive(Clone, Debug, Deserialize)]
struct NpcStatusGroup {
    #[serde(default)]
    statuses: Vec<NpcStatus>,
}

#[derive(Clone, Debug, Deserialize)]
struct NpcStatus {
    #[serde(default)]
    target_url: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    description: String,
}

impl Comment {
    pub(crate) fn action_url(&self) -> Option<&str> {
        self.statuses
            .as_ref()?
            .npc
            .iter()
            .flat_map(|group| &group.statuses)
            .map(|status| status.target_url.as_str())
            .find(|url| url.starts_with("https://cnb.cool/"))
    }

    pub(crate) fn npc_failure(&self) -> Option<&str> {
        self.statuses
            .as_ref()?
            .npc
            .iter()
            .flat_map(|group| &group.statuses)
            .find(|status| matches!(status.state.as_str(), "failure" | "error" | "skipped"))
            .map(|status| {
                if status.description.is_empty() {
                    status.state.as_str()
                } else {
                    status.description.as_str()
                }
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IssueAction {
    AssignSelf,
    SetState(IssueFilter),
    StartNpc,
}

#[derive(Debug)]
pub(crate) struct IssuePage {
    pub(crate) issues: Vec<Issue>,
    pub(crate) total: usize,
    pub(crate) next_cursor: Option<String>,
}

pub(crate) struct IssuesModel {
    pub(crate) enabled: bool,
    pub(crate) opened: bool,
    pub(crate) cli: Option<Cli>,
    pub(crate) repository: Option<String>,
    pub(crate) detection_request: Option<Uuid>,
    pub(crate) detection_error: Option<LocalizedText>,
    pub(crate) filter: IssueFilter,
    pub(crate) page: usize,
    pub(crate) page_cursors: std::collections::BTreeMap<usize, String>,
    pub(crate) total: usize,
    pub(crate) issues: Vec<Issue>,
    pub(crate) list_request: Option<Uuid>,
    pub(crate) list_error: Option<LocalizedText>,
    pub(crate) detail_number: Option<String>,
    pub(crate) detail: Option<Issue>,
    pub(crate) detail_request: Option<Uuid>,
    pub(crate) detail_error: Option<LocalizedText>,
    pub(crate) comments: Option<Vec<Comment>>,
    pub(crate) comments_request: Option<Uuid>,
    pub(crate) comments_error: Option<LocalizedText>,
    pub(crate) action_request: Option<(Uuid, IssueAction)>,
    pub(crate) action_error: Option<LocalizedText>,
    pub(crate) action_success: Option<LocalizedText>,
    pub(crate) list_dirty: bool,
    pub(crate) npc_comment: Option<Comment>,
    pub(crate) npc_request: Option<Uuid>,
    pub(crate) npc_error: Option<LocalizedText>,
}

impl Default for IssuesModel {
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
            page_cursors: Default::default(),
            total: 0,
            issues: Vec::new(),
            list_request: None,
            list_error: None,
            detail_number: None,
            detail: None,
            detail_request: None,
            detail_error: None,
            comments: None,
            comments_request: None,
            comments_error: None,
            action_request: None,
            action_error: None,
            action_success: None,
            list_dirty: false,
            npc_comment: None,
            npc_request: None,
            npc_error: None,
        }
    }
}

impl IssuesModel {
    pub(crate) fn clear_detail(&mut self) {
        self.detail_number = None;
        self.detail = None;
        self.detail_request = None;
        self.detail_error = None;
        self.comments = None;
        self.comments_request = None;
        self.comments_error = None;
        self.action_request = None;
        self.action_error = None;
        self.action_success = None;
        self.npc_comment = None;
        self.npc_request = None;
        self.npc_error = None;
    }
}
