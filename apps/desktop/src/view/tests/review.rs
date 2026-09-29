use super::*;

#[gpui::test]
fn workspace_review_uses_full_page_preserves_drafts_and_navigates_files(
    cx: &mut gpui::TestAppContext,
) {
    use crate::{
        infrastructure::git,
        presenter::tests::{finish_workspace_operation, seed_issues, worktree_fixture},
    };
    use gpui::{ScrollDelta, ScrollWheelEvent, point};
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, runner, _directory, start) = worktree_fixture("review task");
    presenter.set_appearance(AppearanceSettings {
        reduced_motion: true,
        ..Default::default()
    });
    runner.emit(Event::RunExited {
        run_id: start.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    let cwd = Path::new(&start.cwd);
    std::fs::write(
        cwd.join("tracked.txt"),
        format!("{}\n", "long diff content ".repeat(30)).repeat(80),
    )
    .unwrap();
    std::fs::write(cwd.join("staged.rs"), "fn staged() {}\n").unwrap();
    git::git(cwd, &["add", "staged.rs"]).unwrap();
    std::fs::create_dir(cwd.join("docs")).unwrap();
    std::fs::write(cwd.join("docs/新文件.md"), "# New document\n").unwrap();
    presenter.toggle_changes_sidebar();
    finish_workspace_operation(&mut presenter);
    presenter.select_changed_file("tracked.txt".into(), true);
    presenter.set_commit_message("Reviewed draft".into());
    seed_issues(&mut presenter, IssueProvider::Cnb);
    let original = presenter
        .model()
        .conversation
        .workspace_review
        .as_ref()
        .unwrap()
        .unstaged
        .clone();
    let id = presenter
        .model()
        .conversation
        .workspace_review
        .as_ref()
        .unwrap()
        .workspace_id;
    let (root, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| NexusView::new(presenter, window, cx));
        gpui_kit::component::Root::new(view, window, cx)
    });
    let view = root.read_with(cx, |root, _| {
        root.view().clone().downcast::<NexusView>().unwrap()
    });
    let draw = |cx: &mut gpui::VisualTestContext| {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
    };
    cx.simulate_resize(gpui::size(px(1040.), px(680.)));
    draw(cx);
    click_debug(cx, "environment-changes");
    draw(cx);
    click_debug(cx, "show-workspace-diff");
    draw(cx);
    for (width, height, language) in [
        (1040., 680., Language::Chinese),
        (1440., 900., Language::English),
    ] {
        cx.simulate_resize(gpui::size(px(width), px(height)));
        view.update_in(cx, |view, window, cx| {
            view.set_language(language, window, cx)
        });
        draw(cx);
        let page = cx.debug_bounds("workspace-review-page").unwrap();
        let navigation = cx.debug_bounds("review-file-list").unwrap();
        let code = cx
            .debug_bounds(format!("review-diff-{id}-Unstaged-tracked.txt").leak())
            .unwrap();
        assert!(page.right() <= px(width) && page.bottom() <= px(height));
        assert!(
            code.size.height > px(height * 0.8),
            "review must use available height: {code:?}"
        );
        assert!(code.size.width > px(480.) && code.left() >= navigation.right());
        assert!(cx.debug_bounds("composer-surface").is_none());
        assert!(cx.debug_bounds("workspace-header-status").is_none());
        assert!(cx.debug_bounds("review-status").is_none());
        assert!(cx.debug_bounds("review-merge-panel").is_none());
    }
    let code_key: &'static str = format!("review-diff-{id}-Unstaged-tracked.txt").leak();
    let viewport = cx.debug_bounds(code_key).unwrap();
    for delta in [point(px(-80.), px(0.)), point(px(0.), px(-60.))] {
        let before = cx
            .debug_bounds(format!("{code_key}-content").leak())
            .unwrap();
        cx.simulate_event(ScrollWheelEvent {
            position: viewport.center(),
            delta: ScrollDelta::Pixels(delta),
            ..Default::default()
        });
        draw(cx);
        let after = cx
            .debug_bounds(format!("{code_key}-content").leak())
            .unwrap();
        assert_eq!(after.origin, before.origin + delta);
    }
    click_debug(cx, format!("{code_key}-copy").leak());
    cx.update(|_, cx| assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), original));
    click_debug(cx, "review-nav-Untracked-docs/新文件.md");
    draw(cx);
    assert!(
        cx.debug_bounds(format!("review-diff-{id}-Untracked-docs/新文件.md").leak())
            .is_some()
    );
    click_debug(cx, "sidebar-cnb");
    draw(cx);
    assert!(cx.debug_bounds("cnb-page").is_some());
    assert!(cx.debug_bounds("workspace-review-page").is_none());
    assert!(cx.debug_bounds("composer-surface").is_none());
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(cx.debug_bounds("cnb-page").is_some());
    view.update_in(cx, |view, window, cx| {
        view.select_task(start.task_id, window, cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("cnb-page").is_none());
    assert!(cx.debug_bounds("workspace-review-page").is_some());
    assert!(
        cx.debug_bounds(format!("review-diff-{id}-Untracked-docs/新文件.md").leak())
            .is_some()
    );
    std::fs::write(cwd.join("docs/新文件.md"), "# Updated document\n").unwrap();
    click_debug(cx, "refresh-workspace-review");
    view.update(cx, |view, cx| {
        finish_workspace_operation(&mut view.presenter);
        cx.notify();
    });
    draw(cx);
    click_debug(
        cx,
        format!("review-diff-{id}-Untracked-docs/新文件.md-copy").leak(),
    );
    cx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "# Updated document\n"
        )
    });
    click_debug(cx, "close-workspace-review");
    draw(cx);
    assert!(cx.debug_bounds("composer-surface").is_some());
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.presenter.model().conversation.commit_message,
            "Reviewed draft"
        );
        assert_eq!(
            view.presenter
                .model()
                .conversation
                .selected_changes
                .iter()
                .collect::<Vec<_>>(),
            vec!["tracked.txt"]
        );
    });
    click_debug(cx, "open-commit-editor");
    draw(cx);
    assert_eq!(
        cx.debug_bounds("generate-commit-message")
            .unwrap()
            .size
            .width,
        px(28.)
    );
}

#[gpui::test]
fn workspace_review_keeps_merge_preview_and_confirmation_on_the_page(
    cx: &mut gpui::TestAppContext,
) {
    use crate::{
        infrastructure::git,
        presenter::tests::{finish_workspace_operation, worktree_fixture},
    };
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, runner, _directory, start) = worktree_fixture("merge task");
    presenter.set_appearance(AppearanceSettings {
        reduced_motion: true,
        ..Default::default()
    });
    runner.emit(Event::RunExited {
        run_id: start.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    let cwd = Path::new(&start.cwd);
    std::fs::write(cwd.join("tracked.txt"), "reviewed merge\n").unwrap();
    git::git(cwd, &["commit", "-am", "reviewed change"]).unwrap();
    let task_status = presenter
        .model()
        .latest_log_text(presenter.model().language)
        .to_owned();
    let project = presenter
        .model()
        .conversation
        .selected_project
        .clone()
        .unwrap();
    let (root, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| NexusView::new(presenter, window, cx));
        gpui_kit::component::Root::new(view, window, cx)
    });
    let view = root.read_with(cx, |root, _| {
        root.view().clone().downcast::<NexusView>().unwrap()
    });
    cx.simulate_resize(gpui::size(px(1040.), px(680.)));
    view.update_in(cx, |view, window, cx| {
        view.open_workspace_review(start.task_id, window, cx);
        finish_workspace_operation(&mut view.presenter);
        cx.notify();
    });
    let draw = |cx: &mut gpui::VisualTestContext| {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
    };
    draw(cx);
    click_debug(cx, "review-merge-options");
    draw(cx);
    click_debug(cx, "preview-workspace-merge");
    view.update(cx, |view, cx| {
        finish_workspace_operation(&mut view.presenter);
        cx.notify();
    });
    draw(cx);
    assert!(
        cx.debug_bounds(format!("review-diff-{}-Merge-tracked.txt", start.task_id).leak())
            .is_some()
    );
    assert_eq!(
        std::fs::read_to_string(Path::new(&project.canonical_path).join("tracked.txt")).unwrap(),
        "base\n"
    );
    click_debug(cx, "confirm-workspace-merge");
    view.update(cx, |view, cx| {
        finish_workspace_operation(&mut view.presenter);
        cx.notify();
    });
    draw(cx);
    assert_eq!(
        std::fs::read_to_string(Path::new(&project.canonical_path).join("tracked.txt")).unwrap(),
        "reviewed merge\n"
    );
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.presenter
                .model()
                .latest_log_text(view.presenter.model().language),
            task_status
        )
    });
    assert!(cx.debug_bounds("review-status").is_some());
    assert!(cx.debug_bounds("workspace-review-page").is_some());
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(cx.debug_bounds("workspace-review-page").is_none());
    assert!(cx.debug_bounds("composer-surface").is_some());
}

#[gpui::test]
fn changes_sidebar_generates_editable_messages_and_requires_commit_confirmation(
    cx: &mut gpui::TestAppContext,
) {
    use crate::presenter::tests::{finish_workspace_operation, worktree_fixture};
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, runner, _directory, start) = worktree_fixture("review task");
    // Keep click targets fixed while testing the confirmation flow.
    assert!(presenter.set_appearance(AppearanceSettings {
        reduced_motion: true,
        ..Default::default()
    }));
    runner.emit(Event::RunExited {
        run_id: start.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    let cwd = std::path::Path::new(&start.cwd);
    // Opening a review must also notice a branch renamed after the run ended.
    crate::infrastructure::git::git(cwd, &["branch", "-m", "fix/review-task"]).unwrap();
    std::fs::write(cwd.join("tracked.txt"), "reviewed content\n").unwrap();
    let before = crate::infrastructure::git::git(cwd, &["rev-parse", "HEAD"]).unwrap();
    let (root, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| NexusView::new(presenter, window, cx));
        gpui_kit::component::Root::new(view, window, cx)
    });
    let view = root.read_with(cx, |root, _| {
        root.view().clone().downcast::<NexusView>().unwrap()
    });
    cx.simulate_resize(gpui::size(px(1040.), px(680.)));
    view.update_in(cx, |view, window, cx| {
        view.set_language(Language::English, window, cx);
        view.presenter.toggle_changes_sidebar();
        finish_workspace_operation(&mut view.presenter);
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    let content = cx.debug_bounds("conversation-right-sidebar").unwrap();
    assert!(content.size.height > px(0.));
    assert!(content.right() <= px(1040.));
    assert!(content.bottom() <= px(680.));
    assert!(cx.debug_bounds("composer-surface").unwrap().right() <= content.left());
    let card = cx.debug_bounds("environment-card").unwrap();
    assert!(card.size.height < px(240.));
    assert!(cx.debug_bounds("commit-editor").is_none());
    assert!(cx.debug_bounds("conversation-changed-files").is_none());
    click_debug(cx, "environment-changes");
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    click_debug(cx, "review-file-tracked.txt");
    click_debug(cx, "open-commit-editor");
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    click_debug(cx, "generate-commit-message");
    view.update(cx, |view, cx| {
        finish_workspace_operation(&mut view.presenter);
        let request_id = view
            .presenter
            .model()
            .conversation
            .commit_message_request
            .unwrap();
        runner.emit(Event::CommitMessageGenerated {
            request_id,
            message: "fix: Generated description\n\nGenerated body".into(),
        });
        view.presenter.drain_events();
        cx.notify();
    });
    cx.run_until_parked();
    view.update_in(cx, |view, window, cx| {
        let input = &view.commit_inputs[&view.presenter.model().conversation.id];
        assert_eq!(
            input.read(cx).value(),
            "fix: Generated description\n\nGenerated body"
        );
        input.update(cx, |input, cx| input.focus(window, cx));
    });
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a"
    } else {
        "ctrl-a"
    });
    cx.simulate_input("fix: Edited description\n\nReviewed body");
    cx.run_until_parked();
    click_debug(cx, "commit-workspace-files");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    assert_eq!(
        crate::infrastructure::git::git(cwd, &["rev-parse", "HEAD"]).unwrap(),
        before
    );
    click_debug(cx, "commit-workspace-files");
    cx.simulate_prompt_answer("Commit");
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        finish_workspace_operation(&mut view.presenter);
        assert!(!view.presenter.model().conversation.commit_editor_open);
        cx.notify();
    });
    assert_ne!(
        crate::infrastructure::git::git(cwd, &["rev-parse", "HEAD"]).unwrap(),
        before
    );
    assert_eq!(
        crate::infrastructure::git::git(cwd, &["log", "-1", "--format=%s"])
            .unwrap()
            .trim(),
        "fix: Edited description"
    );
    assert!(
        crate::infrastructure::git::git(cwd, &["status", "--porcelain"])
            .unwrap()
            .is_empty()
    );
    click_debug(cx, "environment-changes");
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    click_debug(cx, "open-commit-editor");
    assert!(cx.debug_bounds("commit-editor").is_none());
    assert!(cx.debug_bounds("generate-commit-message").is_none());
    assert!(cx.debug_bounds("environment-card").unwrap().size.height < px(260.));
}

#[gpui::test]
fn changes_sidebar_scrolls_many_files_without_squeezing_rows(cx: &mut gpui::TestAppContext) {
    use crate::presenter::tests::{finish_workspace_operation, worktree_fixture};
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, runner, _directory, start) = worktree_fixture("many changes");
    presenter.set_appearance(AppearanceSettings {
        reduced_motion: true,
        ..Default::default()
    });
    runner.emit(Event::RunExited {
        run_id: start.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    let cwd = Path::new(&start.cwd);
    let write_file = |name: &str| {
        let full = cwd.join(name);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, "fn changed() {}\n").unwrap();
    };
    write_file("first.rs");
    presenter.toggle_changes_sidebar();
    finish_workspace_operation(&mut presenter);
    let (root, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| NexusView::new(presenter, window, cx));
        gpui_kit::component::Root::new(view, window, cx)
    });
    let view = root.read_with(cx, |root, _| {
        root.view().clone().downcast::<NexusView>().unwrap()
    });
    cx.simulate_resize(gpui::size(px(1040.), px(680.)));
    let draw = |cx: &mut gpui::VisualTestContext| {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
    };
    draw(cx);
    click_debug(cx, "environment-changes");
    draw(cx);
    let baseline = cx.debug_bounds("change-row-first.rs").unwrap().size.height;
    assert!(baseline > px(0.));
    // More rows than the capped list can show at once must scroll, not shrink.
    let paths: Vec<String> = (0..24).map(|index| format!("f{index:02}.rs")).collect();
    for path in &paths {
        write_file(path);
    }
    click_debug(cx, "refresh-conversation-changes");
    view.update(cx, |view, cx| {
        finish_workspace_operation(&mut view.presenter);
        cx.notify();
    });
    draw(cx);
    let list = cx.debug_bounds("conversation-changed-files").unwrap();
    for path in paths
        .iter()
        .map(String::as_str)
        .chain(std::iter::once("first.rs"))
    {
        let selector: &'static str = format!("change-row-{path}").leak();
        let row = cx.debug_bounds(selector).unwrap();
        assert!(
            row.size.height >= baseline,
            "changed-file row was squeezed: {path} {row:?}"
        );
    }
    let last = cx
        .debug_bounds(format!("change-row-{}", paths.last().unwrap()).leak())
        .unwrap();
    assert!(
        last.bottom() > list.bottom(),
        "the file list must scroll instead of squeezing rows"
    );
}
