use super::*;

#[gpui::test]
fn task_rows_fill_the_same_width_and_accept_clicks_past_short_titles(cx: &mut TestAppContext) {
    let (view, cx) = scroll_test_view(cx);
    let tasks = view.read_with(cx, |view, _| {
        view.presenter.model().conversation.tasks[..2].to_vec()
    });
    let bounds = tasks
        .iter()
        .map(|task| {
            let selector = format!("sidebar-task-{}", task.id).leak();
            cx.debug_bounds(selector).unwrap()
        })
        .collect::<Vec<_>>();
    assert_ne!(tasks[0].title.len(), tasks[1].title.len());
    assert_eq!(bounds[0].left(), bounds[1].left());
    assert_eq!(bounds[0].size.width, bounds[1].size.width);
    assert_eq!(
        bounds[0].right(),
        cx.debug_bounds("add-project").unwrap().right()
    );
    let short_index = tasks.iter().position(|task| task.title == "Hi").unwrap();
    view.update(cx, |view, cx| {
        view.presenter.select_task(tasks[1 - short_index].id);
        cx.notify();
    });
    cx.run_until_parked();
    let action_selector = format!("task-actions-{}", tasks[short_index].id).leak();
    let action_bounds = cx.debug_bounds(action_selector).unwrap();
    cx.simulate_click(
        point(
            action_bounds.left() - px(8.),
            bounds[short_index].center().y,
        ),
        Default::default(),
    );
    assert_eq!(
        view.read_with(cx, |view, _| view
            .presenter
            .model()
            .conversation
            .selected_task),
        Some(tasks[short_index].id),
    );
}

#[gpui::test]
fn supported_window_sizes_keep_workspace_regions_and_header_context_separate(
    cx: &mut TestAppContext,
) {
    let (view, cx) = scroll_test_view(cx);
    for size in [
        gpui::size(px(1040.), px(680.)),
        gpui::size(px(1280.), px(800.)),
    ] {
        cx.simulate_resize(size);
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });

        let page = cx.debug_bounds("workspace-page").unwrap();
        let sidebar = cx.debug_bounds("workspace-sidebar").unwrap();
        let header = cx.debug_bounds("workspace-header").unwrap();
        let timeline = view.read_with(cx, |view, _| view.timeline_scroll.bounds());
        let composer = cx.debug_bounds("composer-surface").unwrap();
        let project = cx.debug_bounds("workspace-header-project").unwrap();
        let task = cx.debug_bounds("workspace-header-task").unwrap();
        let settings = cx.debug_bounds("open-settings").unwrap();

        for bounds in [sidebar, header, timeline, composer, project, task, settings] {
            assert!(
                bounds.left() >= page.left()
                    && bounds.right() <= page.right()
                    && bounds.top() >= page.top()
                    && bounds.bottom() <= page.bottom(),
                "{bounds:?} must stay inside {page:?} at {size:?}"
            );
        }
        assert!(sidebar.right() <= header.left());
        assert!(header.bottom() <= timeline.top());
        assert!(timeline.bottom() <= composer.top());
        assert!(project.right() <= task.left());
        assert!(task.right() <= settings.left());
        assert!(cx.debug_bounds("workspace-header-status").is_none());
        assert!(composer.left() >= timeline.left());
        assert!(composer.right() <= timeline.right());
    }
}

#[gpui::test]
fn background_run_does_not_add_operation_status_to_the_workspace(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, runner, _directory) = crate::presenter::tests::fixture();
    assert!(presenter.submit("inactive conversation", "claude"));
    let inactive_task = presenter.model().conversation.active_task.unwrap();
    runner.emit(Event::RunExited {
        run_id: presenter.model().conversation.active_run.unwrap(),
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    presenter.new_task();
    assert!(presenter.submit("background conversation", "claude"));
    let active_task = presenter.model().conversation.active_task.unwrap();
    presenter.select_task(inactive_task);
    assert_ne!(
        presenter.model().conversation.selected_task,
        Some(active_task)
    );

    let (_view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });

    assert!(
        cx.debug_bounds("workspace-header-background-status")
            .is_none()
    );
    assert!(cx.debug_bounds("workspace-header-task").is_some());
    assert!(cx.debug_bounds("conversation-run-status").is_none());
}
