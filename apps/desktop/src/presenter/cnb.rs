use super::*;
use crate::{
    infrastructure::cnb::{ActionResult, Event, Request, Response},
    model::cnb::{CnbModel, IssueAction, IssueFilter, PAGE_SIZE},
};

impl Presenter {
    pub(crate) fn set_cnb_enabled(&mut self, enabled: bool) {
        if let Err(error) = self
            .storage
            .set_setting("cnb_enabled", if enabled { "true" } else { "false" })
        {
            self.model.cnb.detection_error = Some(error.to_string().into());
            return;
        }
        self.model.cnb.enabled = enabled;
        self.reset_cnb_project();
    }

    pub(super) fn reset_cnb_project(&mut self) {
        self.model.cnb = CnbModel {
            enabled: self.model.cnb.enabled,
            cli: self.model.cnb.cli.clone(),
            ..CnbModel::default()
        };
        if self.model.cnb.enabled && self.model.project_is_git {
            self.inspect_cnb();
        }
    }

    pub(crate) fn inspect_cnb(&mut self) {
        if self.model.cnb.detection_request.is_some() {
            return;
        }
        let id = Uuid::new_v4();
        let path = self
            .model
            .selected_project
            .as_ref()
            .map(|project| project.canonical_path.clone().into());
        self.model.cnb.detection_error = None;
        match self.cnb_client.request(id, Request::Inspect(path)) {
            Ok(()) => self.model.cnb.detection_request = Some(id),
            Err(error) => self.model.cnb.detection_error = Some(error.to_string().into()),
        }
    }

    pub(crate) fn open_cnb(&mut self) {
        if !self.model.cnb.enabled || self.model.cnb.repository.is_none() {
            return;
        }
        self.model.cnb.opened = true;
        if self.model.cnb.issues.is_empty() && self.model.cnb.list_request.is_none() {
            self.load_cnb_issues(1, self.model.cnb.filter);
        }
    }

    pub(crate) fn load_cnb_issues(&mut self, page: usize, filter: IssueFilter) {
        let cnb = &mut self.model.cnb;
        if !cnb.enabled || page == 0 || cnb.list_request.is_some() {
            return;
        }
        let (Some(cli), Some(repository)) = (cnb.cli.clone(), cnb.repository.clone()) else {
            return;
        };
        let id = Uuid::new_v4();
        if page != cnb.page || filter != cnb.filter {
            cnb.issues.clear();
            cnb.total = 0;
        }
        cnb.page = page;
        cnb.filter = filter;
        cnb.list_error = None;
        cnb.clear_detail();
        match self.cnb_client.request(
            id,
            Request::List {
                cli,
                repository,
                page,
                filter,
            },
        ) {
            Ok(()) => cnb.list_request = Some(id),
            Err(error) => cnb.list_error = Some(error.to_string().into()),
        }
    }

    pub(crate) fn select_cnb_issue(&mut self, number: String) {
        let cnb = &mut self.model.cnb;
        if !cnb.enabled || !cnb.issues.iter().any(|issue| issue.number == number) {
            return;
        }
        let (Some(cli), Some(repository)) = (cnb.cli.clone(), cnb.repository.clone()) else {
            return;
        };
        let id = Uuid::new_v4();
        cnb.clear_detail();
        cnb.detail_number = Some(number.clone());
        // Replacing the request id prevents an earlier selection from overwriting this detail.
        match self.cnb_client.request(
            id,
            Request::Detail {
                cli,
                repository,
                number,
            },
        ) {
            Ok(()) => cnb.detail_request = Some(id),
            Err(error) => {
                cnb.detail_request = None;
                cnb.detail_error = Some(error.to_string().into());
            }
        }
        self.load_cnb_comments();
    }

    pub(crate) fn close_cnb_issue(&mut self) {
        let cnb = &mut self.model.cnb;
        cnb.clear_detail();
        if cnb.list_dirty {
            let (page, filter) = (cnb.page, cnb.filter);
            self.load_cnb_issues(page, filter);
        }
    }

    pub(crate) fn load_cnb_comments(&mut self) {
        let cnb = &mut self.model.cnb;
        if !cnb.enabled || cnb.comments_request.is_some() {
            return;
        }
        let (Some(cli), Some(repository), Some(number)) = (
            cnb.cli.clone(),
            cnb.repository.clone(),
            cnb.detail_number.clone(),
        ) else {
            return;
        };
        let id = Uuid::new_v4();
        cnb.comments = None;
        cnb.comments_error = None;
        match self.cnb_client.request(
            id,
            Request::Comments {
                cli,
                repository,
                number,
            },
        ) {
            Ok(()) => cnb.comments_request = Some(id),
            Err(error) => cnb.comments_error = Some(error.to_string().into()),
        }
    }

    pub(crate) fn prepare_cnb_chat(&mut self) -> Option<String> {
        let cnb = &self.model.cnb;
        if !cnb.enabled
            || self.model.selected_project.is_none()
            || cnb.detail_request.is_some()
            || cnb.comments_request.is_some()
            || cnb.action_request.is_some()
        {
            return None;
        }
        let prompt = cnb
            .detail
            .as_ref()?
            .chat_prompt(cnb.repository.as_deref()?, cnb.comments.as_deref()?);
        self.new_task();
        self.model
            .log_status("Issue 已加入聊天草稿，请选择 Harness 后发送。".into());
        Some(prompt)
    }

    pub(crate) fn act_on_cnb_issue(&mut self, action: IssueAction) {
        let cnb = &mut self.model.cnb;
        if !cnb.enabled
            || cnb.action_request.is_some()
            || cnb.detail_request.is_some()
            || (action == IssueAction::StartNpc && cnb.npc_comment.is_some())
        {
            return;
        }
        let (Some(cli), Some(repository), Some(issue)) =
            (cnb.cli.clone(), cnb.repository.clone(), cnb.detail.as_ref())
        else {
            return;
        };
        let id = Uuid::new_v4();
        cnb.action_error = None;
        cnb.action_success = None;
        match self.cnb_client.request(
            id,
            Request::Action {
                cli,
                repository,
                number: issue.number.clone(),
                action,
            },
        ) {
            Ok(()) => {
                cnb.action_request = Some((id, action));
                // A list requested before this mutation must not restore the old state.
                cnb.list_request = None;
                cnb.list_dirty = true;
            }
            Err(error) => cnb.action_error = Some(error.to_string().into()),
        }
    }

    pub(crate) fn refresh_cnb_npc_action(&mut self) {
        let cnb = &mut self.model.cnb;
        if !cnb.enabled || cnb.npc_request.is_some() {
            return;
        }
        let (Some(cli), Some(repository), Some(number), Some(comment)) = (
            cnb.cli.clone(),
            cnb.repository.clone(),
            cnb.detail_number.clone(),
            cnb.npc_comment.as_ref(),
        ) else {
            return;
        };
        let id = Uuid::new_v4();
        cnb.npc_error = None;
        match self.cnb_client.request(
            id,
            Request::NpcAction {
                cli,
                repository,
                number,
                comment_id: comment.id.clone(),
            },
        ) {
            Ok(()) => cnb.npc_request = Some(id),
            Err(error) => cnb.npc_error = Some(error.to_string().into()),
        }
    }

    pub(crate) fn drain_cnb_events(&mut self) -> bool {
        let events: Vec<_> = self.cnb_client.events.try_iter().collect();
        let changed = !events.is_empty();
        for event in events {
            self.handle_cnb_event(event);
        }
        changed
    }

    pub(super) fn handle_cnb_event(&mut self, event: Event) {
        let cnb = &mut self.model.cnb;
        match event.response {
            Response::Inspection { repository, cli } if cnb.detection_request == Some(event.id) => {
                cnb.detection_request = None;
                if cnb.repository != repository {
                    *cnb = CnbModel {
                        enabled: cnb.enabled,
                        ..CnbModel::default()
                    };
                }
                cnb.repository = repository;
                match cli {
                    Ok(cli) => {
                        cnb.cli = Some(cli);
                        cnb.detection_error = None;
                    }
                    Err(error) => {
                        cnb.cli = None;
                        cnb.detection_error = Some(error);
                    }
                }
                if cnb.opened && cnb.cli.is_some() && cnb.issues.is_empty() {
                    let page = cnb.page;
                    let filter = cnb.filter;
                    self.load_cnb_issues(page, filter);
                }
            }
            Response::List(result) if cnb.list_request == Some(event.id) => {
                cnb.list_request = None;
                match result {
                    Ok(page) => {
                        cnb.issues = page.issues;
                        cnb.total = page.total;
                        cnb.list_dirty = false;
                        if cnb.page > 1 && (cnb.page - 1) * PAGE_SIZE >= cnb.total {
                            let page = cnb.total.div_ceil(PAGE_SIZE).max(1);
                            let filter = cnb.filter;
                            self.load_cnb_issues(page, filter);
                        }
                    }
                    Err(error) => cnb.list_error = Some(error),
                }
            }
            Response::Detail(result) if cnb.detail_request == Some(event.id) => {
                cnb.detail_request = None;
                match result {
                    Ok(issue) if cnb.detail_number.as_deref() == Some(&issue.number) => {
                        cnb.detail = Some(issue)
                    }
                    Ok(_) => cnb.detail_error = Some("CNB 返回了不同的 Issue，请重试。".into()),
                    Err(error) => cnb.detail_error = Some(error),
                }
            }
            Response::Comments(result) if cnb.comments_request == Some(event.id) => {
                cnb.comments_request = None;
                match result {
                    Ok(comments) => {
                        if let Some(issue) = &mut cnb.detail {
                            issue.comment_count = comments.len() as u64;
                        }
                        cnb.comments = Some(comments);
                    }
                    Err(error) => cnb.comments_error = Some(error),
                }
            }
            Response::Action(result)
                if cnb.action_request.is_some_and(|(id, _)| id == event.id) =>
            {
                let (_, action) = cnb.action_request.take().unwrap();
                match result {
                    Ok(ActionResult::Updated(issue))
                        if cnb.detail_number.as_deref() == Some(&issue.number) =>
                    {
                        if let Some(index) = cnb
                            .issues
                            .iter()
                            .position(|existing| existing.number == issue.number)
                        {
                            if issue.state == cnb.filter.state() {
                                cnb.issues[index] = *issue.clone();
                            } else {
                                cnb.issues.remove(index);
                                cnb.total = cnb.total.saturating_sub(1);
                            }
                        } else if issue.state == cnb.filter.state() {
                            cnb.issues.insert(0, *issue.clone());
                            cnb.issues.truncate(PAGE_SIZE);
                            cnb.total += 1;
                        }
                        cnb.detail = Some(*issue);
                        cnb.action_success = Some(
                            match action {
                                IssueAction::AssignSelf => "已指派给当前 CNB 用户。",
                                IssueAction::SetState(IssueFilter::Closed) => "Issue 已关闭。",
                                _ => "Issue 已重新打开。",
                            }
                            .into(),
                        );
                    }
                    Ok(ActionResult::Updated(_)) => {
                        cnb.action_error = Some("CNB 返回了不同的 Issue，请重试。".into())
                    }
                    Ok(ActionResult::Npc(comment)) => {
                        let has_action = comment.action_url().is_some();
                        cnb.npc_comment = Some(comment);
                        cnb.action_success = Some("已发送 CodeBuddy NPC 处理请求。".into());
                        // Invalidate a comments read started before the newly created comment.
                        cnb.comments_request = None;
                        self.load_cnb_comments();
                        if !has_action {
                            self.refresh_cnb_npc_action();
                        }
                    }
                    Err(error) => cnb.action_error = Some(error),
                }
            }
            Response::NpcAction(result) if cnb.npc_request == Some(event.id) => {
                cnb.npc_request = None;
                match result {
                    Ok(comment)
                        if cnb
                            .npc_comment
                            .as_ref()
                            .is_some_and(|current| current.id == comment.id) =>
                    {
                        if comment.action_url().is_none() {
                            cnb.npc_error = Some(match comment.npc_failure() {
                                Some(error) => LocalizedText::new(
                                    "NPC 未启动：{error}",
                                    &[("error", error.to_owned())],
                                ),
                                None => "NPC 请求已发送，Action 链接尚未生成，请刷新状态。".into(),
                            });
                        }
                        if let Some(comments) = &mut cnb.comments
                            && let Some(existing) = comments
                                .iter_mut()
                                .find(|existing| existing.id == comment.id)
                        {
                            *existing = comment.clone();
                        }
                        cnb.npc_comment = Some(comment);
                    }
                    Ok(_) => cnb.npc_error = Some("CNB 返回了不同的评论，请刷新状态。".into()),
                    Err(error) => cnb.npc_error = Some(error),
                }
            }
            _ => {}
        }
    }
}
