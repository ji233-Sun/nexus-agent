use super::*;

#[gpui::test]
fn empty_workspace_keeps_project_guidance_without_operation_status(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let directory = tempfile::tempdir().unwrap();
    let storage = Storage::open(Path::new(":memory:")).unwrap();
    let presenter = Presenter::new(storage, Err(anyhow::anyhow!("runner unavailable")), None);
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("workspace-empty-no-project").is_some());
    assert!(cx.debug_bounds("workspace-empty-agent-status").is_none());
    assert!(cx.debug_bounds("workspace-header-task").is_some());
    assert!(cx.debug_bounds("workspace-header-settled-status").is_none());
    assert!(cx.debug_bounds("workspace-header-pending-status").is_none());

    for size in [
        gpui::size(px(1040.), px(680.)),
        gpui::size(px(1280.), px(800.)),
    ] {
        cx.simulate_resize(size);
        cx.run_until_parked();
        let welcome = cx.debug_bounds("workspace-empty-no-project").unwrap();
        let choose_project = cx.debug_bounds("welcome-choose-project").unwrap();
        let timeline = view.read_with(cx, |view, _| view.timeline_scroll.bounds());
        assert!(choose_project.left() >= welcome.left());
        assert!(choose_project.right() <= welcome.right());
        assert!(choose_project.top() >= timeline.top());
        assert!(choose_project.bottom() <= timeline.bottom());
    }

    view.update(cx, |view, cx| {
        view.presenter.open_project(directory.path());
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("workspace-empty-no-project").is_none());
    assert!(cx.debug_bounds("workspace-empty-agent-status").is_some());
    assert!(cx.debug_bounds("workspace-empty-status").is_none());
    assert!(cx.debug_bounds("workspace-header-task").is_some());
    assert!(cx.debug_bounds("workspace-header-settled-status").is_none());
}

#[gpui::test]
fn task_menu_archives_and_settings_restores_the_conversation(cx: &mut TestAppContext) {
    let (view, cx) = scroll_test_view(cx);
    let task_id = view.read_with(cx, |view, _| {
        view.presenter.model().conversation.selected_task.unwrap()
    });
    let task_selector = format!("sidebar-task-{task_id}").leak();
    let actions_selector = format!("task-actions-{task_id}").leak();
    let actions = cx.debug_bounds(actions_selector).unwrap().center();

    cx.simulate_click(actions, Default::default());
    cx.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("animated-menu-surface").is_some());
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });
    view.read_with(cx, |view, _| {
        assert!(view.presenter.model().conversation.selected_task.is_none());
        assert_eq!(view.presenter.model().archived_tasks[0].id, task_id);
    });
    assert!(cx.debug_bounds(task_selector).is_none());

    let settings = cx.debug_bounds("open-settings").unwrap().center();
    cx.simulate_click(settings, Default::default());
    cx.run_until_parked();
    let archived = cx.debug_bounds("settings-nav-archived").unwrap().center();
    cx.simulate_click(archived, Default::default());
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });
    let restore_selector = format!("restore-archived-{task_id}").leak();
    let delete_selector = format!("delete-archived-{task_id}").leak();
    assert!(cx.debug_bounds(delete_selector).is_some());
    assert!(cx.debug_bounds("delete-all-archived").is_some());
    let restore = cx.debug_bounds(restore_selector).unwrap().center();
    cx.simulate_click(restore, Default::default());
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(view.presenter.model().archived_tasks.is_empty());
        assert!(
            view.presenter
                .model()
                .conversation
                .tasks
                .iter()
                .any(|task| task.id == task_id)
        );
    });
}

#[gpui::test]
fn project_deletion_menu_requires_confirmation_and_preserves_selection_on_cancel(
    cx: &mut TestAppContext,
) {
    let (view, cx) = scroll_test_view(cx);
    let (project, task_id) = view.read_with(cx, |view, _| {
        (
            view.presenter
                .model()
                .conversation
                .selected_project
                .clone()
                .unwrap(),
            view.presenter.model().conversation.selected_task,
        )
    });
    let project_selector = format!("sidebar-project-{}", project.id).leak();
    let actions_selector = format!("project-actions-{}", project.id).leak();
    let bounds = cx.debug_bounds(project_selector).unwrap();
    cx.simulate_mouse_move(bounds.center(), None, Default::default());
    let actions = cx.debug_bounds(actions_selector).unwrap().center();
    cx.simulate_click(actions, Default::default());
    cx.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });
    let menu = cx.debug_bounds("animated-menu-surface").unwrap().center();
    cx.simulate_mouse_move(menu, None, Default::default());
    cx.simulate_click(menu, Default::default());
    let (message, detail) = cx.pending_prompt().unwrap();
    assert_eq!(message, format!("删除项目“{}”？", project.display_name));
    assert!(detail.contains("含归档"));
    assert!(detail.contains("无法撤销"));
    assert!(detail.contains("Worktree、文件和 Git 分支不会删除"));
    cx.simulate_prompt_answer("取消");
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(view.presenter.model().conversation.selected_task, task_id);
        assert!(!view.collapsed_projects.contains(&project.id));
        assert_eq!(view.presenter.model().projects.len(), 1);
    });

    view.update_in(cx, |view, window, cx| {
        view.set_language(Language::English, window, cx);
        view.confirm_delete_project(project.id, window, cx);
    });
    let (message, detail) = cx.pending_prompt().unwrap();
    assert_eq!(
        message,
        format!("Delete project “{}”?", project.display_name)
    );
    assert!(detail.contains("files, and Git branches on disk will be kept"));
    cx.simulate_prompt_answer("Delete project");
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(view.presenter.model().projects.is_empty());
        assert!(
            view.presenter
                .model()
                .conversation
                .selected_project
                .is_none()
        );
        assert!(view.presenter.model().conversation.selected_task.is_none());
        assert!(view.presenter.model().conversation.messages.is_empty());
    });
    assert!(cx.debug_bounds(project_selector).is_none());
    assert!(cx.debug_bounds("add-project").is_some());
}

#[gpui::test]
fn permanent_task_deletion_requires_explicit_confirmation(cx: &mut TestAppContext) {
    let (view, cx) = scroll_test_view(cx);
    let (task_id, task_title) = view.read_with(cx, |view, _| {
        let task_id = view.presenter.model().conversation.selected_task.unwrap();
        let task_title = view
            .presenter
            .model()
            .conversation
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .unwrap()
            .title
            .clone();
        (task_id, task_title)
    });

    let actions_selector = format!("task-actions-{task_id}").leak();
    let actions = cx.debug_bounds(actions_selector).unwrap().center();
    cx.simulate_click(actions, Default::default());
    cx.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });
    cx.simulate_keystrokes("down down enter");
    assert!(cx.has_pending_prompt());
    let (message, detail) = cx.pending_prompt().unwrap();
    assert_eq!(message, format!("永久删除“{task_title}”？"));
    assert!(detail.contains("无法撤销"));
    assert!(view.read_with(cx, |view, _| {
        view.presenter
            .model()
            .conversation
            .tasks
            .iter()
            .any(|task| task.id == task_id)
    }));

    cx.simulate_prompt_answer("取消");
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| {
        view.presenter
            .model()
            .conversation
            .tasks
            .iter()
            .any(|task| task.id == task_id)
    }));

    view.update_in(cx, |view, window, cx| {
        view.archive_task(task_id, window, cx);
    });
    let settings = cx.debug_bounds("open-settings").unwrap().center();
    cx.simulate_click(settings, Default::default());
    cx.run_until_parked();
    let archived = cx.debug_bounds("settings-nav-archived").unwrap().center();
    cx.simulate_click(archived, Default::default());
    cx.run_until_parked();

    let delete_selector = format!("delete-archived-{task_id}").leak();
    let delete = cx.debug_bounds(delete_selector).unwrap().center();
    cx.simulate_click(delete, Default::default());
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("永久删除");
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| {
        view.presenter.model().archived_tasks.is_empty()
    }));
}

#[gpui::test]
fn clearing_archived_tasks_requires_counted_confirmation(cx: &mut TestAppContext) {
    let (view, cx) = scroll_test_view(cx);
    let task_ids = view.read_with(cx, |view, _| {
        view.presenter
            .model()
            .conversation
            .tasks
            .iter()
            .take(2)
            .map(|task| task.id)
            .collect::<Vec<_>>()
    });
    view.update_in(cx, |view, window, cx| {
        for task_id in task_ids {
            view.archive_task(task_id, window, cx);
        }
    });
    let settings = cx.debug_bounds("open-settings").unwrap().center();
    cx.simulate_click(settings, Default::default());
    cx.run_until_parked();
    let archived = cx.debug_bounds("settings-nav-archived").unwrap().center();
    cx.simulate_click(archived, Default::default());
    cx.run_until_parked();

    let clear = cx.debug_bounds("delete-all-archived").unwrap().center();
    cx.simulate_click(clear, Default::default());
    assert!(cx.has_pending_prompt());
    let (message, detail) = cx.pending_prompt().unwrap();
    assert_eq!(message, "永久删除 2 个归档对话？");
    assert!(detail.contains("这 2 个对话"));
    assert_eq!(
        view.read_with(cx, |view, _| view.presenter.model().archived_tasks.len()),
        2
    );

    cx.simulate_prompt_answer("取消");
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |view, _| view.presenter.model().archived_tasks.len()),
        2
    );

    let clear = cx.debug_bounds("delete-all-archived").unwrap().center();
    cx.simulate_click(clear, Default::default());
    cx.simulate_prompt_answer("全部删除");
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| {
        view.presenter.model().archived_tasks.is_empty()
    }));
}

#[gpui::test]
fn managing_another_task_preserves_the_current_timeline_state(cx: &mut TestAppContext) {
    let (view, cx) = scroll_test_view(cx);
    let (selected_task, other_tasks) = view.read_with(cx, |view, _| {
        let selected_task = view.presenter.model().conversation.selected_task.unwrap();
        let other_tasks = view
            .presenter
            .model()
            .conversation
            .tasks
            .iter()
            .filter(|task| task.id != selected_task)
            .take(2)
            .map(|task| task.id)
            .collect::<Vec<_>>();
        (selected_task, other_tasks)
    });
    let expanded_message = ElementId::from("expanded-message");
    view.update_in(cx, |view, window, cx| {
        view.timeline_scroll.set_offset(point(px(0.), px(-120.)));
        view.expanded_messages.insert(expanded_message.clone());
        view.search_input
            .update(cx, |input, cx| input.focus(window, cx));

        view.archive_task(other_tasks[0], window, cx);
        assert_eq!(
            view.presenter.model().conversation.selected_task,
            Some(selected_task)
        );
        assert_eq!(view.timeline_scroll.offset().y, px(-120.));
        assert!(view.expanded_messages.contains(&expanded_message));
        assert!(
            view.search_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );

        view.delete_task(other_tasks[1], window, cx);
        assert_eq!(
            view.presenter.model().conversation.selected_task,
            Some(selected_task)
        );
        assert_eq!(view.timeline_scroll.offset().y, px(-120.));
        assert!(view.expanded_messages.contains(&expanded_message));
        assert!(
            view.search_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
    });
}

#[gpui::test]
fn project_disclosure_animates_layout_reversibly_and_can_skip_motion(cx: &mut TestAppContext) {
    let (view, cx) = scroll_test_view(cx);
    let project_id = view.read_with(cx, |view, _| {
        view.presenter
            .model()
            .conversation
            .selected_project
            .as_ref()
            .unwrap()
            .id
    });
    let project_selector = format!("sidebar-project-{project_id}").leak();
    let project_bounds = cx.debug_bounds(project_selector).unwrap();
    let trigger = point(project_bounds.left() + px(60.), project_bounds.center().y);
    let selected_task = view.read_with(cx, |view, _| {
        view.presenter.model().conversation.selected_task
    });
    view.update_in(cx, |view, window, cx| {
        view.set_appearance(
            AppearanceSettings {
                reduced_motion: false,
                ..view.presenter.model().appearance
            },
            window,
            cx,
        );
    });
    let frame = |cx: &mut gpui::VisualTestContext, millis| {
        cx.executor().advance_clock(Duration::from_millis(millis));
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
    };
    frame(cx, 0);
    let expanded_y = cx.debug_bounds("add-project").unwrap().origin.y;
    cx.simulate_click(trigger, Default::default());
    assert!(view.read_with(cx, |view, _| view.collapsed_projects.contains(&project_id)));
    frame(cx, 0);
    assert_eq!(cx.debug_bounds("add-project").unwrap().origin.y, expanded_y);
    frame(cx, 20);
    let closing_y = cx.debug_bounds("add-project").unwrap().origin.y;
    assert!(closing_y < expanded_y);
    cx.simulate_click(trigger, Default::default());
    frame(cx, 0);
    assert_eq!(cx.debug_bounds("add-project").unwrap().origin.y, closing_y);
    frame(cx, 200);
    assert_eq!(cx.debug_bounds("add-project").unwrap().origin.y, expanded_y);
    cx.simulate_click(trigger, Default::default());
    frame(cx, 0);
    frame(cx, 200);
    assert!(cx.debug_bounds("add-project").unwrap().origin.y < closing_y);
    view.update_in(cx, |view, window, cx| {
        view.set_appearance(
            AppearanceSettings {
                reduced_motion: true,
                ..view.presenter.model().appearance
            },
            window,
            cx,
        );
    });
    cx.simulate_click(trigger, Default::default());
    frame(cx, 0);
    assert_eq!(cx.debug_bounds("add-project").unwrap().origin.y, expanded_y);
    assert_eq!(
        view.read_with(cx, |view, _| view
            .presenter
            .model()
            .conversation
            .selected_task),
        selected_task,
    );
}

#[gpui::test]
fn project_drag_reorders_without_switching_conversations(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, _, directory) = crate::presenter::tests::fixture();
    let selected = presenter
        .model()
        .conversation
        .selected_project
        .clone()
        .unwrap();
    assert!(presenter.submit("Keep the active conversation", "claude"));
    let task = presenter.model().conversation.selected_task;
    for name in ["second", "third"] {
        let path = directory.path().join(name);
        std::fs::create_dir(&path).unwrap();
        presenter.open_project(&path);
    }
    presenter.select_project(selected.clone());
    presenter.select_task(task.unwrap());
    let original: Vec<_> = presenter
        .model()
        .projects
        .iter()
        .map(|project| project.id)
        .collect();
    let selectors: Vec<&'static str> = original
        .iter()
        .map(|id| &*format!("sidebar-project-{id}").leak())
        .collect();
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = NexusView::new(presenter, window, cx);
        view.set_appearance(
            AppearanceSettings {
                reduced_motion: true,
                ..Default::default()
            },
            window,
            cx,
        );
        view.prompt_input
            .update(cx, |input, cx| input.set_value("Keep my draft", window, cx));
        view
    });
    cx.simulate_resize(gpui::size(px(1040.), px(720.)));
    cx.run_until_parked();

    for (source, target, expected) in [
        (0, 2, vec![original[1], original[2], original[0]]),
        (0, 1, original.clone()),
        (2, 0, vec![original[2], original[0], original[1]]),
    ] {
        let from = cx.debug_bounds(selectors[source]).unwrap().center();
        let to = cx.debug_bounds(selectors[target]).unwrap().center();
        cx.simulate_mouse_down(from, gpui::MouseButton::Left, Default::default());
        cx.simulate_mouse_move(
            from + point(px(8.), px(0.)),
            gpui::MouseButton::Left,
            Default::default(),
        );
        cx.update(|_, cx| assert!(cx.has_active_drag()));
        cx.simulate_mouse_move(to, gpui::MouseButton::Left, Default::default());
        cx.simulate_mouse_up(to, gpui::MouseButton::Left, Default::default());
        cx.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.presenter
                    .model()
                    .projects
                    .iter()
                    .map(|project| project.id)
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(
                view.presenter
                    .model()
                    .conversation
                    .selected_project
                    .as_ref()
                    .unwrap()
                    .id,
                selected.id
            );
            assert_eq!(view.presenter.model().conversation.selected_task, task);
            assert!(!view.collapsed_projects.contains(&selected.id));
            assert_eq!(view.prompt_input.read(cx).value(), "Keep my draft");
        });
        let positions: Vec<_> = expected
            .iter()
            .map(|id| {
                let index = original
                    .iter()
                    .position(|original_id| original_id == id)
                    .unwrap();
                cx.debug_bounds(selectors[index]).unwrap().top()
            })
            .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    }

    let before = view.read_with(cx, |view, _| {
        view.presenter
            .model()
            .projects
            .iter()
            .map(|project| project.id)
            .collect::<Vec<_>>()
    });
    for destination in [
        cx.debug_bounds(selectors[0]).unwrap().center(),
        cx.debug_bounds("workspace-header").unwrap().center(),
    ] {
        let from = cx.debug_bounds(selectors[0]).unwrap().center();
        cx.simulate_mouse_down(from, gpui::MouseButton::Left, Default::default());
        cx.simulate_mouse_move(
            from + point(px(8.), px(0.)),
            gpui::MouseButton::Left,
            Default::default(),
        );
        cx.simulate_mouse_move(destination, gpui::MouseButton::Left, Default::default());
        cx.simulate_mouse_up(destination, gpui::MouseButton::Left, Default::default());
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.presenter
                    .model()
                    .projects
                    .iter()
                    .map(|project| project.id)
                    .collect::<Vec<_>>(),
                before,
            );
            assert_eq!(view.presenter.model().conversation.selected_task, task);
        });
    }
}

#[gpui::test]
fn project_new_chat_opens_its_project_without_toggling_the_row(cx: &mut TestAppContext) {
    let (view, cx) = scroll_test_view(cx);
    let project_id = view.read_with(cx, |view, _| {
        assert!(view.presenter.model().conversation.selected_task.is_some());
        view.presenter
            .model()
            .conversation
            .selected_project
            .as_ref()
            .unwrap()
            .id
    });
    let button_selector = format!("project-new-task-{project_id}").leak();
    let project_selector = format!("sidebar-project-{project_id}").leak();
    assert!(cx.debug_bounds(button_selector).is_none());
    let project_bounds = cx.debug_bounds(project_selector).unwrap();
    cx.simulate_mouse_move(project_bounds.center(), None, Default::default());
    assert!(cx.debug_bounds(button_selector).is_some());
    cx.simulate_mouse_move(point(px(400.), px(100.)), None, Default::default());
    assert!(cx.debug_bounds(button_selector).is_none());
    view.update(cx, |view, cx| {
        view.collapsed_projects.insert(project_id);
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.simulate_mouse_move(project_bounds.center(), None, Default::default());
    let button = cx.debug_bounds(button_selector).unwrap().center();
    cx.simulate_click(button, Default::default());
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.presenter
                .model()
                .conversation
                .selected_project
                .as_ref()
                .unwrap()
                .id,
            project_id
        );
        assert!(view.presenter.model().conversation.selected_task.is_none());
        assert!(view.presenter.model().conversation.messages.is_empty());
        assert!(!view.collapsed_projects.contains(&project_id));
    });

    let other_project = tempfile::tempdir().unwrap();
    view.update(cx, |view, cx| {
        view.presenter.open_project(other_project.path());
        assert_ne!(
            view.presenter
                .model()
                .conversation
                .selected_project
                .as_ref()
                .unwrap()
                .id,
            project_id
        );
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let project_bounds = cx.debug_bounds(project_selector).unwrap();
    cx.simulate_mouse_move(project_bounds.center(), None, Default::default());
    let button = cx.debug_bounds(button_selector).unwrap().center();
    cx.simulate_click(button, Default::default());
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.presenter
                .model()
                .conversation
                .selected_project
                .as_ref()
                .unwrap()
                .id,
            project_id
        );
        assert!(view.presenter.model().conversation.selected_task.is_none());
        assert!(!view.collapsed_projects.contains(&project_id));
    });
}
