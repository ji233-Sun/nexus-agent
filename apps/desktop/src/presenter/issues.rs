use super::*;
use crate::{
    infrastructure::issues::{ActionResult, Event, Request, Response},
    model::issues::{IssueAction, IssueFilter, IssuesModel, PAGE_SIZE},
};

impl Presenter {
    pub(super) fn reset_issues_project(&mut self) {
        for provider in IssueProvider::ALL {
            self.reset_issue_provider(provider);
        }
    }

    pub(crate) fn set_issues_enabled(&mut self, provider: IssueProvider, enabled: bool) {
        if let Err(error) = self.storage.set_setting(
            &format!("{}_enabled", provider.key()),
            if enabled { "true" } else { "false" },
        ) {
            self.model.issues_mut(provider).detection_error = Some(error.to_string().into());
            return;
        }
        self.model.issues_mut(provider).enabled = enabled;
        self.reset_issue_provider(provider);
    }

    pub(super) fn reset_issue_provider(&mut self, provider: IssueProvider) {
        *self.model.issues_mut(provider) = IssuesModel {
            enabled: self.model.issues(provider).enabled,
            cli: self.model.issues(provider).cli.clone(),
            ..IssuesModel::default()
        };
        if self.model.issues(provider).enabled && self.model.selected_project.is_some() {
            self.inspect_issues(provider);
        }
    }

    pub(crate) fn inspect_issues(&mut self, provider: IssueProvider) {
        if self.model.issues(provider).detection_request.is_some() {
            return;
        }
        let id = Uuid::new_v4();
        let path = self
            .model
            .selected_project
            .as_ref()
            .map(|project| project.canonical_path.clone().into());
        self.model.issues_mut(provider).detection_error = None;
        match self
            .issues_client
            .request(id, provider, Request::Inspect(path))
        {
            Ok(()) => self.model.issues_mut(provider).detection_request = Some(id),
            Err(error) => {
                self.model.issues_mut(provider).detection_error = Some(error.to_string().into())
            }
        }
    }

    pub(crate) fn open_issues(&mut self, provider: IssueProvider) {
        if !self.model.issues(provider).enabled || self.model.issues(provider).repository.is_none()
        {
            return;
        }
        for other in IssueProvider::ALL {
            self.model.issues_mut(other).opened = other == provider;
        }
        if self.model.issues(provider).issues.is_empty()
            && self.model.issues(provider).list_request.is_none()
        {
            self.load_issues(provider, 1, self.model.issues(provider).filter);
        }
    }

    pub(crate) fn load_issues(
        &mut self,
        provider: IssueProvider,
        page: usize,
        filter: IssueFilter,
    ) {
        let issues = self.model.issues_mut(provider);
        if !issues.enabled || page == 0 || issues.list_request.is_some() {
            return;
        }
        let (Some(cli), Some(repository)) = (issues.cli.clone(), issues.repository.clone()) else {
            return;
        };
        let cursor = if page > 1 && provider == IssueProvider::GitHub {
            let Some(cursor) = issues.page_cursors.get(&page).cloned() else {
                return;
            };
            Some(cursor)
        } else {
            None
        };
        let id = Uuid::new_v4();
        if page == 1 || filter != issues.filter {
            issues.page_cursors.clear();
        }
        if page != issues.page || filter != issues.filter {
            issues.issues.clear();
            issues.total = 0;
        }
        issues.page = page;
        issues.filter = filter;
        issues.list_error = None;
        issues.clear_detail();
        match self.issues_client.request(
            id,
            provider,
            Request::List {
                cli,
                repository,
                page,
                filter,
                cursor,
            },
        ) {
            Ok(()) => issues.list_request = Some(id),
            Err(error) => issues.list_error = Some(error.to_string().into()),
        }
    }

    pub(crate) fn select_issue(&mut self, provider: IssueProvider, number: String) {
        let issues = self.model.issues_mut(provider);
        if !issues.enabled || !issues.issues.iter().any(|issue| issue.number == number) {
            return;
        }
        let (Some(cli), Some(repository)) = (issues.cli.clone(), issues.repository.clone()) else {
            return;
        };
        let id = Uuid::new_v4();
        issues.clear_detail();
        issues.detail_number = Some(number.clone());
        // Replacing the request id prevents an earlier selection from overwriting this detail.
        match self.issues_client.request(
            id,
            provider,
            Request::Detail {
                cli,
                repository,
                number,
            },
        ) {
            Ok(()) => issues.detail_request = Some(id),
            Err(error) => {
                issues.detail_request = None;
                issues.detail_error = Some(error.to_string().into());
            }
        }
        self.load_issue_comments(provider);
    }

    pub(crate) fn close_issue(&mut self, provider: IssueProvider) {
        let issues = self.model.issues_mut(provider);
        issues.clear_detail();
        if issues.list_dirty {
            let page = if provider == IssueProvider::GitHub {
                1
            } else {
                issues.page
            };
            let filter = issues.filter;
            self.load_issues(provider, page, filter);
        }
    }

    pub(crate) fn load_issue_comments(&mut self, provider: IssueProvider) {
        let issues = self.model.issues_mut(provider);
        if !issues.enabled || issues.comments_request.is_some() {
            return;
        }
        let (Some(cli), Some(repository), Some(number)) = (
            issues.cli.clone(),
            issues.repository.clone(),
            issues.detail_number.clone(),
        ) else {
            return;
        };
        let id = Uuid::new_v4();
        issues.comments = None;
        issues.comments_error = None;
        match self.issues_client.request(
            id,
            provider,
            Request::Comments {
                cli,
                repository,
                number,
            },
        ) {
            Ok(()) => issues.comments_request = Some(id),
            Err(error) => issues.comments_error = Some(error.to_string().into()),
        }
    }

    pub(crate) fn prepare_issue_chat(&mut self, provider: IssueProvider) -> Option<String> {
        let issues = self.model.issues(provider);
        if !issues.enabled
            || self.model.selected_project.is_none()
            || issues.detail_request.is_some()
            || issues.comments_request.is_some()
            || issues.action_request.is_some()
        {
            return None;
        }
        let prompt = issues.detail.as_ref()?.chat_prompt(
            provider,
            issues.repository.as_deref()?,
            issues.comments.as_deref()?,
        );
        self.new_task();
        self.model
            .log_status("Issue 已加入聊天草稿，请选择 Harness 后发送。".into());
        Some(prompt)
    }

    pub(crate) fn act_on_issue(&mut self, provider: IssueProvider, action: IssueAction) {
        let issues = self.model.issues_mut(provider);
        if !issues.enabled
            || issues.action_request.is_some()
            || issues.detail_request.is_some()
            || (action == IssueAction::StartNpc
                && (provider != IssueProvider::Cnb || issues.npc_comment.is_some()))
        {
            return;
        }
        let (Some(cli), Some(repository), Some(issue)) = (
            issues.cli.clone(),
            issues.repository.clone(),
            issues.detail.as_ref(),
        ) else {
            return;
        };
        let id = Uuid::new_v4();
        issues.action_error = None;
        issues.action_success = None;
        match self.issues_client.request(
            id,
            provider,
            Request::Action {
                cli,
                repository,
                number: issue.number.clone(),
                action,
            },
        ) {
            Ok(()) => {
                issues.action_request = Some((id, action));
                // A list requested before this mutation must not restore the old state.
                issues.list_request = None;
                issues.list_dirty = true;
            }
            Err(error) => issues.action_error = Some(error.to_string().into()),
        }
    }

    pub(crate) fn refresh_cnb_npc_action(&mut self) {
        let provider = IssueProvider::Cnb;
        let issues = self.model.issues_mut(provider);
        if !issues.enabled || issues.npc_request.is_some() {
            return;
        }
        let (Some(cli), Some(repository), Some(number), Some(comment)) = (
            issues.cli.clone(),
            issues.repository.clone(),
            issues.detail_number.clone(),
            issues.npc_comment.as_ref(),
        ) else {
            return;
        };
        let id = Uuid::new_v4();
        issues.npc_error = None;
        match self.issues_client.request(
            id,
            provider,
            Request::NpcAction {
                cli,
                repository,
                number,
                comment_id: comment.id.clone(),
            },
        ) {
            Ok(()) => issues.npc_request = Some(id),
            Err(error) => issues.npc_error = Some(error.to_string().into()),
        }
    }

    pub(crate) fn drain_issue_events(&mut self) -> bool {
        let events: Vec<_> = self.issues_client.events.try_iter().collect();
        let changed = !events.is_empty();
        for event in events {
            self.handle_issue_event(event);
        }
        changed
    }

    pub(super) fn handle_issue_event(&mut self, event: Event) {
        let provider = event.provider;
        let issues = self.model.issues_mut(provider);
        match event.response {
            Response::Inspection { repository, cli }
                if issues.detection_request == Some(event.id) =>
            {
                issues.detection_request = None;
                if issues.repository != repository {
                    *issues = IssuesModel {
                        enabled: issues.enabled,
                        ..IssuesModel::default()
                    };
                }
                issues.repository = repository;
                match cli {
                    Ok(cli) => {
                        issues.cli = Some(cli);
                        issues.detection_error = None;
                    }
                    Err(error) => {
                        issues.cli = None;
                        issues.detection_error = Some(error);
                    }
                }
                if issues.opened && issues.cli.is_some() && issues.issues.is_empty() {
                    let page = issues.page;
                    let filter = issues.filter;
                    self.load_issues(provider, page, filter);
                }
            }
            Response::List(result) if issues.list_request == Some(event.id) => {
                issues.list_request = None;
                match result {
                    Ok(page) => {
                        issues.page_cursors.retain(|page, _| *page <= issues.page);
                        if let Some(cursor) = page.next_cursor {
                            issues.page_cursors.insert(issues.page + 1, cursor);
                        }
                        issues.issues = page.issues;
                        issues.total = page.total;
                        issues.list_dirty = false;
                        if issues.page > 1 && (issues.page - 1) * PAGE_SIZE >= issues.total {
                            let page = if provider == IssueProvider::GitHub {
                                1
                            } else {
                                issues.total.div_ceil(PAGE_SIZE).max(1)
                            };
                            let filter = issues.filter;
                            self.load_issues(provider, page, filter);
                        }
                    }
                    Err(error) => issues.list_error = Some(error),
                }
            }
            Response::Detail(result) if issues.detail_request == Some(event.id) => {
                issues.detail_request = None;
                match result {
                    Ok(issue) if issues.detail_number.as_deref() == Some(&issue.number) => {
                        issues.detail = Some(issue)
                    }
                    Ok(_) => {
                        issues.detail_error = Some(LocalizedText::new(
                            "{provider} 返回了不同的 Issue，请重试。",
                            &[("provider", provider.name().into())],
                        ))
                    }
                    Err(error) => issues.detail_error = Some(error),
                }
            }
            Response::Comments(result) if issues.comments_request == Some(event.id) => {
                issues.comments_request = None;
                match result {
                    Ok(comments) => {
                        if let Some(issue) = &mut issues.detail {
                            issue.comment_count = comments.len() as u64;
                        }
                        issues.comments = Some(comments);
                    }
                    Err(error) => issues.comments_error = Some(error),
                }
            }
            Response::Action(result)
                if issues.action_request.is_some_and(|(id, _)| id == event.id) =>
            {
                let (_, action) = issues.action_request.take().unwrap();
                match result {
                    Ok(ActionResult::Updated(issue))
                        if issues.detail_number.as_deref() == Some(&issue.number) =>
                    {
                        if let Some(index) = issues
                            .issues
                            .iter()
                            .position(|existing| existing.number == issue.number)
                        {
                            if issue.state == issues.filter.state() {
                                issues.issues[index] = *issue.clone();
                            } else {
                                issues.issues.remove(index);
                                issues.total = issues.total.saturating_sub(1);
                            }
                        } else if issue.state == issues.filter.state() {
                            issues.issues.insert(0, *issue.clone());
                            issues.issues.truncate(PAGE_SIZE);
                            issues.total += 1;
                        }
                        issues.detail = Some(*issue);
                        issues.action_success = Some(
                            match action {
                                IssueAction::AssignSelf => "已指派给当前用户。",
                                IssueAction::SetState(IssueFilter::Closed) => "Issue 已关闭。",
                                _ => "Issue 已重新打开。",
                            }
                            .into(),
                        );
                    }
                    Ok(ActionResult::Updated(_)) => {
                        issues.action_error = Some(LocalizedText::new(
                            "{provider} 返回了不同的 Issue，请重试。",
                            &[("provider", provider.name().into())],
                        ))
                    }
                    Ok(ActionResult::Npc(comment)) => {
                        let has_action = comment.action_url().is_some();
                        issues.npc_comment = Some(comment);
                        issues.action_success = Some("已发送 CodeBuddy NPC 处理请求。".into());
                        // Invalidate a comments read started before the newly created comment.
                        issues.comments_request = None;
                        self.load_issue_comments(provider);
                        if !has_action {
                            self.refresh_cnb_npc_action();
                        }
                    }
                    Err(error) => issues.action_error = Some(error),
                }
            }
            Response::NpcAction(result) if issues.npc_request == Some(event.id) => {
                issues.npc_request = None;
                match result {
                    Ok(comment)
                        if issues
                            .npc_comment
                            .as_ref()
                            .is_some_and(|current| current.id == comment.id) =>
                    {
                        if comment.action_url().is_none() {
                            issues.npc_error = Some(match comment.npc_failure() {
                                Some(error) => LocalizedText::new(
                                    "NPC 未启动：{error}",
                                    &[("error", error.to_owned())],
                                ),
                                None => "NPC 请求已发送，Action 链接尚未生成，请刷新状态。".into(),
                            });
                        }
                        if let Some(comments) = &mut issues.comments
                            && let Some(existing) = comments
                                .iter_mut()
                                .find(|existing| existing.id == comment.id)
                        {
                            *existing = comment.clone();
                        }
                        issues.npc_comment = Some(comment);
                    }
                    Ok(_) => issues.npc_error = Some("CNB 返回了不同的评论，请刷新状态。".into()),
                    Err(error) => issues.npc_error = Some(error),
                }
            }
            _ => {}
        }
    }
}
