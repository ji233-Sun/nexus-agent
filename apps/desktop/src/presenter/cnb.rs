use super::*;
use crate::{
    infrastructure::cnb::{Event, Request, Response},
    model::cnb::{CnbModel, IssueFilter, PAGE_SIZE},
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
        cnb.detail = None;
        cnb.detail_number = None;
        cnb.detail_request = None;
        cnb.detail_error = None;
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
        cnb.detail_number = Some(number.clone());
        cnb.detail = None;
        cnb.detail_error = None;
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
    }

    pub(crate) fn close_cnb_issue(&mut self) {
        self.model.cnb.detail_number = None;
        self.model.cnb.detail = None;
        self.model.cnb.detail_request = None;
        self.model.cnb.detail_error = None;
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
            _ => {}
        }
    }
}
