use super::*;

#[gpui::test]
fn worktree_selectors_share_the_directory_row_and_select_existing_branches(
    cx: &mut gpui::TestAppContext,
) {
    use crate::{infrastructure::git, model::workspace::WorkspaceKind};
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (_repository, project) = git::tests::repository_fixture();
    let (mut presenter, _, _directory) = fixture();
    presenter.open_project(Path::new(&project.canonical_path));
    assert!(presenter.set_appearance(AppearanceSettings {
        reduced_motion: true,
        ..Default::default()
    }));
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    // Read branch choices when opening the menu, including branches added after render.
    // Overflow the dropdown without exceeding Windows' Git ref lock-path limit.
    let source = format!("release/{}", "long-branch-".repeat(6));
    git::git(Path::new(&project.canonical_path), &["branch", &source]).unwrap();
    for (language, width, height) in [
        (Language::Chinese, 1040., 680.),
        (Language::English, 1280., 800.),
    ] {
        cx.simulate_resize(gpui::size(px(width), px(height)));
        view.update_in(cx, |view, window, cx| {
            view.presenter.select_workspace_kind(WorkspaceKind::Local);
            view.set_language(language, window, cx);
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("workspace-base").is_none());
        click_debug(cx, "workspace-mode");
        cx.run_until_parked();
        cx.simulate_keystrokes("down down enter");
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view
                .presenter
                .model()
                .conversation
                .workspace_draft
                .kind),
            WorkspaceKind::Worktree
        );
        click_debug(cx, "workspace-base");
        cx.run_until_parked();
        cx.simulate_keystrokes("down down enter");
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view
                .presenter
                .model()
                .conversation
                .workspace_draft
                .base
                .clone()),
            source
        );
        let context = cx.debug_bounds("composer-context").unwrap();
        let directory = cx.debug_bounds("project-picker-trigger").unwrap();
        let mode = cx.debug_bounds("workspace-mode").unwrap();
        let base = cx.debug_bounds("workspace-base").unwrap();
        let composer = cx.debug_bounds("composer-surface").unwrap();
        assert_eq!(directory.left(), context.left() + px(12.));
        assert_eq!(directory.size.height, mode.size.height);
        assert!(
            directory.center().y >= mode.center().y - px(1.)
                && directory.center().y <= mode.center().y + px(1.)
        );
        assert_eq!(mode.center().y, base.center().y);
        assert!(directory.right() <= mode.left());
        assert!(mode.right() <= base.left());
        assert!(base.right() <= context.right());
        assert_eq!(base.size.width, px(200.));
        assert!(context.bottom() <= composer.top());
        click_debug(cx, "workspace-mode");
        cx.run_until_parked();
        cx.simulate_keystrokes("down enter");
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view
                .presenter
                .model()
                .conversation
                .workspace_draft
                .kind),
            WorkspaceKind::Local
        );
        assert!(cx.debug_bounds("workspace-base").is_none());
    }
}

#[test]
fn working_directory_labels_preserve_paths_and_localize_empty_states() {
    let (presenter, _, _directory) = fixture();
    let mut model = AppModel::default();
    for (language, no_directory) in [
        (Language::Chinese, "未选择目录"),
        (Language::English, "No directory selected"),
    ] {
        model.language = language;
        model.conversation.selected_project = None;
        assert_eq!(working_directory_label(&model), language.text("未关联项目"));
        model.conversation.selected_project =
            presenter.model().conversation.selected_project.clone();
        let long_name = "中文 long directory ".repeat(30);
        for name in ["nexus", "含 空格的目录", long_name.as_str()] {
            let path = Path::new("workspace").join(name).display().to_string();
            let project = model.conversation.selected_project.as_mut().unwrap();
            project.canonical_path = path.clone();
            project.display_name = "unrelated project alias".into();
            assert_eq!(working_directory_label(&model), name);
            assert_eq!(model.working_directory(), Some(path.as_str()));
        }
        let root = if cfg!(windows) { "C:\\" } else { "/" };
        model
            .conversation
            .selected_project
            .as_mut()
            .unwrap()
            .canonical_path = root.into();
        assert_eq!(working_directory_label(&model), root);

        model
            .conversation
            .selected_project
            .as_mut()
            .unwrap()
            .canonical_path
            .clear();
        assert_eq!(working_directory_label(&model), no_directory);
        assert_eq!(model.working_directory(), None);
    }
}

#[gpui::test]
fn projectless_composer_sends_and_sidebar_restores_its_context(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (presenter, runner, _directory) = fixture();
    let project = presenter
        .model()
        .conversation
        .selected_project
        .clone()
        .unwrap();
    let models = presenter
        .model()
        .conversation
        .model_catalog
        .models()
        .unwrap()
        .to_vec();
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    cx.run_until_parked();
    click_debug(cx, "sidebar-projectless");
    cx.run_until_parked();
    view.update_in(cx, |view, window, cx| {
        let ModelCatalogState::Loading { request_id, .. } =
            view.presenter.model().conversation.model_catalog
        else {
            panic!("expected model discovery in the conversation directory");
        };
        runner.emit(nexus_protocol::Event::ModelCatalogLoaded {
            request_id,
            harness: HarnessKind::Claude,
            models,
        });
        view.presenter.drain_events();
        view.prompt_input
            .update(cx, |input, cx| input.set_value("你好", window, cx));
        cx.notify();
    });
    cx.run_until_parked();
    click_debug(cx, "composer-submit");
    cx.run_until_parked();
    let (task_id, cwd) = view.read_with(cx, |view, cx| {
        let model = view.presenter.model();
        assert!(model.conversation.selected_project.is_none());
        assert!(model.conversation.active_run.is_some());
        assert_eq!(model.conversation.messages[0].content, "你好");
        assert_eq!(working_directory_label(model), "未关联项目");
        assert!(view.prompt_input.read(cx).value().is_empty());
        (
            model.conversation.selected_task.unwrap(),
            model.working_directory().unwrap().to_owned(),
        )
    });
    let task_row = format!("sidebar-task-{task_id}").leak();
    assert!(cx.debug_bounds(task_row).is_some());
    click_debug(cx, format!("sidebar-project-{}", project.id).leak());
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| {
        view.presenter
            .model()
            .conversation
            .selected_project
            .is_some()
    }));
    click_debug(cx, task_row);
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        let model = view.presenter.model();
        assert!(model.conversation.selected_project.is_none());
        assert_eq!(model.conversation.selected_task, Some(task_id));
        assert_eq!(model.working_directory(), Some(cwd.as_str()));
    });
    view.update_in(cx, |view, window, cx| view.new_task(window, cx));
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        let model = view.presenter.model();
        assert!(model.conversation.selected_project.is_none());
        assert!(model.conversation.selected_task.is_none());
        assert_ne!(model.working_directory(), Some(cwd.as_str()));
    });
}

#[gpui::test]
fn workspace_opener_tracks_task_switches_and_reports_missing_directories(
    cx: &mut gpui::TestAppContext,
) {
    use crate::presenter::tests::{finish_workspace_operation, worktree_fixture};
    // Path validation uses smol workers outside GPUI's deterministic test scheduler.
    cx.executor().allow_parking();
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, runner, directory, start) = worktree_fixture("open directory");
    runner.emit(Event::RunExited {
        run_id: start.run_id,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    presenter.set_appearance(AppearanceSettings {
        reduced_motion: true,
        ..Default::default()
    });
    let mut project = presenter
        .model()
        .conversation
        .selected_project
        .clone()
        .unwrap();
    project.canonical_path = directory
        .path()
        .join("missing local 目录")
        .display()
        .to_string();
    std::fs::rename(&start.cwd, directory.path().join("moved-worktree")).unwrap();
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    cx.run_until_parked();
    click_debug(cx, "composer-open-directory");
    cx.run_until_parked();
    assert!(cx.debug_bounds("animated-menu-surface").is_some());
    view.update(cx, |view, cx| {
        view.select_project(project.clone());
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("animated-menu-surface").is_none());

    for (language, environment, expected) in [
        (Language::Chinese, false, project.canonical_path.as_str()),
        (Language::English, true, start.cwd.as_str()),
    ] {
        view.update_in(cx, |view, window, cx| {
            if environment {
                view.presenter.select_task(start.task_id);
                view.presenter.toggle_changes_sidebar();
                finish_workspace_operation(&mut view.presenter);
            }
            view.set_language(language, window, cx);
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view
                .presenter
                .model()
                .working_directory()
                .map(str::to_owned)),
            Some(expected.to_owned())
        );
        click_debug(
            cx,
            if environment {
                "environment-directory"
            } else {
                "composer-open-directory"
            },
        );
        cx.run_until_parked();
        cx.simulate_keystrokes("down enter");
        // Path validation runs on a real worker, outside GPUI's fake executor.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !cx.has_pending_prompt() {
            cx.run_until_parked();
            assert!(
                Instant::now() < deadline,
                "missing directory error was not shown"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let (title, detail) = cx.pending_prompt().unwrap();
        assert_eq!(title, language.text("无法打开工作目录"));
        assert!(detail.contains(expected));
        assert!(
            detail.contains(
                language
                    .text("工作目录不存在或无法访问：{path}")
                    .split("{path}")
                    .next()
                    .unwrap()
            )
        );
        cx.simulate_prompt_answer(language.text("确定"));
        cx.run_until_parked();
    }

    project.canonical_path.clear();
    view.update(cx, |view, cx| {
        view.select_project(project);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| {
        view.presenter.model().working_directory().is_none()
    }));
    assert!(cx.debug_bounds("animated-menu-surface").is_none());
    click_debug(cx, "composer-open-directory");
    cx.run_until_parked();
    assert!(cx.debug_bounds("animated-menu-surface").is_none());
    assert!(!cx.has_pending_prompt());
}

#[gpui::test]
fn working_directory_stays_above_composer_without_overlapping_queue_or_controls(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, runner, directory) = fixture();
    let path = directory
        .path()
        .join("parent".repeat(35))
        .join(format!("{}nexus", "目录 name ".repeat(15)));
    std::fs::create_dir_all(&path).unwrap();
    presenter.open_project(&path);
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    for (theme, language, width, height) in [
        (ThemePreference::Light, Language::Chinese, 1040., 680.),
        (ThemePreference::Dark, Language::Chinese, 1040., 680.),
        (ThemePreference::Light, Language::English, 1040., 680.),
        (ThemePreference::Dark, Language::English, 1040., 680.),
        (ThemePreference::Dark, Language::English, 1280., 800.),
    ] {
        cx.simulate_resize(gpui::size(px(width), px(height)));
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
        for prompt in ["start task", "queued message"] {
            cx.run_until_parked();
            let context = cx.debug_bounds("composer-context").unwrap();
            let directory = cx.debug_bounds("composer-directory").unwrap();
            let opener = cx.debug_bounds("composer-open-directory").unwrap();
            let name = cx.debug_bounds("composer-directory-name").unwrap();
            let composer = cx.debug_bounds("composer-surface").unwrap();
            let send = cx.debug_bounds("composer-submit").unwrap();
            assert_eq!(context.left(), composer.left());
            assert_eq!(context.right(), composer.right());
            assert!(directory.top() > px(HEADER_HEIGHT));
            assert!(directory.bottom() <= composer.top());
            assert!(directory.right() <= opener.left());
            assert!(opener.right() <= context.right());
            assert!(name.right() <= context.right());
            assert!(name.size.height <= px(24.));
            assert!(send.left() >= composer.left() && send.right() <= composer.right());
            assert!(send.top() >= composer.top() && send.bottom() <= px(height));
            view.update_in(cx, |view, window, cx| {
                view.prompt_input
                    .update(cx, |input, cx| input.set_value(prompt, window, cx));
            });
            cx.run_until_parked();
            click_debug(cx, "composer-submit");
        }
        cx.run_until_parked();
        let queue = cx.debug_bounds("message-queue").unwrap();
        let composer = cx.debug_bounds("composer-surface").unwrap();
        let directory = cx.debug_bounds("composer-directory").unwrap();
        let stop = cx.debug_bounds("composer-cancel").unwrap();
        assert!(directory.bottom() <= composer.top());
        assert!(queue.top() >= composer.top() && queue.bottom() <= stop.top());
        assert!(stop.bottom() <= composer.bottom() && composer.bottom() <= px(height));
        assert!(stop.right() <= cx.debug_bounds("composer-submit").unwrap().left());
        cx.simulate_mouse_move(directory.center(), None, Default::default());
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        let tooltip = cx.debug_bounds("composer-directory-tooltip").unwrap();
        assert!(tooltip.left() >= px(0.) && tooltip.right() <= px(width));
        assert!(tooltip.top() >= px(0.) && tooltip.bottom() <= px(height));
        assert!(tooltip.size.height > px(24.));
        click_debug(cx, "composer-cancel");
        view.update(cx, |view, cx| {
            let model = view.presenter.model();
            assert!(model.conversation.run_cancelling);
            assert_eq!(model.conversation.queued_messages.len(), 1);
            let queued_id = model.conversation.queued_messages[0].id;
            runner.emit(Event::RunExited {
                run_id: model.conversation.active_run.unwrap(),
                status: RunStatus::Cancelled,
                exit_code: None,
            });
            view.presenter.drain_events();
            view.presenter.remove_queued_message(queued_id);
            view.presenter.new_task();
            cx.notify();
        });
    }
    cx.run_until_parked();
    let directory_bounds = cx.debug_bounds("composer-directory").unwrap();
    cx.simulate_mouse_move(
        gpui::point(
            directory_bounds.left() + px(20.),
            directory_bounds.center().y,
        ),
        None,
        Default::default(),
    );
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert!(cx.debug_bounds("composer-directory-tooltip").is_some());
    view.update(cx, |view, cx| {
        view.presenter.open_project(directory.path());
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("composer-directory-tooltip").is_none());
    let directory_bounds = cx.debug_bounds("composer-directory").unwrap();
    cx.simulate_mouse_move(directory_bounds.center(), None, Default::default());
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert!(cx.debug_bounds("composer-directory-tooltip").is_some());
}
