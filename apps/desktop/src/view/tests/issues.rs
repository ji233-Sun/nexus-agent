use super::*;

#[gpui::test]
fn github_issue_page_opens_builtin_ai_with_complete_context_and_harness_selection(
    cx: &mut gpui::TestAppContext,
) {
    use crate::{
        infrastructure::issues::{ActionResult, Response},
        model::issues::{IssueAction, IssueFilter},
        presenter::tests::{cnb_comment, cnb_issue, finish_issue_request, seed_issues},
    };
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let provider = IssueProvider::GitHub;
    let (mut presenter, _, _directory) = fixture();
    presenter.set_language(Language::English);
    for provider in IssueProvider::ALL {
        seed_issues(&mut presenter, provider);
    }
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    cx.simulate_resize(gpui::size(px(1120.), px(900.)));
    cx.run_until_parked();
    click_debug(cx, "sidebar-github");
    cx.run_until_parked();
    assert!(cx.debug_bounds("github-page").is_some());
    assert!(cx.debug_bounds("composer-surface").is_none());
    assert!(cx.debug_bounds("cnb-page").is_none());
    click_debug(cx, "github-repository");
    assert_eq!(
        cx.opened_url(),
        Some("https://github.com/team/project".into())
    );
    click_debug(cx, "github-issue-1");
    view.update(cx, |view, cx| {
        let mut issue = cnb_issue("1");
        issue.title = "GitHub issue".into();
        issue.body = "Issue body **Markdown**.".into();
        finish_issue_request(&mut view.presenter, provider, Response::Detail(Ok(issue)));
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("github-detail").is_some());
    assert!(cx.debug_bounds("github-npc").is_none());
    click_debug(cx, "github-chat");
    view.update(cx, |view, _| assert!(view.presenter.model().github.opened));
    assert!(cx.debug_bounds("issue-launch-surface").is_none());
    view.update_in(cx, |view, _window, cx| {
        let mut last = cnb_comment("101");
        last.body = "Acceptance criteria from the final page.".into();
        finish_issue_request(
            &mut view.presenter,
            provider,
            Response::Comments(Ok(vec![last])),
        );
        cx.notify();
    });
    cx.run_until_parked();
    for (selector, action) in [
        ("github-assign-self", IssueAction::AssignSelf),
        (
            "github-change-state",
            IssueAction::SetState(IssueFilter::Closed),
        ),
    ] {
        click_debug(cx, selector);
        view.update(cx, |view, cx| {
            assert_eq!(
                view.presenter.model().github.action_request.unwrap().1,
                action
            );
            let mut issue = view.presenter.model().github.detail.clone().unwrap();
            if let IssueAction::SetState(state) = action {
                issue.state = state.state().into();
            }
            finish_issue_request(
                &mut view.presenter,
                provider,
                Response::Action(Ok(ActionResult::Updated(Box::new(issue)))),
            );
            cx.notify();
        });
        cx.run_until_parked();
    }
    click_debug(cx, "github-chat");
    cx.run_until_parked();
    let surface = cx.debug_bounds("issue-launch-surface").unwrap();
    let card = cx.debug_bounds("issue-launch-card").unwrap();
    assert!(cx.debug_bounds("github-page").is_some());
    assert!(cx.debug_bounds("composer-surface").is_none());
    assert!(card.left() >= surface.left() && card.right() <= surface.right());
    assert!(card.top() >= surface.top() && card.bottom() <= surface.bottom());
    assert!(card.center().x >= surface.center().x - px(1.));
    assert!(card.center().x <= surface.center().x + px(1.));
    assert!(card.center().y >= surface.center().y - px(1.));
    assert!(card.center().y <= surface.center().y + px(1.));
    let start = cx.debug_bounds("issue-launch-start").unwrap();
    assert!(start.left() >= card.left() && start.right() <= card.right());
    assert!(start.top() >= card.top() && start.bottom() <= card.bottom());
    view.update_in(cx, |view, window, cx| {
        view.issue_launch_input
            .update(cx, |input, cx| input.set_value("补充说明", window, cx));
    });
    cx.run_until_parked();
    click_debug(cx, "issue-launch-start");
    cx.run_until_parked();
    assert!(cx.debug_bounds("issue-launch-surface").is_none());
    assert!(cx.debug_bounds("github-page").is_none());
    assert!(cx.debug_bounds("composer-surface").is_some());
    view.update(cx, |view, _| {
        let prompt = view
            .presenter
            .model()
            .conversation
            .messages
            .iter()
            .find(|message| message.role == MessageRole::User)
            .map(|message| message.content.clone())
            .expect("user prompt");
        for content in [
            "GitHub Issue",
            "https://github.com/team/project/issues/1",
            "Issue body **Markdown**.",
            "Acceptance criteria from the final page.",
            "## 补充信息",
            "补充说明",
        ] {
            assert!(prompt.contains(content), "{content}");
        }
        assert!(!prompt.contains("cnb.cool"));
        assert!(view.presenter.model().conversation.active_run.is_some());
        assert!(view.presenter.model().opened_issues().is_none());
    });
}

#[gpui::test]
fn cnb_issue_actions_import_complete_context_and_offer_harness_selection(
    cx: &mut gpui::TestAppContext,
) {
    use crate::{
        infrastructure::issues::{ActionResult, Response},
        model::issues::{IssueAction, IssueFilter},
        presenter::tests::{cnb_comment, cnb_issue, finish_issue_request, seed_issues},
    };
    // Media loading uses Tokio workers outside GPUI's deterministic test scheduler.
    cx.executor().allow_parking();
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, _, _directory) = fixture();
    seed_issues(&mut presenter, IssueProvider::Cnb);
    presenter.open_issues(IssueProvider::Cnb);
    presenter.select_issue(IssueProvider::Cnb, "1".into());
    let mut issue = cnb_issue("1");
    issue.body = "需要实现的功能描述。".into();
    issue.author.nickname = "用于验证信息卡片不会被长名称撑开的开发者".repeat(6);
    issue.assignees = vec![issue.author.clone(); 3];
    finish_issue_request(
        &mut presenter,
        IssueProvider::Cnb,
        Response::Detail(Ok(issue.clone())),
    );
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    for (width, theme, language) in [
        (1040., ThemePreference::Light, Language::English),
        (1280., ThemePreference::Dark, Language::Chinese),
    ] {
        cx.simulate_resize(gpui::size(px(width), px(900.)));
        view.update_in(cx, |view, window, cx| {
            view.set_appearance(
                AppearanceSettings {
                    theme,
                    reduced_motion: true,
                    ..Default::default()
                },
                window,
                cx,
            );
            view.set_language(language, window, cx);
        });
        cx.run_until_parked();
        let summary = cx.debug_bounds("cnb-issue-summary").unwrap();
        let metadata = cx.debug_bounds("cnb-issue-metadata").unwrap();
        let actions = cx.debug_bounds("cnb-issue-actions").unwrap();
        let page = cx.debug_bounds("cnb-page").unwrap();
        assert!(summary.left() > page.left() && summary.right() < page.right());
        assert!(metadata.bottom() <= actions.top());
        let mut fields = Vec::new();
        for index in 0..6 {
            let field = cx
                .debug_bounds(format!("cnb-issue-field-{index}").leak())
                .unwrap();
            let value = cx
                .debug_bounds(format!("cnb-issue-field-value-{index}").leak())
                .unwrap();
            assert!(field.left() >= metadata.left() && field.right() <= metadata.right());
            assert!(field.top() >= metadata.top() && field.bottom() <= metadata.bottom());
            assert!(value.left() >= field.left() && value.right() <= field.right());
            assert!(fields.iter().all(|previous| !field.intersects(previous)));
            fields.push(field);
        }
        let mut buttons = Vec::new();
        for selector in ["cnb-chat", "cnb-assign-self", "cnb-change-state", "cnb-npc"] {
            let button = cx.debug_bounds(selector).unwrap();
            assert_eq!(button.size.height, px(CONTROL_HEIGHT));
            assert!(button.left() >= actions.left() && button.right() <= actions.right());
            assert!(button.top() >= actions.top() && button.bottom() <= actions.bottom());
            assert!(buttons.iter().all(|previous| !button.intersects(previous)));
            buttons.push(button);
        }
        assert!(summary.bottom() <= cx.debug_bounds("cnb-issue-body").unwrap().top());
    }
    cx.simulate_resize(gpui::size(px(1120.), px(900.)));
    click_debug(cx, "cnb-chat");
    view.update(cx, |view, _| assert!(view.presenter.model().cnb.opened));
    assert!(cx.debug_bounds("issue-launch-surface").is_none());
    view.update(cx, |view, cx| {
        let mut comment = cnb_comment("1");
        comment.body = "完整验收条件，包含 **格式**。".into();
        finish_issue_request(
            &mut view.presenter,
            IssueProvider::Cnb,
            Response::Comments(Ok(vec![comment])),
        );
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("cnb-comment-1").is_some());
    click_debug(cx, "cnb-assign-self");
    view.update(cx, |view, cx| {
        assert_eq!(
            view.presenter.model().cnb.action_request.unwrap().1,
            IssueAction::AssignSelf
        );
        finish_issue_request(
            &mut view.presenter,
            IssueProvider::Cnb,
            Response::Action(Ok(ActionResult::Updated(Box::new(issue.clone())))),
        );
        cx.notify();
    });
    cx.run_until_parked();
    for state in [IssueFilter::Closed, IssueFilter::Open] {
        click_debug(cx, "cnb-change-state");
        view.update(cx, |view, cx| {
            assert_eq!(
                view.presenter.model().cnb.action_request.unwrap().1,
                IssueAction::SetState(state)
            );
            issue.state = state.state().into();
            finish_issue_request(
                &mut view.presenter,
                IssueProvider::Cnb,
                Response::Action(Ok(ActionResult::Updated(Box::new(issue.clone())))),
            );
            cx.notify();
        });
        cx.run_until_parked();
    }
    click_debug(cx, "cnb-npc");
    view.update(cx, |view, cx| {
        assert_eq!(view.presenter.model().cnb.action_request.unwrap().1, IssueAction::StartNpc);
        let comment = serde_json::from_value(serde_json::json!({"id":"987", "body":"@CodeBuddy 请处理", "statuses":{
            "npc":[{"statuses":[{"target_url":"https://cnb.cool/team/project/-/build/logs/cnb-1"}]}]
        }})).unwrap();
        finish_issue_request(&mut view.presenter, IssueProvider::Cnb, Response::Action(Ok(ActionResult::Npc(comment))));
        let mut comment = cnb_comment("1");
        comment.body = "完整验收条件，包含 **格式**。".into();
        finish_issue_request(&mut view.presenter, IssueProvider::Cnb, Response::Comments(Ok(vec![comment, cnb_comment("987")])));
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("cnb-npc-action").is_some());
    click_debug(cx, "cnb-chat");
    cx.run_until_parked();
    assert!(cx.debug_bounds("issue-launch-surface").is_some());
    assert!(cx.debug_bounds("cnb-page").is_some());
    assert!(cx.debug_bounds("composer-surface").is_none());
    view.update_in(cx, |view, window, cx| {
        view.issue_launch_input
            .update(cx, |input, cx| input.set_value("补充上下文", window, cx));
    });
    cx.run_until_parked();
    click_debug(cx, "issue-launch-start");
    cx.run_until_parked();
    assert!(cx.debug_bounds("issue-launch-surface").is_none());
    assert!(cx.debug_bounds("cnb-page").is_none());
    assert!(cx.debug_bounds("composer-surface").is_some());
    view.update(cx, |view, _| {
        let prompt = view
            .presenter
            .model()
            .conversation
            .messages
            .iter()
            .find(|message| message.role == MessageRole::User)
            .map(|message| message.content.clone())
            .expect("user prompt");
        for content in [
            "https://cnb.cool/team/project/-/issues/1",
            "需要实现的功能描述。",
            "完整验收条件，包含 **格式**。",
            "## 补充信息",
            "补充上下文",
        ] {
            assert!(prompt.contains(content), "{content}");
        }
        assert!(view.presenter.model().conversation.active_run.is_some());
        assert!(view.presenter.model().opened_issues().is_none());
    });
}

#[gpui::test]
fn issue_launch_dialog_opens_and_launches_while_a_session_is_executing(
    cx: &mut gpui::TestAppContext,
) {
    use crate::{
        infrastructure::issues::Response,
        presenter::tests::{
            cnb_comment, cnb_issue, finish_issue_request, finish_workspace_operation, seed_issues,
            start_test_worktree, worktree_fixture,
        },
    };
    // Media loading uses Tokio workers outside GPUI's deterministic test scheduler.
    cx.executor().allow_parking();
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, runner, _directory, first) = worktree_fixture("正在执行的任务");
    presenter.new_task();
    let second = start_test_worktree(&mut presenter, &runner, "第二个任务");
    presenter.new_task();
    let third = start_test_worktree(&mut presenter, &runner, "第三个任务");
    presenter.select_task(first.task_id);
    seed_issues(&mut presenter, IssueProvider::Cnb);
    presenter.open_issues(IssueProvider::Cnb);
    presenter.select_issue(IssueProvider::Cnb, "1".into());
    finish_issue_request(
        &mut presenter,
        IssueProvider::Cnb,
        Response::Detail(Ok(cnb_issue("1"))),
    );
    finish_issue_request(
        &mut presenter,
        IssueProvider::Cnb,
        Response::Comments(Ok(vec![cnb_comment("1")])),
    );
    assert_eq!(
        presenter.model().conversation.active_run,
        Some(first.run_id)
    );
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    cx.simulate_resize(gpui::size(px(1120.), px(900.)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("cnb-page").is_some());
    assert!(cx.debug_bounds("cnb-detail").is_some());
    click_debug(cx, "cnb-chat");
    cx.run_until_parked();
    assert!(cx.debug_bounds("issue-launch-surface").is_some());
    assert!(cx.debug_bounds("issue-launch-workspace").is_some());
    assert!(cx.debug_bounds("workspace-mode").is_some());
    assert!(cx.debug_bounds("workspace-base").is_some());
    click_debug(cx, "composer-model");
    cx.run_until_parked();
    assert!(cx.debug_bounds("model-picker-surface").is_some());
    click_debug(cx, "model-config-codex-cli");
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.presenter.model().conversation.selected_harness,
            HarnessKind::Codex
        );
        assert!(view.presenter.model().conversation.active_run.is_none());
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    click_debug(cx, "issue-launch-cancel");
    cx.run_until_parked();
    assert!(cx.debug_bounds("issue-launch-surface").is_none());
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.presenter.model().conversation.active_run,
            Some(first.run_id)
        );
        assert_eq!(
            view.presenter.model().conversation.selected_harness,
            HarnessKind::Claude
        );
    });
    click_debug(cx, "cnb-back");
    cx.run_until_parked();
    click_debug(cx, "cnb-new-issue");
    cx.run_until_parked();
    assert!(cx.debug_bounds("issue-launch-surface").is_some());
    assert!(cx.debug_bounds("issue-launch-card").is_some());
    click_debug(cx, "composer-model");
    cx.run_until_parked();
    click_debug(cx, "model-config-codex-cli");
    cx.run_until_parked();
    runner.emit(Event::HarnessDetected(nexus_protocol::HarnessProbe {
        harness: HarnessKind::Codex,
        available: true,
        authenticated: true,
        executable: "/fake/codex".into(),
        version: Some("1.2.3".into()),
        message: "ready".into(),
    }));
    view.update(cx, |view, cx| {
        let ModelCatalogState::Loading { request_id, .. } =
            view.presenter.model().conversation.model_catalog
        else {
            panic!("Codex catalog request")
        };
        runner.emit(Event::ModelCatalogLoaded {
            request_id,
            harness: HarnessKind::Codex,
            models: vec![],
        });
        view.presenter.drain_events();
        cx.notify();
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    view.update_in(cx, |view, window, cx| {
        view.issue_launch_input.update(cx, |input, cx| {
            input.set_value("并行提交的缺陷", window, cx)
        });
    });
    cx.run_until_parked();
    click_debug(cx, "issue-launch-start");
    cx.run_until_parked();
    assert!(cx.debug_bounds("issue-launch-surface").is_none());
    // A worktree launch defers the run until the task directory's model
    // catalog responds, mirroring the manual start flow.
    view.update(cx, |view, _| {
        finish_workspace_operation(&mut view.presenter);
        view.presenter.drain_events();
    });
    let pending = view.read_with(cx, |view, _| {
        let model = view.presenter.model();
        match &model.conversation.model_catalog {
            ModelCatalogState::Loading { request_id, .. } => {
                Some((*request_id, model.conversation.selected_harness))
            }
            _ => None,
        }
        .expect("catalog request for the new worktree")
    });
    runner.emit(Event::ModelCatalogLoaded {
        request_id: pending.0,
        harness: pending.1,
        models: vec![],
    });
    view.update(cx, |view, _| {
        view.presenter.drain_events();
        assert_eq!(view.presenter.model().active_run_count(), 4);
        assert_eq!(
            view.presenter.model().conversation.selected_harness,
            HarnessKind::Codex
        );
        assert_ne!(
            view.presenter.model().conversation.active_run,
            Some(first.run_id)
        );
        let prompt = view
            .presenter
            .model()
            .conversation
            .messages
            .iter()
            .find(|message| message.role == MessageRole::User)
            .map(|message| message.content.clone())
            .expect("user prompt");
        assert!(prompt.contains("并行提交的缺陷"));
        for task in [first, second, third] {
            view.presenter.select_task(task.task_id);
            assert_eq!(
                view.presenter.model().conversation.active_run,
                Some(task.run_id)
            );
            assert_eq!(
                view.presenter.model().working_directory(),
                Some(task.cwd.as_str())
            );
            assert!(!view.presenter.model().conversation.run_cancelling);
        }
    });
}

#[gpui::test]
fn github_quick_issue_keeps_the_draft_when_the_local_checkout_is_occupied(
    cx: &mut gpui::TestAppContext,
) {
    use crate::{
        model::workspace::WorkspaceKind,
        presenter::tests::{finish_workspace_operation, seed_issues, worktree_fixture},
    };
    cx.executor().allow_parking();
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, runner, _directory, first) = worktree_fixture("Worktree task");
    presenter.new_task();
    presenter.select_workspace_kind(WorkspaceKind::Local);
    assert!(presenter.submit("local task", "claude"));
    let local_run = presenter.model().conversation.active_run.unwrap();
    seed_issues(&mut presenter, IssueProvider::GitHub);
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    cx.simulate_resize(gpui::size(px(1120.), px(900.)));
    cx.run_until_parked();
    click_debug(cx, "sidebar-github");
    cx.run_until_parked();
    click_debug(cx, "github-new-issue");
    cx.run_until_parked();
    assert!(cx.debug_bounds("issue-launch-surface").is_some());
    view.update_in(cx, |view, window, cx| {
        assert_eq!(
            view.presenter.model().conversation.workspace_draft.kind,
            WorkspaceKind::Worktree
        );
        view.issue_launch_input.update(cx, |input, cx| {
            input.set_value("保留 Issue 草稿", window, cx)
        });
    });
    cx.run_until_parked();
    click_debug(cx, "workspace-mode");
    cx.run_until_parked();
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    assert!(cx.debug_bounds("issue-launch-blocker").is_some());
    click_debug(cx, "issue-launch-start");
    cx.run_until_parked();
    view.read_with(cx, |view, cx| {
        assert_eq!(view.issue_launch_input.read(cx).value(), "保留 Issue 草稿");
        assert_eq!(view.presenter.model().active_run_count(), 2);
        assert!(
            view.presenter
                .issue_run_blocker()
                .unwrap()
                .render(Language::Chinese)
                .contains("本地目录已有任务")
        );
    });
    click_debug(cx, "workspace-mode");
    cx.run_until_parked();
    cx.simulate_keystrokes("down down enter");
    cx.run_until_parked();
    assert!(cx.debug_bounds("issue-launch-blocker").is_none());
    click_debug(cx, "issue-launch-start");
    cx.run_until_parked();
    assert!(cx.debug_bounds("issue-launch-surface").is_none());
    view.update(cx, |view, cx| {
        finish_workspace_operation(&mut view.presenter);
        if let ModelCatalogState::Loading { request_id, .. } =
            view.presenter.model().conversation.model_catalog
        {
            runner.emit(Event::ModelCatalogLoaded {
                request_id,
                harness: HarnessKind::Claude,
                models: vec![],
            });
        }
        view.presenter.drain_events();
        assert_eq!(view.presenter.model().active_run_count(), 3);
        assert!(
            view.presenter
                .model()
                .conversation
                .messages
                .iter()
                .any(|message| message.content.contains("保留 Issue 草稿"))
        );
        view.presenter.select_task(first.task_id);
        assert_eq!(
            view.presenter.model().conversation.active_run,
            Some(first.run_id)
        );
        assert_eq!(
            view.presenter.model().working_directory(),
            Some(first.cwd.as_str())
        );
        assert!(
            view.presenter
                .model()
                .all_conversations()
                .any(|conversation| conversation.active_run == Some(local_run))
        );
        cx.notify();
    });
}

#[gpui::test]
fn cnb_navigation_renders_issues_details_and_pagination_without_a_composer(
    cx: &mut gpui::TestAppContext,
) {
    use crate::{
        infrastructure::issues::Response,
        model::issues::{IssueFilter, IssuePage},
        presenter::tests::{cnb_issue, finish_issue_request, seed_issues},
    };
    // Media loading uses Tokio workers outside GPUI's deterministic test scheduler.
    cx.executor().allow_parking();
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, _, _directory) = fixture();
    let project = presenter
        .model()
        .conversation
        .selected_project
        .as_ref()
        .unwrap()
        .id;
    seed_issues(&mut presenter, IssueProvider::Cnb);
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    cx.simulate_resize(gpui::size(px(1040.), px(720.)));
    view.update_in(cx, |view, window, cx| {
        view.set_appearance(
            AppearanceSettings {
                theme: ThemePreference::Light,
                reduced_motion: true,
                ..Default::default()
            },
            window,
            cx,
        );
        view.prompt_input
            .update(cx, |input, cx| input.set_value("保留会话草稿", window, cx));
    });
    cx.run_until_parked();
    let project_selector: &'static str = format!("sidebar-project-{project}").leak();
    let project_row = cx.debug_bounds(project_selector).unwrap();
    let cnb_row = cx.debug_bounds("sidebar-cnb").unwrap();
    assert!(cnb_row.top() >= project_row.bottom());
    assert_eq!(cnb_row.size.height, px(40.));
    click_debug(cx, "sidebar-cnb");
    cx.run_until_parked();
    assert!(cx.debug_bounds("cnb-page").is_some());
    assert!(cx.debug_bounds("cnb-issue-tab").is_some());
    let assert_breadcrumb = |cx: &mut gpui::VisualTestContext, segment_count| {
        let breadcrumb = cx.debug_bounds("cnb-breadcrumb").unwrap();
        let mut previous = cx.debug_bounds("cnb-breadcrumb-root").unwrap();
        let first_separator = cx.debug_bounds("cnb-breadcrumb-separator-0").unwrap();
        let gap = first_separator.left() - previous.right();
        assert!(gap > px(0.));
        for index in 0..segment_count {
            let separator = cx
                .debug_bounds(format!("cnb-breadcrumb-separator-{index}").leak())
                .unwrap();
            let segment = cx
                .debug_bounds(format!("cnb-breadcrumb-segment-{index}").leak())
                .unwrap();
            assert_eq!(separator.size, first_separator.size);
            assert_eq!(separator.left() - previous.right(), gap);
            assert_eq!(segment.left() - separator.right(), gap);
            assert_eq!(segment.center().y, previous.center().y);
            assert_eq!(segment.center().y, separator.center().y);
            assert!(segment.size.width > px(0.));
            assert!(segment.right() <= breadcrumb.right());
            previous = segment;
        }
        let repository_button = cx.debug_bounds("cnb-repository").unwrap();
        assert!(repository_button.left() >= breadcrumb.right());
        assert!(repository_button.right() < cx.debug_bounds("cnb-page").unwrap().right());
    };
    assert_breadcrumb(cx, 2);
    click_debug(cx, "cnb-repository");
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://cnb.cool/team/project")
    );
    let icon = cx.debug_bounds("sidebar-cnb-icon").unwrap();
    assert!(icon.left() >= cnb_row.left() && icon.right() < cnb_row.left() + px(34.));
    assert!(cx.debug_bounds("composer-surface").is_none());
    let issue = cx.debug_bounds("cnb-issue-1").unwrap();
    let page = cx.debug_bounds("cnb-page").unwrap();
    assert!(issue.left() > page.left() && issue.right() < page.right());
    click_debug(cx, "cnb-issue-1");
    view.update(cx, |view, cx| {
        let mut issue = cnb_issue("1");
        issue.body = "截图说明\n\n![截图](https://example.test/screenshot.png)\n\n[录屏.mp4](undefined/team/repo/-/files/issues/1/clip.mp4)\n\nhttps://example.test/sound.mp3\n\n[普通链接](https://example.test/page)\n\n```text\nhttps://example.test/code.mp4\n```".into();
        finish_issue_request(&mut view.presenter, IssueProvider::Cnb, Response::Detail(Ok(issue)));
        finish_issue_request(
            &mut view.presenter,
            IssueProvider::Cnb,
            Response::Comments(Ok(Vec::new())),
        );
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("cnb-detail").is_some());
    assert!(cx.debug_bounds("cnb-issue-body").is_some());
    assert!(cx.debug_bounds("cnb-media-image").is_some());
    assert!(cx.debug_bounds("cnb-media-video").is_some());
    assert!(cx.debug_bounds("cnb-media-audio").is_some());
    assert!(cx.debug_bounds("cnb-native-player-visible").is_some());
    click_debug(cx, "cnb-chat");
    cx.run_until_parked();
    assert!(cx.debug_bounds("issue-launch-surface").is_some());
    assert!(cx.debug_bounds("cnb-native-player-visible").is_none());
    assert!(cx.debug_bounds("cnb-native-player-obscured").is_some());
    click_debug(cx, "issue-launch-close");
    cx.run_until_parked();
    assert!(cx.debug_bounds("issue-launch-surface").is_none());
    assert!(cx.debug_bounds("cnb-native-player-visible").is_some());
    click_debug(cx, "cnb-back");
    cx.run_until_parked();
    assert!(cx.debug_bounds("cnb-issue-1").is_some());
    assert!(cx.debug_bounds("cnb-media-video").is_none());
    click_debug(cx, "cnb-next");
    view.update(cx, |view, cx| {
        assert_eq!(view.presenter.model().cnb.page, 2);
        finish_issue_request(
            &mut view.presenter,
            IssueProvider::Cnb,
            Response::List(Ok(IssuePage {
                next_cursor: None,
                issues: vec![cnb_issue("31")],
                total: 61,
            })),
        );
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("cnb-issue-31").is_some());
    click_debug(cx, "cnb-filter-closed");
    view.update_in(cx, |view, window, cx| {
        assert_eq!(view.presenter.model().cnb.filter, IssueFilter::Closed);
        assert_eq!(view.presenter.model().cnb.page, 1);
        finish_issue_request(
            &mut view.presenter,
            IssueProvider::Cnb,
            Response::List(Ok(IssuePage {
                next_cursor: None,
                issues: vec![],
                total: 0,
            })),
        );
        view.set_appearance(
            AppearanceSettings {
                theme: ThemePreference::Dark,
                reduced_motion: true,
                ..Default::default()
            },
            window,
            cx,
        );
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("cnb-empty").is_some());
    let nested_repository = "organization-with-a-long-name/subgroup/project-with-a-long-name";
    view.update(cx, |view, cx| {
        let cli = view.presenter.model().cnb.cli.clone().unwrap();
        view.presenter.inspect_issues(IssueProvider::Cnb);
        finish_issue_request(
            &mut view.presenter,
            IssueProvider::Cnb,
            Response::Inspection {
                repository: Some(nested_repository.into()),
                cli: Ok(cli),
            },
        );
        view.presenter.open_issues(IssueProvider::Cnb);
        cx.notify();
    });
    cx.simulate_resize(gpui::size(px(760.), px(720.)));
    cx.run_until_parked();
    assert_breadcrumb(cx, 3);
    click_debug(cx, "cnb-repository");
    assert_eq!(
        cx.opened_url(),
        Some(format!("https://cnb.cool/{nested_repository}"))
    );
    view.update_in(cx, |view, window, cx| {
        view.new_task(window, cx);
        assert_eq!(view.prompt_input.read(cx).value(), "保留会话草稿");
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("cnb-page").is_none());
    assert!(cx.debug_bounds("composer-surface").is_some());
    view.update_in(cx, |view, window, cx| {
        view.settings_open = true;
        view.select_settings_section(SettingsSection::SourceControl, window, cx);
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("settings-nav-source-control").is_some());
    assert!(cx.debug_bounds("cnb-enabled").is_some());
}
