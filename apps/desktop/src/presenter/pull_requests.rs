use super::*;
use crate::{
    infrastructure::pull_requests::{Request, Response},
    model::{
        issues::IssueFilter,
        pull_requests::{MergeMethod, PullAction, PullRunKind},
    },
};

impl Presenter {
    pub(crate) fn open_pull_requests(&mut self, provider: IssueProvider) {
        let issues = self.model.issues_mut(provider);
        if !issues.enabled || issues.repository.is_none() {
            return;
        }
        issues.pulls.opened = true;
        self.open_issues(provider);
    }

    pub(crate) fn open_issue_tab(&mut self, provider: IssueProvider) {
        self.model.issues_mut(provider).pulls.opened = false;
        self.open_issues(provider);
    }

    pub(crate) fn load_pull_requests(
        &mut self,
        provider: IssueProvider,
        page: usize,
        filter: IssueFilter,
    ) {
        let issues = self.model.issues_mut(provider);
        let pulls = &mut issues.pulls;
        if !issues.enabled
            || page == 0
            || pulls.list_request.is_some()
            || pulls.action_request.is_some()
        {
            return;
        }
        let (Some(cli), Some(repository)) = (issues.cli.clone(), issues.repository.clone()) else {
            return;
        };
        let cursor = if provider == IssueProvider::GitHub && page > 1 {
            let Some(cursor) = pulls.page_cursors.get(&page).cloned() else {
                return;
            };
            Some(cursor)
        } else {
            None
        };
        if page == 1 || filter != pulls.filter {
            pulls.page_cursors.clear();
        }
        if page != pulls.page || filter != pulls.filter {
            pulls.pulls.clear();
            pulls.total = 0;
        }
        pulls.page = page;
        pulls.filter = filter;
        pulls.list_error = None;
        pulls.clear_detail();
        let id = Uuid::new_v4();
        match self.issues_client.request_pulls(
            id,
            provider,
            cli,
            repository,
            Request::List {
                page,
                filter,
                cursor,
            },
        ) {
            Ok(()) => pulls.list_request = Some(id),
            Err(error) => pulls.list_error = Some(error.to_string().into()),
        }
    }

    pub(crate) fn select_pull_request(&mut self, provider: IssueProvider, number: String) {
        let issues = self.model.issues_mut(provider);
        if !issues.enabled || issues.pulls.action_request.is_some() {
            return;
        }
        let valid = issues.pulls.pulls.iter().any(|pull| pull.number == number)
            || issues
                .pulls
                .detail
                .as_ref()
                .is_some_and(|detail| detail.stack.iter().any(|pull| pull.number == number));
        if !valid {
            return;
        }
        issues.pulls.clear_detail();
        issues.pulls.detail_number = Some(number);
        self.refresh_pull_request(provider);
    }

    pub(crate) fn refresh_pull_request(&mut self, provider: IssueProvider) {
        let issues = self.model.issues_mut(provider);
        let pulls = &mut issues.pulls;
        if !issues.enabled || pulls.detail_request.is_some() || pulls.action_request.is_some() {
            return;
        }
        let (Some(cli), Some(repository), Some(number)) = (
            issues.cli.clone(),
            issues.repository.clone(),
            pulls.detail_number.clone(),
        ) else {
            return;
        };
        pulls.detail_error = None;
        pulls.confirmation = None;
        let id = Uuid::new_v4();
        match self.issues_client.request_pulls(
            id,
            provider,
            cli,
            repository,
            Request::Detail(number),
        ) {
            Ok(()) => pulls.detail_request = Some(id),
            Err(error) => pulls.detail_error = Some(error.to_string().into()),
        }
    }

    pub(crate) fn close_pull_request(&mut self, provider: IssueProvider) {
        let pulls = &mut self.model.issues_mut(provider).pulls;
        if pulls.action_request.is_some() {
            return;
        }
        pulls.clear_detail();
        let filter = pulls.filter;
        self.load_pull_requests(provider, 1, filter);
    }

    pub(crate) fn set_pull_merge_method(&mut self, provider: IssueProvider, method: MergeMethod) {
        let pulls = &mut self.model.issues_mut(provider).pulls;
        if pulls.action_request.is_none() && pulls.confirmation.is_none() {
            pulls.merge_method = method;
        }
    }

    pub(crate) fn confirm_pull_action(
        &mut self,
        provider: IssueProvider,
        action: Option<PullAction>,
    ) {
        let issues = self.model.issues_mut(provider);
        let pulls = &mut issues.pulls;
        if !issues.enabled
            || pulls.detail_request.is_some()
            || pulls.detail_error.is_some()
            || pulls.action_request.is_some()
        {
            return;
        }
        if let Some(action) = action {
            let Some(detail) = pulls.detail.as_ref() else {
                return;
            };
            if detail.pull.state != "open"
                || (matches!(action, PullAction::Merge(_)) && !detail.pull.can_merge())
            {
                return;
            }
        }
        pulls.confirmation = action;
    }

    pub(crate) fn act_on_pull_request(&mut self, provider: IssueProvider) {
        let issues = self.model.issues_mut(provider);
        let pulls = &mut issues.pulls;
        if !issues.enabled
            || pulls.detail_request.is_some()
            || pulls.detail_error.is_some()
            || pulls.action_request.is_some()
        {
            return;
        }
        let (Some(cli), Some(repository), Some(detail), Some(action)) = (
            issues.cli.clone(),
            issues.repository.clone(),
            pulls.detail.as_ref(),
            pulls.confirmation,
        ) else {
            return;
        };
        if detail.pull.state != "open"
            || (matches!(action, PullAction::Merge(_)) && !detail.pull.can_merge())
        {
            return;
        }
        let id = Uuid::new_v4();
        pulls.action_error = None;
        pulls.action_success = None;
        match self.issues_client.request_pulls(
            id,
            provider,
            cli,
            repository,
            Request::Action {
                pull: Box::new(detail.pull.clone()),
                action,
            },
        ) {
            Ok(()) => {
                pulls.action_request = Some((id, action));
                pulls.confirmation = None;
                // A list requested before this mutation must not restore the old state.
                pulls.list_request = None;
            }
            Err(error) => pulls.action_error = Some(error.to_string().into()),
        }
    }

    pub(crate) fn start_pull_run(
        &mut self,
        provider: IssueProvider,
        kind: PullRunKind,
        extra: &str,
        executable: &str,
    ) -> bool {
        let issues = self.model.issues(provider);
        if !issues.enabled
            || issues.pulls.detail_request.is_some()
            || issues.pulls.detail_error.is_some()
            || issues.pulls.action_request.is_some()
            || self.model.selected_project.is_none()
            || self.model.active_run.is_some()
            || self.model.occupied_run_slots() >= 2
        {
            return false;
        }
        let (Some(repository), Some(detail)) =
            (issues.repository.as_deref(), issues.pulls.detail.as_ref())
        else {
            return false;
        };
        if !detail.can_run(provider, kind) {
            return false;
        }
        let prompt = detail.chat_prompt(provider, repository, kind, extra);
        self.new_task();
        self.start_run(None, &prompt, executable, self.model.permission_mode)
    }

    pub(super) fn handle_pull_event(
        &mut self,
        id: Uuid,
        provider: IssueProvider,
        response: Response,
    ) {
        let pulls = &mut self.model.issues_mut(provider).pulls;
        match response {
            Response::List(result) if pulls.list_request == Some(id) => {
                pulls.list_request = None;
                match result {
                    Ok(page) => {
                        pulls.page_cursors.retain(|page, _| *page <= pulls.page);
                        if let Some(cursor) = page.next_cursor {
                            pulls.page_cursors.insert(pulls.page + 1, cursor);
                        }
                        pulls.pulls = page.pulls;
                        pulls.total = page.total;
                        if pulls.page > 1
                            && (pulls.page - 1) * crate::model::issues::PAGE_SIZE >= pulls.total
                        {
                            let filter = pulls.filter;
                            self.load_pull_requests(provider, 1, filter);
                        }
                    }
                    Err(error) => pulls.list_error = Some(error),
                }
            }
            Response::Detail(result) if pulls.detail_request == Some(id) => {
                pulls.detail_request = None;
                match result {
                    Ok(detail) if pulls.detail_number.as_deref() == Some(&detail.pull.number) => {
                        pulls.detail = Some(detail)
                    }
                    Ok(_) => {
                        pulls.detail_error = Some("平台返回了不同的 PR，请刷新后重试。".into())
                    }
                    Err(error) => pulls.detail_error = Some(error),
                }
            }
            Response::Action(result)
                if pulls
                    .action_request
                    .is_some_and(|(request, _)| request == id) =>
            {
                let (_, action) = pulls.action_request.take().unwrap();
                match result {
                    Ok(pull) if pulls.detail_number.as_deref() == Some(&pull.number) => {
                        if let Some(index) = pulls
                            .pulls
                            .iter()
                            .position(|item| item.number == pull.number)
                        {
                            let matches_filter = if pulls.filter == IssueFilter::Open {
                                pull.state == "open"
                            } else {
                                pull.state != "open"
                            };
                            if matches_filter {
                                pulls.pulls[index] = *pull.clone();
                            } else {
                                pulls.pulls.remove(index);
                                pulls.total = pulls.total.saturating_sub(1);
                            }
                        }
                        if let Some(detail) = &mut pulls.detail {
                            detail.pull = *pull;
                        }
                        pulls.action_success = Some(
                            match action {
                                PullAction::Merge(_) => "PR 已合并。",
                                PullAction::Close => "PR 已关闭。",
                            }
                            .into(),
                        );
                    }
                    Ok(_) => {
                        pulls.action_error = Some("平台返回了不同的 PR，请刷新后重试。".into())
                    }
                    Err(error) => pulls.action_error = Some(error),
                }
            }
            _ => {}
        }
    }
}
