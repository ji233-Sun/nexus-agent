use super::*;

#[test]
fn github_pagination_and_navigation_keep_provider_and_project_results_isolated() {
    use crate::{
        infrastructure::issues::{Event, Response},
        model::issues::{IssueFilter, IssuePage},
    };
    let provider = IssueProvider::GitHub;
    let (mut presenter, _, _directory) = fixture();
    for provider in IssueProvider::ALL {
        seed_issues(&mut presenter, provider);
    }
    presenter.open_issues(IssueProvider::Cnb);
    presenter.select_issue(IssueProvider::Cnb, "1".into());
    let cnb_request = presenter.model.cnb.detail_request;
    presenter.open_issues(provider);
    assert!(!presenter.model.cnb.opened);
    assert_eq!(presenter.model.opened_issues(), Some(provider));
    presenter.select_issue(provider, "1".into());
    let stale_detail = presenter.model.github.detail_request.unwrap();
    presenter.select_issue(provider, "2".into());
    presenter.handle_issue_event(Event {
        id: stale_detail,
        provider,
        response: Response::Detail(Ok(cnb_issue("1"))),
    });
    assert!(presenter.model.github.detail.is_none());
    finish_issue_request(
        &mut presenter,
        provider,
        Response::Detail(Ok(cnb_issue("2"))),
    );
    assert_eq!(presenter.model.cnb.detail_request, cnb_request);
    assert!(presenter.model.cnb.detail.is_none());
    presenter.act_on_issue(provider, crate::model::issues::IssueAction::StartNpc);
    assert!(presenter.model.github.action_request.is_none());
    presenter.close_issue(provider);

    presenter.load_issues(provider, 1, IssueFilter::Open);
    let first_request = presenter.model.github.list_request.unwrap();
    finish_issue_request(
        &mut presenter,
        provider,
        Response::List(Ok(IssuePage {
            issues: vec![cnb_issue("1")],
            total: 61,
            next_cursor: Some("page-2".into()),
        })),
    );
    assert_eq!(
        presenter
            .model
            .github
            .page_cursors
            .get(&2)
            .map(String::as_str),
        Some("page-2")
    );
    presenter.load_issues(provider, 2, IssueFilter::Open);
    assert_eq!(presenter.model.github.page, 2);
    assert!(presenter.model.github.issues.is_empty());
    presenter.handle_issue_event(Event {
        id: first_request,
        provider,
        response: Response::List(Ok(IssuePage {
            issues: vec![cnb_issue("stale")],
            total: 1,
            next_cursor: None,
        })),
    });
    assert!(presenter.model.github.issues.is_empty());
    finish_issue_request(
        &mut presenter,
        provider,
        Response::List(Err("读取失败".into())),
    );
    presenter.load_issues(provider, 2, IssueFilter::Open);
    assert!(presenter.model.github.list_request.is_some());
    finish_issue_request(
        &mut presenter,
        provider,
        Response::List(Ok(IssuePage {
            issues: vec![cnb_issue("31")],
            total: 61,
            next_cursor: Some("page-3".into()),
        })),
    );
    assert_eq!(presenter.model.github.page_cursors.len(), 2);
    assert_eq!(presenter.model.cnb.issues.len(), 30);
    presenter.load_issues(provider, 1, IssueFilter::Closed);
    assert!(presenter.model.github.page_cursors.is_empty());
    let stale_list = presenter.model.github.list_request.unwrap();
    presenter.new_projectless_task();
    presenter.handle_issue_event(Event {
        id: stale_list,
        provider,
        response: Response::List(Ok(IssuePage {
            issues: vec![cnb_issue("31")],
            total: 61,
            next_cursor: Some("old-project".into()),
        })),
    });
    for provider in IssueProvider::ALL {
        assert!(presenter.model.issues(provider).repository.is_none());
        assert!(presenter.model.issues(provider).issues.is_empty());
        assert!(presenter.model.issues(provider).page_cursors.is_empty());
    }
}

#[test]
fn cnb_navigation_preserves_conversation_and_ignores_obsolete_project_and_detail_results() {
    use crate::{
        infrastructure::issues::{Event, Response},
        model::issues::{IssueFilter, IssuePage},
    };
    let (mut presenter, runner, directory) = fixture();
    seed_issues(&mut presenter, IssueProvider::Cnb);
    let conversation = presenter.model.conversation.id;
    presenter.open_issues(IssueProvider::Cnb);
    assert!(presenter.model.cnb.opened);
    assert_eq!(presenter.model.conversation.id, conversation);
    assert!(runner.0.borrow().commands.is_empty());
    presenter.select_issue(IssueProvider::Cnb, "1".into());
    let earlier = presenter.model.cnb.detail_request.unwrap();
    let earlier_comments = presenter.model.cnb.comments_request.unwrap();
    presenter.select_issue(IssueProvider::Cnb, "2".into());
    presenter.handle_issue_event(Event {
        provider: IssueProvider::Cnb,
        id: earlier,
        response: Response::Detail(Ok(cnb_issue("1"))),
    });
    presenter.handle_issue_event(Event {
        provider: IssueProvider::Cnb,
        id: earlier_comments,
        response: Response::Comments(Ok(vec![cnb_comment("1")])),
    });
    assert!(presenter.model.cnb.detail.is_none());
    assert!(presenter.model.cnb.comments.is_none());
    finish_issue_request(
        &mut presenter,
        IssueProvider::Cnb,
        Response::Detail(Ok(cnb_issue("2"))),
    );
    assert_eq!(presenter.model.cnb.detail.as_ref().unwrap().number, "2");
    presenter.close_issue(IssueProvider::Cnb);
    assert_eq!(presenter.model.cnb.issues.len(), 30);
    assert_eq!(presenter.model.cnb.page, 1);
    presenter.load_issues(IssueProvider::Cnb, 2, IssueFilter::Closed);
    let previous = presenter.model.cnb.list_request.unwrap();
    assert!(presenter.model.cnb.issues.is_empty());
    assert_eq!(presenter.model.cnb.filter, IssueFilter::Closed);
    let project = directory.path().join("other-project");
    fs::create_dir(&project).unwrap();
    presenter.open_project(&project);
    presenter.handle_issue_event(Event {
        provider: IssueProvider::Cnb,
        id: previous,
        response: Response::List(Ok(IssuePage {
            next_cursor: None,
            issues: vec![cnb_issue("3")],
            total: 1,
        })),
    });
    assert!(!presenter.model.cnb.opened);
    assert!(presenter.model.cnb.repository.is_none());
    assert!(presenter.model.cnb.issues.is_empty());
}

#[test]
fn issues_launch_requires_complete_comments_and_starts_a_run_with_issue_context() {
    for provider in IssueProvider::ALL {
        use crate::{infrastructure::issues::Response, model::issues::IssueLaunchKind};
        let (mut presenter, runner, _directory) = fixture();
        seed_issues(&mut presenter, provider);
        presenter.open_issues(provider);
        presenter.select_issue(provider, "1".into());
        finish_issue_request(
            &mut presenter,
            provider,
            Response::Detail(Ok(cnb_issue("1"))),
        );
        let harness = presenter.model.conversation.selected_harness;
        assert!(!presenter.start_issue_run(
            provider,
            IssueLaunchKind::Process,
            "补充要求",
            "claude"
        ));
        finish_issue_request(
            &mut presenter,
            provider,
            Response::Comments(Err("无法读取评论".into())),
        );
        assert!(!presenter.start_issue_run(provider, IssueLaunchKind::Process, "", "claude"));
        assert!(
            !runner
                .0
                .borrow()
                .commands
                .iter()
                .any(|command| matches!(command.command, Command::RunStart(_)))
        );
        presenter.load_issue_comments(provider);
        finish_issue_request(
            &mut presenter,
            provider,
            Response::Comments(Ok((1..=31)
                .map(|id| cnb_comment(&id.to_string()))
                .collect())),
        );
        assert!(presenter.start_issue_run(
            provider,
            IssueLaunchKind::Process,
            "补充要求",
            "claude"
        ));
        let start = last_start(&runner);
        for content in [
            "CNB 集成测试 #1",
            &provider.issue_url("team/project", "1"),
            "open",
            "开发者 (@author)",
            "owner",
            "enhancement",
            "P1",
            "2026-09-09T00:00:00Z",
            "2026-09-09T01:00:00Z",
            "```rust\nfn main() {}\n```",
            "评论（31）",
            "验收条件 31",
            "评审者 (@reviewer)",
            "![截图](https://example.test/comment.png)",
            "## 补充信息",
            "补充要求",
        ] {
            assert!(start.prompt.contains(content), "{content}");
        }
        assert!(!presenter.model.issues(provider).opened);
        assert_eq!(presenter.model.conversation.selected_harness, harness);
        assert!(presenter.model.conversation.selected_task.is_some());
        assert_eq!(presenter.model.conversation.active_run, Some(start.run_id));
    }
}

#[test]
fn creating_an_issue_starts_a_run_with_the_filed_content_and_current_selection() {
    use crate::model::issues::IssueLaunchKind;
    for provider in IssueProvider::ALL {
        let (mut presenter, runner, _directory) = fixture();
        seed_issues(&mut presenter, provider);
        let source_permission = presenter.model.conversation.permission_mode;
        let previous = presenter
            .prepare_issue_run(provider, IssueLaunchKind::Create)
            .unwrap();
        presenter.select_permission_mode(PermissionMode::Yolo);
        let harness = presenter.model.conversation.selected_harness;
        let executable = presenter.model.conversation.executable.clone();
        assert!(!presenter.start_issue_run(provider, IssueLaunchKind::Create, "  ", &executable));
        assert_eq!(presenter.model.occupied_run_slots(), 0);
        assert!(presenter.start_issue_run(
            provider,
            IssueLaunchKind::Create,
            "导出 PDF 时崩溃",
            &executable
        ));
        let start = last_start(&runner);
        for content in [
            "导出 PDF 时崩溃",
            "team/project",
            provider.executable(),
            "需要提的 Issue",
        ] {
            assert!(start.prompt.contains(content), "{content}");
        }
        assert_eq!(start.harness, harness);
        assert_eq!(start.permission_mode, PermissionMode::Yolo);
        assert_eq!(presenter.model[previous].permission_mode, source_permission);
        assert!(presenter.model.conversation.selected_task.is_some());
        assert_eq!(presenter.model.conversation.active_run, Some(start.run_id));
    }
}

#[test]
fn issue_launch_runs_in_parallel_while_a_session_is_executing() {
    use crate::{infrastructure::issues::Response, model::issues::IssueLaunchKind};
    for provider in IssueProvider::ALL {
        for kind in [IssueLaunchKind::Create, IssueLaunchKind::Process] {
            let (mut presenter, runner, _directory, first) = worktree_fixture("先执行的任务");
            assert_eq!(presenter.model.conversation.active_run, Some(first.run_id));
            seed_issues(&mut presenter, provider);
            if kind == IssueLaunchKind::Process {
                presenter.select_issue(provider, "1".into());
                finish_issue_request(
                    &mut presenter,
                    provider,
                    Response::Detail(Ok(cnb_issue("1"))),
                );
                finish_issue_request(
                    &mut presenter,
                    provider,
                    Response::Comments(Ok(vec![cnb_comment("1")])),
                );
            }
            let extra = match kind {
                IssueLaunchKind::Create => "并行提交的 Issue",
                IssueLaunchKind::Process => "并行处理的补充信息",
            };
            let previous = presenter.prepare_issue_run(provider, kind).unwrap();
            assert_eq!(presenter.model[previous].active_run, Some(first.run_id));
            assert!(presenter.select_harness(HarnessKind::Codex, "claude"));
            presenter
                .model
                .harnesses
                .insert(HarnessKind::Codex, ready_probe(HarnessKind::Codex));
            let models = vec![catalog_model(
                "gpt-5.2-codex",
                false,
                &[ThinkingEffort::High],
                ThinkingEffort::High,
            )];
            emit_current_catalog(&presenter, &runner, models.clone());
            presenter.drain_events();
            presenter.select_catalog_model(Some("gpt-5.2-codex".into()));
            presenter.select_effort(ThinkingEffort::High);
            presenter.select_permission_mode(PermissionMode::Yolo);
            assert!(presenter.start_issue_run(provider, kind, extra, "codex"));
            finish_workspace_operation(&mut presenter);
            emit_current_catalog(&presenter, &runner, models);
            presenter.drain_events();
            let second = last_start(&runner);
            assert_ne!(second.run_id, first.run_id);
            assert_ne!(second.cwd, first.cwd);
            assert_eq!(second.harness, HarnessKind::Codex);
            assert_eq!(second.model.as_deref(), Some("gpt-5.2-codex"));
            assert_eq!(second.effort, ThinkingEffort::High);
            assert_eq!(second.permission_mode, PermissionMode::Yolo);
            assert_eq!(
                presenter.model[previous].selected_harness,
                HarnessKind::Claude
            );
            assert_eq!(
                presenter.model[previous].permission_mode,
                first.permission_mode
            );
            assert!(second.prompt.contains(extra));
            assert_eq!(presenter.model.conversation.active_run, Some(second.run_id));
            assert_eq!(presenter.model.active_run_count(), 2);
        }
    }
}

#[test]
fn cancelling_issue_configuration_restores_the_source_draft_and_ignores_late_catalogs() {
    use crate::model::issues::IssueLaunchKind;
    let (mut presenter, runner, _directory) = fixture();
    seed_issues(&mut presenter, IssueProvider::GitHub);
    presenter.select_permission_mode(PermissionMode::AutoEdit);
    presenter.select_catalog_model(Some("opus".into()));
    let previous = presenter
        .prepare_issue_run(IssueProvider::GitHub, IssueLaunchKind::Create)
        .unwrap();
    let context = presenter.model.conversation.id;
    assert!(presenter.select_harness(HarnessKind::Codex, "claude"));
    let ModelCatalogState::Loading { request_id, .. } = presenter.model.conversation.model_catalog
    else {
        panic!("catalog refresh")
    };
    presenter.select_permission_mode(PermissionMode::Yolo);
    presenter.cancel_issue_run(context, previous);
    assert_eq!(presenter.model.conversation.id, previous);
    assert_eq!(
        presenter.model.conversation.selected_harness,
        HarnessKind::Claude
    );
    assert_eq!(
        presenter.model.conversation.permission_mode,
        PermissionMode::AutoEdit
    );
    assert_eq!(
        presenter.model.conversation.model_override.as_deref(),
        Some("opus")
    );
    assert!(!presenter.model.conversations.contains_key(&context));
    runner.emit(Event::ModelCatalogLoaded {
        request_id,
        harness: HarnessKind::Codex,
        models: vec![catalog_model(
            "gpt-5.2-codex",
            false,
            &[ThinkingEffort::High],
            ThinkingEffort::High,
        )],
    });
    presenter.drain_events();
    assert_eq!(
        presenter.model.conversation.selected_harness,
        HarnessKind::Claude
    );
    assert_eq!(presenter.model.occupied_run_slots(), 0);
}

#[test]
fn issue_processing_can_use_local_or_worktree_and_preserves_the_choice_when_starting() {
    use crate::{
        infrastructure::{git, issues::Response},
        model::{issues::IssueLaunchKind, workspace::WorkspaceKind},
    };
    for kind in [WorkspaceKind::Local, WorkspaceKind::Worktree] {
        let (directory, project) = git::tests::repository_fixture();
        let path = Path::new(&project.canonical_path);
        git::git(path, &["update-ref", "refs/remotes/origin/main", "HEAD"]).unwrap();
        let (mut presenter, runner, _fixture) = fixture();
        presenter.worktree_root = Ok(directory.path().canonicalize().unwrap().join("worktrees"));
        presenter.open_project(path);
        // The explicit launch choice must win over the opposite saved project preference.
        presenter.select_workspace_kind(if kind == WorkspaceKind::Local {
            WorkspaceKind::Worktree
        } else {
            WorkspaceKind::Local
        });
        seed_issues(&mut presenter, IssueProvider::GitHub);
        presenter.open_issues(IssueProvider::GitHub);
        presenter.select_issue(IssueProvider::GitHub, "1".into());
        finish_issue_request(
            &mut presenter,
            IssueProvider::GitHub,
            Response::Detail(Ok(cnb_issue("1"))),
        );
        finish_issue_request(
            &mut presenter,
            IssueProvider::GitHub,
            Response::Comments(Ok(vec![])),
        );
        presenter
            .prepare_issue_run(IssueProvider::GitHub, IssueLaunchKind::Process)
            .unwrap();
        presenter.select_workspace_kind(kind);
        assert_eq!(
            presenter.model.conversation.workspace_draft.base,
            "origin/main"
        );
        assert!(presenter.start_issue_run(
            IssueProvider::GitHub,
            IssueLaunchKind::Process,
            "",
            "claude"
        ));
        if kind == WorkspaceKind::Worktree {
            finish_workspace_operation(&mut presenter);
            emit_current_catalog(&presenter, &runner, claude_aliases());
            presenter.drain_events();
        }
        let start = last_start(&runner);
        assert_eq!(
            presenter
                .model
                .conversation
                .selected_workspace
                .as_ref()
                .unwrap()
                .kind,
            kind
        );
        assert_eq!(
            start.cwd == project.canonical_path,
            kind == WorkspaceKind::Local
        );
    }
}

#[test]
fn issue_configuration_defaults_to_worktree_when_the_local_checkout_is_occupied() {
    use crate::{
        infrastructure::git,
        model::{issues::IssueLaunchKind, workspace::WorkspaceKind},
    };
    let (directory, project) = git::tests::repository_fixture();
    let (mut presenter, runner, _fixture) = fixture();
    presenter.worktree_root = Ok(directory.path().canonicalize().unwrap().join("worktrees"));
    presenter.open_project(Path::new(&project.canonical_path));
    assert!(presenter.submit("local task", "claude"));
    let first = last_start(&runner);
    seed_issues(&mut presenter, IssueProvider::Cnb);
    let previous = presenter
        .prepare_issue_run(IssueProvider::Cnb, IssueLaunchKind::Create)
        .unwrap();
    assert_eq!(
        presenter.model.conversation.workspace_draft.kind,
        WorkspaceKind::Worktree
    );
    presenter.select_workspace_kind(WorkspaceKind::Local);
    assert!(
        presenter
            .issue_run_blocker()
            .unwrap()
            .render(Language::Chinese)
            .contains("本地目录已有任务")
    );
    assert!(!presenter.start_issue_run(
        IssueProvider::Cnb,
        IssueLaunchKind::Create,
        "new issue",
        "claude"
    ));
    assert_eq!(presenter.model[previous].active_run, Some(first.run_id));
    presenter.select_workspace_kind(WorkspaceKind::Worktree);
    assert!(presenter.start_issue_run(
        IssueProvider::Cnb,
        IssueLaunchKind::Create,
        "new issue",
        "claude"
    ));
    finish_workspace_operation(&mut presenter);
    emit_current_catalog(&presenter, &runner, claude_aliases());
    presenter.drain_events();
    assert_eq!(presenter.model.active_run_count(), 2);
    assert_ne!(last_start(&runner).cwd, first.cwd);
    assert!(presenter.can_prepare_issue_run(IssueProvider::Cnb, IssueLaunchKind::Create));
    presenter
        .prepare_issue_run(IssueProvider::Cnb, IssueLaunchKind::Create)
        .unwrap();
    assert!(presenter.issue_run_blocker().is_none());
    assert!(presenter.start_issue_run(
        IssueProvider::Cnb,
        IssueLaunchKind::Create,
        "third issue task",
        "claude"
    ));
    finish_workspace_operation(&mut presenter);
    emit_current_catalog(&presenter, &runner, claude_aliases());
    presenter.drain_events();
    assert_eq!(presenter.model.active_run_count(), 3);
    assert_ne!(last_start(&runner).cwd, first.cwd);
    assert_eq!(presenter.model[previous].active_run, Some(first.run_id));
}

#[test]
fn issues_mutations_block_duplicates_preserve_failures_and_update_filtered_lists() {
    for provider in IssueProvider::ALL {
        use crate::{
            infrastructure::issues::{ActionResult, Event, Response},
            model::issues::{IssueAction, IssueFilter, User},
        };
        let (mut presenter, _, _directory) = fixture();
        seed_issues(&mut presenter, provider);
        presenter.select_issue(provider, "1".into());
        finish_issue_request(
            &mut presenter,
            provider,
            Response::Detail(Ok(cnb_issue("1"))),
        );
        presenter.act_on_issue(provider, IssueAction::AssignSelf);
        let request = presenter.model.issues(provider).action_request;
        presenter.act_on_issue(provider, IssueAction::SetState(IssueFilter::Closed));
        assert_eq!(presenter.model.issues(provider).action_request, request);
        finish_issue_request(
            &mut presenter,
            provider,
            Response::Action(Err("HTTP 403".into())),
        );
        assert!(presenter.model.issues(provider).action_error.is_some());
        assert_eq!(
            presenter
                .model
                .issues(provider)
                .detail
                .as_ref()
                .unwrap()
                .state,
            "open"
        );
        assert_eq!(presenter.model.issues(provider).total, 61);
        presenter.act_on_issue(provider, IssueAction::AssignSelf);
        let mut issue = cnb_issue("1");
        issue.assignees.push(User {
            username: "me".into(),
            nickname: String::new(),
        });
        finish_issue_request(
            &mut presenter,
            provider,
            Response::Action(Ok(ActionResult::Updated(Box::new(issue.clone())))),
        );
        assert!(presenter.model.issues(provider).action_error.is_none());
        assert_eq!(
            presenter
                .model
                .issues(provider)
                .detail
                .as_ref()
                .unwrap()
                .assignees[1]
                .username,
            "me"
        );
        assert_eq!(
            presenter.model.issues(provider).issues[0].assignees.len(),
            2
        );
        for state in [IssueFilter::Closed, IssueFilter::Open] {
            presenter.act_on_issue(provider, IssueAction::SetState(state));
            issue.state = state.state().into();
            finish_issue_request(
                &mut presenter,
                provider,
                Response::Action(Ok(ActionResult::Updated(Box::new(issue.clone())))),
            );
            assert_eq!(
                presenter
                    .model
                    .issues(provider)
                    .detail
                    .as_ref()
                    .unwrap()
                    .state,
                state.state()
            );
            assert_eq!(
                presenter.model.issues(provider).total,
                if state == IssueFilter::Closed { 60 } else { 61 }
            );
            assert_eq!(
                presenter
                    .model
                    .issues(provider)
                    .issues
                    .iter()
                    .any(|issue| issue.number == "1"),
                state == IssueFilter::Open
            );
        }
        presenter.act_on_issue(provider, IssueAction::AssignSelf);
        finish_issue_request(
            &mut presenter,
            provider,
            Response::Action(Ok(ActionResult::Updated(Box::new(cnb_issue("2"))))),
        );
        assert!(presenter.model.issues(provider).action_error.is_some());
        assert_eq!(
            presenter
                .model
                .issues(provider)
                .detail
                .as_ref()
                .unwrap()
                .number,
            "1"
        );
        presenter.act_on_issue(provider, IssueAction::AssignSelf);
        let stale = presenter.model.issues(provider).action_request.unwrap().0;
        presenter.close_issue(provider);
        assert!(presenter.model.issues(provider).list_request.is_some());
        presenter.handle_issue_event(Event {
            provider,
            id: stale,
            response: Response::Action(Ok(ActionResult::Updated(Box::new(issue)))),
        });
        assert!(presenter.model.issues(provider).detail.is_none());
    }
}

#[test]
fn cnb_npc_tracks_the_triggering_comment_and_retries_only_status_reads() {
    use crate::{
        infrastructure::issues::{ActionResult, Event, Response},
        model::issues::IssueAction,
    };
    let (mut presenter, _, _directory) = fixture();
    seed_issues(&mut presenter, IssueProvider::Cnb);
    presenter.select_issue(IssueProvider::Cnb, "1".into());
    finish_issue_request(
        &mut presenter,
        IssueProvider::Cnb,
        Response::Detail(Ok(cnb_issue("1"))),
    );
    let stale_comments = presenter.model.cnb.comments_request.unwrap();
    presenter.act_on_issue(IssueProvider::Cnb, IssueAction::StartNpc);
    finish_issue_request(
        &mut presenter,
        IssueProvider::Cnb,
        Response::Action(Ok(ActionResult::Npc(cnb_comment("987")))),
    );
    let initial = presenter.model.cnb.npc_request;
    assert!(initial.is_some());
    presenter.handle_issue_event(Event {
        provider: IssueProvider::Cnb,
        id: stale_comments,
        response: Response::Comments(Ok(vec![])),
    });
    assert!(presenter.model.cnb.comments.is_none());
    presenter.act_on_issue(IssueProvider::Cnb, IssueAction::StartNpc);
    assert!(presenter.model.cnb.action_request.is_none());
    presenter.refresh_cnb_npc_action();
    assert_eq!(presenter.model.cnb.npc_request, initial);
    finish_issue_request(
        &mut presenter,
        IssueProvider::Cnb,
        Response::NpcAction(Err("读取失败".into())),
    );
    assert!(presenter.model.cnb.npc_error.is_some());
    assert_eq!(presenter.model.cnb.npc_comment.as_ref().unwrap().id, "987");
    presenter.refresh_cnb_npc_action();
    assert_ne!(presenter.model.cnb.npc_request, initial);
    finish_issue_request(
        &mut presenter,
        IssueProvider::Cnb,
        Response::NpcAction(Ok(cnb_comment("987"))),
    );
    assert!(presenter.model.cnb.npc_error.is_some());
    presenter.refresh_cnb_npc_action();
    let failure = serde_json::from_value(serde_json::json!({"id":"987", "statuses":{
        "npc":[{"statuses":[{"state":"skipped", "description":"需要开发者权限"}]}]
    }}))
    .unwrap();
    finish_issue_request(
        &mut presenter,
        IssueProvider::Cnb,
        Response::NpcAction(Ok(failure)),
    );
    assert!(
        presenter
            .model
            .cnb
            .npc_error
            .as_ref()
            .unwrap()
            .render(crate::i18n::Language::Chinese)
            .contains("需要开发者权限")
    );
    presenter.refresh_cnb_npc_action();
    let comment: crate::model::issues::Comment =
        serde_json::from_value(serde_json::json!({"id":"987", "statuses":{
            "npc":[{"statuses":[{"target_url":"https://cnb.cool/team/project/-/build/logs/cnb-1"}]}]
        }}))
        .unwrap();
    finish_issue_request(
        &mut presenter,
        IssueProvider::Cnb,
        Response::NpcAction(Ok(comment)),
    );
    assert!(presenter.model.cnb.npc_error.is_none());
    assert_eq!(
        presenter
            .model
            .cnb
            .npc_comment
            .as_ref()
            .unwrap()
            .action_url(),
        Some("https://cnb.cool/team/project/-/build/logs/cnb-1")
    );
    presenter.refresh_cnb_npc_action();
    let stale = presenter.model.cnb.npc_request.unwrap();
    presenter.select_issue(IssueProvider::Cnb, "2".into());
    presenter.handle_issue_event(Event {
        provider: IssueProvider::Cnb,
        id: stale,
        response: Response::NpcAction(Ok(cnb_comment("987"))),
    });
    assert!(presenter.model.cnb.npc_comment.is_none());
    assert!(presenter.model.cnb.npc_error.is_none());
}

#[test]
fn issues_failures_can_be_retried_and_disabling_invalidates_requests_and_persists() {
    for provider in IssueProvider::ALL {
        use crate::{
            infrastructure::issues::{Event, Response},
            model::issues::{IssueFilter, IssuePage},
        };
        let (mut presenter, _, _directory) = fixture();
        seed_issues(&mut presenter, provider);
        presenter.load_issues(provider, 1, IssueFilter::Open);
        finish_issue_request(
            &mut presenter,
            provider,
            Response::List(Err("需要登录".into())),
        );
        assert!(presenter.model.issues(provider).list_request.is_none());
        assert!(presenter.model.issues(provider).list_error.is_some());
        presenter.load_issues(provider, 1, IssueFilter::Open);
        finish_issue_request(
            &mut presenter,
            provider,
            Response::List(Ok(IssuePage {
                next_cursor: None,
                issues: vec![cnb_issue("2")],
                total: 1,
            })),
        );
        assert!(presenter.model.issues(provider).list_error.is_none());
        assert_eq!(presenter.model.issues(provider).total, 1);
        presenter.select_issue(provider, "2".into());
        let id = presenter.model.issues(provider).detail_request.unwrap();
        presenter.set_issues_enabled(provider, false);
        assert!(!presenter.model.issues(provider).enabled);
        assert_eq!(
            presenter
                .storage
                .setting(&format!("{}_enabled", provider.key()))
                .unwrap()
                .as_deref(),
            Some("false")
        );
        presenter.handle_issue_event(Event {
            provider,
            id,
            response: Response::Detail(Ok(cnb_issue("2"))),
        });
        assert!(presenter.model.issues(provider).detail.is_none());
        let presenter =
            Presenter::new(presenter.storage, Err(anyhow::anyhow!("test runner")), None);
        assert!(!presenter.model.issues(provider).enabled);
    }
}

#[test]
fn cnb_inspection_keeps_repository_visible_when_cli_is_missing_and_rejects_stale_inspections() {
    use crate::infrastructure::issues::{Event, Response};
    let (mut presenter, _, _directory) = fixture();
    let old = Uuid::new_v4();
    let current = Uuid::new_v4();
    presenter.model.cnb.detection_request = Some(current);
    presenter.handle_issue_event(Event {
        provider: IssueProvider::Cnb,
        id: old,
        response: Response::Inspection {
            repository: Some("wrong/project".into()),
            cli: Err("missing".into()),
        },
    });
    assert!(presenter.model.cnb.repository.is_none());
    finish_issue_request(
        &mut presenter,
        IssueProvider::Cnb,
        Response::Inspection {
            repository: Some("team/project".into()),
            cli: Err("missing".into()),
        },
    );
    assert_eq!(
        presenter.model.cnb.repository.as_deref(),
        Some("team/project")
    );
    assert!(presenter.model.cnb.cli.is_none());
    presenter.open_issues(IssueProvider::Cnb);
    assert!(presenter.model.cnb.opened);
    assert!(presenter.model.cnb.list_request.is_none());
    presenter.model.cnb.detection_request = Some(Uuid::new_v4());
    finish_issue_request(
        &mut presenter,
        IssueProvider::Cnb,
        Response::Inspection {
            repository: Some("team/project".into()),
            cli: Ok(crate::model::issues::Cli {
                path: "/missing-test-cnb".into(),
                version: "1.10.10".into(),
            }),
        },
    );
    assert!(presenter.model.cnb.list_request.is_some());
    presenter.new_task();
    assert!(!presenter.model.cnb.opened);
}
