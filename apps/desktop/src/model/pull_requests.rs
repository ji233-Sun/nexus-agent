use super::issues::{Comment, IssueFilter, IssueProvider};
use crate::i18n::LocalizedText;
use std::{collections::BTreeMap, fmt::Write as _};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub(crate) struct PullRequest {
    pub(crate) number: String,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) state: String,
    pub(crate) author: String,
    pub(crate) head_repository: String,
    pub(crate) head_branch: String,
    pub(crate) base_branch: String,
    pub(crate) head_sha: String,
    pub(crate) base_sha: String,
    pub(crate) draft: bool,
    pub(crate) merge_status: String,
    pub(crate) mergeable: bool,
}

impl PullRequest {
    pub(crate) fn can_merge(&self) -> bool {
        self.state == "open" && !self.draft && self.mergeable && !self.head_sha.is_empty()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Review {
    pub(crate) id: String,
    pub(crate) author: String,
    pub(crate) state: String,
    pub(crate) body: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ReviewThread {
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) line: Option<u64>,
    pub(crate) resolved: bool,
    pub(crate) outdated: bool,
    pub(crate) comments: Vec<Comment>,
}

#[derive(Clone, Debug)]
pub(crate) struct Check {
    pub(crate) name: String,
    pub(crate) state: String,
    pub(crate) description: String,
    pub(crate) url: String,
}

impl Check {
    pub(crate) fn failed(&self) -> bool {
        matches!(
            self.state.as_str(),
            "failure" | "error" | "timed_out" | "cancelled" | "action_required" | "startup_failure"
        )
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PullDetail {
    pub(crate) pull: PullRequest,
    pub(crate) comments: Vec<Comment>,
    pub(crate) reviews: Vec<Review>,
    pub(crate) threads: Vec<ReviewThread>,
    pub(crate) checks: Vec<Check>,
    pub(crate) stack: Vec<PullRequest>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PullRunKind {
    ResolveConflicts,
    Review,
    AddressReviews,
    FixCi,
    Stack,
}

impl PullRunKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ResolveConflicts => "处理 PR 冲突",
            Self::Review => "审查并评论 PR",
            Self::AddressReviews => "处理审查意见",
            Self::FixCi => "处理 CI 报错",
            Self::Stack => "处理 Stack PR",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    pub(crate) const ALL: [Self; 3] = [Self::Merge, Self::Squash, Self::Rebase];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Squash => "squash",
            Self::Rebase => "rebase",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PullAction {
    Merge(MergeMethod),
    Close,
}

impl PullAction {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Merge(_) => "合并 PR",
            Self::Close => "关闭 PR",
        }
    }
}

pub(crate) fn pull_url(provider: IssueProvider, repository: &str, number: &str) -> String {
    let path = if provider == IssueProvider::Cnb {
        "/-/pulls/"
    } else {
        "/pull/"
    };
    format!("{}{path}{number}", provider.repository_url(repository))
}

impl PullDetail {
    pub(crate) fn can_run(&self, provider: IssueProvider, kind: PullRunKind) -> bool {
        self.pull.state == "open"
            && match kind {
                PullRunKind::AddressReviews => {
                    !self.reviews.is_empty()
                        || !self.threads.is_empty()
                        || !self.comments.is_empty()
                }
                PullRunKind::FixCi => self.checks.iter().any(Check::failed),
                PullRunKind::Stack => provider == IssueProvider::GitHub && self.stack.len() > 1,
                _ => true,
            }
    }

    pub(crate) fn chat_prompt(
        &self,
        provider: IssueProvider,
        repository: &str,
        kind: PullRunKind,
        extra: &str,
    ) -> String {
        let pull = &self.pull;
        // GitHub uses /pull/, while CNB uses /-/pulls/.
        let url = pull_url(provider, repository, &pull.number);
        let mut prompt = format!(
            "请{}，并验证结果。\n\n# {} PR #{}：{}\n\n{}\n\n\
             - 仓库：{repository}\n- 状态：{}\n- 作者：{}\n\
             - 来源：{}:{}\n- 目标分支：{}\n- Head SHA：{}\n- Base SHA：{}\n\
             - 合并状态：{}\n\n## 描述\n\n{}\n",
            kind.label(),
            provider.name(),
            pull.number,
            pull.title,
            url,
            pull.state,
            pull.author,
            pull.head_repository,
            pull.head_branch,
            pull.base_branch,
            pull.head_sha,
            pull.base_sha,
            pull.merge_status,
            pull.body,
        );
        prompt.push_str("\n## 评论\n");
        append_comments(&mut prompt, &self.comments);
        prompt.push_str("\n## 审查结论\n");
        for review in &self.reviews {
            let _ = writeln!(
                prompt,
                "\n### Review {} · {} · {}\n\n{}",
                review.id, review.author, review.state, review.body
            );
        }
        prompt.push_str("\n## 审查 Conversations\n");
        for thread in &self.threads {
            let _ = writeln!(
                prompt,
                "\n### Conversation {} · {}:{} · resolved={} · outdated={}",
                thread.id,
                thread.path,
                thread.line.map(|line| line.to_string()).unwrap_or_default(),
                thread.resolved,
                thread.outdated
            );
            append_comments(&mut prompt, &thread.comments);
        }
        prompt.push_str("\n## 当前 Head 的 CI\n");
        for check in &self.checks {
            let _ = writeln!(
                prompt,
                "- {}：{} · {} · {}",
                check.name, check.state, check.description, check.url
            );
        }
        if !self.stack.is_empty() {
            prompt.push_str("\n## Stack PR（依赖顺序）\n");
            for item in &self.stack {
                let _ = writeln!(
                    prompt,
                    "- #{} {} · {}:{} → {} · head={} · {}",
                    item.number,
                    item.title,
                    item.head_repository,
                    item.head_branch,
                    item.base_branch,
                    item.head_sha,
                    pull_url(provider, repository, &item.number)
                );
            }
        }
        prompt.push_str("\n## 执行要求\n\n\
            - 使用已登录的本机 CLI，先重新读取 PR 的最新 head/base、评论、审查 conversations 和 CI。上面的信息是启动时快照。\n\
            - 先核对当前工作目录和 Git 状态，读取该 PR 的实际 diff。修改必须针对该 PR 的来源仓库和 head 分支；不要误改默认分支，不要丢弃已有未提交改动。\n");
        match kind {
            PullRunKind::Review => prompt.push_str(
                "- 目标是完成代码审查并将审查结论发布到这个 PR 下，不能只在聊天中回答。列出可定位的文件/行号、问题影响及验证依据；无问题也要发布结论。不要修改代码或合并 PR。\n"),
            PullRunKind::ResolveConflicts => prompt.push_str(
                "- 将最新目标分支合入 PR 的 head，解决全部冲突，运行相关验证，将修复提交推送到该 PR 来源分支，并在 PR 下评论结果。不要自动合并 PR。\n"),
            PullRunKind::AddressReviews => prompt.push_str(
                "- 处理所有作者（包括其他 Agent）提出的有效审查结论和未解决 conversations，保留完整讨论上下文，逐条验证、修复并回复；GitHub 中已解决的问题要 resolve 对应 review thread。将修复提交推送到该 PR 来源分支，不要自动合并 PR。\n"),
            PullRunKind::FixCi => prompt.push_str(
                "- 打开失败 CI 对应的运行并读取失败日志，确认日志属于当前 head；复现并修复根因，运行相关检查，将修复提交推送到 PR 来源分支，等待新 CI 并在 PR 下评论结果。不要以关闭或跳过检查替代修复。\n"),
            PullRunKind::Stack => prompt.push_str(
                "- 重新核对 Stack 的 base/head 依赖关系，按父 PR 到子 PR 的顺序处理冲突、审查意见和失败 CI；父分支变化后同步下游，保留各 PR 的改动边界，并分别提交、推送和评论。不要擅自合并整个 Stack 或强制推送。\n"),
        }
        match provider {
            IssueProvider::GitHub => {
                let _ = writeln!(
                    prompt,
                    "- 使用 gh pr view/diff/review/comment、gh api graphql（reviewThreads、回复与 resolveReviewThread）和 gh run view --log-failed，仓库明确指定 {repository}，PR 明确指定 {}。审查结论使用 gh pr review --comment --body-file 发布。",
                    pull.number
                );
            }
            IssueProvider::Cnb => {
                let _ = writeln!(
                    prompt,
                    "- 使用 cnb pulls get-pull/list-pull-files/list-pull-reviews/list-pull-review-comments/list-pull-commit-statuses，始终指定 --repo {repository} --number {}。审查结论使用 post-pull-review --event comment，讨论使用 post-pull-comment 或 post-pull-request-review-reply。CI 日志使用 cnb build 命令（先用 --help 确认参数）。长正文写入临时文件并用 @文件引用。",
                    pull.number
                );
            }
        }
        if !extra.trim().is_empty() {
            let _ = writeln!(prompt, "\n## 补充信息\n\n{}", extra.trim());
        }
        prompt
    }
}

fn append_comments(prompt: &mut String, comments: &[Comment]) {
    for comment in comments {
        let _ = writeln!(
            prompt,
            "\n#### 评论 {} · {} · {}\n\n{}",
            comment.id,
            comment.author.name(),
            comment.created_at,
            comment.body
        );
    }
}

pub(crate) struct PullPage {
    pub(crate) pulls: Vec<PullRequest>,
    pub(crate) total: usize,
    pub(crate) next_cursor: Option<String>,
}

pub(crate) struct PullRequestsModel {
    pub(crate) opened: bool,
    pub(crate) filter: IssueFilter,
    pub(crate) page: usize,
    pub(crate) page_cursors: BTreeMap<usize, String>,
    pub(crate) total: usize,
    pub(crate) pulls: Vec<PullRequest>,
    pub(crate) list_request: Option<Uuid>,
    pub(crate) list_error: Option<LocalizedText>,
    pub(crate) detail_number: Option<String>,
    pub(crate) detail: Option<Box<PullDetail>>,
    pub(crate) detail_request: Option<Uuid>,
    pub(crate) detail_error: Option<LocalizedText>,
    pub(crate) merge_method: MergeMethod,
    pub(crate) confirmation: Option<PullAction>,
    pub(crate) action_request: Option<(Uuid, PullAction)>,
    pub(crate) action_error: Option<LocalizedText>,
    pub(crate) action_success: Option<LocalizedText>,
}

impl Default for PullRequestsModel {
    fn default() -> Self {
        Self {
            opened: false,
            filter: IssueFilter::Open,
            page: 1,
            page_cursors: BTreeMap::new(),
            total: 0,
            pulls: Vec::new(),
            list_request: None,
            list_error: None,
            detail_number: None,
            detail: None,
            detail_request: None,
            detail_error: None,
            merge_method: MergeMethod::Squash,
            confirmation: None,
            action_request: None,
            action_error: None,
            action_success: None,
        }
    }
}

impl PullRequestsModel {
    pub(crate) fn clear_detail(&mut self) {
        self.detail_number = None;
        self.detail = None;
        self.detail_request = None;
        self.detail_error = None;
        self.confirmation = None;
        self.action_request = None;
        self.action_error = None;
        self.action_success = None;
    }
}
