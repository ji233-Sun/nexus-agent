use super::*;

#[gpui::test]
fn working_directory_picker_searches_switches_and_dismisses_without_resetting_current_project(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, _, directory) = fixture();
    let first = presenter
        .model()
        .conversation
        .selected_project
        .clone()
        .unwrap();
    let path = directory.path().join("第二个 Project");
    std::fs::create_dir_all(&path).unwrap();
    presenter.open_project(&path);
    let second = presenter
        .model()
        .conversation
        .selected_project
        .clone()
        .unwrap();
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    view.update_in(cx, |view, window, cx| {
        view.prompt_input
            .update(cx, |input, cx| input.set_value("keep draft", window, cx));
    });
    cx.run_until_parked();
    let trigger = cx.debug_bounds("project-picker-trigger").unwrap();
    let content = cx.debug_bounds("composer-directory").unwrap();
    let name = cx.debug_bounds("composer-directory-name").unwrap();
    assert_eq!(content.left() - trigger.left(), px(8.));
    assert_eq!(trigger.right() - content.right(), px(8.));
    assert!(trigger.top() < content.top() && content.bottom() < trigger.bottom());
    assert!(
        trigger.size.width <= name.size.width + px(40.),
        "directory trigger should fit its icon, name and padding: {trigger:?}, {name:?}"
    );
    click_debug(cx, "composer-directory");
    cx.run_until_parked();
    assert!(cx.debug_bounds("project-picker-surface").is_some());
    assert!(cx.debug_bounds("project-picker-new").is_some());
    let second_row: &'static str = format!("project-picker-{}", second.id).leak();
    let first_row: &'static str = format!("project-picker-{}", first.id).leak();
    assert!(cx.debug_bounds(first_row).is_some());
    let search = cx.debug_bounds("project-picker-search").unwrap();
    let first_bounds = cx.debug_bounds(first_row).unwrap();
    let second_bounds = cx.debug_bounds(second_row).unwrap();
    assert!(search.size.height >= px(CONTROL_HEIGHT));
    assert_eq!(first_bounds.size.height, px(40.));
    assert_eq!(second_bounds.size.height, px(40.));
    assert!(first_bounds.top() >= search.bottom() + px(12.));
    assert!(second_bounds.top() >= search.bottom() + px(12.));
    assert!(
        first_bounds.bottom() + px(4.) <= second_bounds.top()
            || second_bounds.bottom() + px(4.) <= first_bounds.top()
    );
    view.update_in(cx, |view, window, cx| {
        view.project_search_input.update(cx, |input, cx| {
            input.set_value(" 第二个 PROJECT ", window, cx)
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds(first_row).is_none());
    click_debug(cx, second_row);
    cx.run_until_parked();
    assert!(cx.debug_bounds("project-picker-surface").is_none());
    view.read_with(cx, |view, cx| {
        assert_eq!(view.prompt_input.read(cx).value(), "keep draft");
        assert_eq!(
            view.presenter
                .model()
                .conversation
                .selected_project
                .as_ref()
                .unwrap()
                .id,
            second.id
        );
    });
    click_debug(cx, "composer-directory");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds(first_row).is_some(),
        "reopening clears search"
    );
    click_debug(cx, first_row);
    cx.run_until_parked();
    assert!(cx.debug_bounds("project-picker-surface").is_none());
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.presenter
                .model()
                .conversation
                .selected_project
                .as_ref()
                .unwrap()
                .id,
            first.id
        );
    });
    click_debug(cx, "composer-directory");
    cx.run_until_parked();
    click_debug(cx, "project-picker-none");
    cx.run_until_parked();
    assert!(cx.debug_bounds("project-picker-surface").is_none());
    assert!(cx.debug_bounds("sidebar-projectless").is_some());
    view.read_with(cx, |view, _| {
        assert!(
            view.presenter
                .model()
                .conversation
                .selected_project
                .is_none()
        );
        assert_eq!(
            working_directory_label(view.presenter.model()),
            "未关联项目"
        );
        assert_ne!(
            view.presenter.model().working_directory(),
            Some(first.canonical_path.as_str())
        );
    });
    click_debug(cx, "composer-directory");
    cx.run_until_parked();
    click_debug(cx, first_row);
    cx.run_until_parked();
    click_debug(cx, "composer-directory");
    cx.run_until_parked();
    view.update_in(cx, |view, window, cx| {
        view.project_search_input.update(cx, |input, cx| {
            input.set_value("no such project", window, cx)
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds(first_row).is_none());
    assert!(cx.debug_bounds(second_row).is_none());
    assert!(cx.debug_bounds("project-picker-new").is_some());
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds("project-picker-surface").is_none());
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.presenter
                .model()
                .conversation
                .selected_project
                .as_ref()
                .unwrap()
                .id,
            first.id
        );
    });
    let empty_directory = tempfile::tempdir().unwrap();
    view.update(cx, |view, cx| {
        view.presenter = Presenter::new(
            crate::infrastructure::storage::Storage::open(
                &empty_directory.path().join("empty.sqlite"),
            )
            .unwrap(),
            Err(anyhow::anyhow!("no runner")),
            None,
        );
        cx.notify();
    });
    cx.run_until_parked();
    click_debug(cx, "composer-directory");
    cx.run_until_parked();
    assert!(cx.debug_bounds("project-picker-surface").is_some());
    assert!(cx.debug_bounds("project-picker-new").is_some());
    cx.simulate_click(gpui::point(px(10.), px(10.)), Default::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("project-picker-surface").is_none());
    assert!(view.read_with(cx, |view, _| {
        view.presenter
            .model()
            .conversation
            .selected_project
            .is_none()
    }));
}

#[test]
fn catalog_content_groups_providers_and_searches_provider_name_and_full_id() {
    let mut model = AppModel {
        conversation: crate::model::ConversationState {
            selected_harness: HarnessKind::Omp,
            model_catalog: ModelCatalogState::Ready(vec![
                omp_model("openai", "openai/shared-model"),
                omp_model("bigmodel", "bigmodel/shared-model"),
            ]),
            ..Default::default()
        },
        ..Default::default()
    };
    model.conversation.model_override = Some("bigmodel/shared-model".into());

    let content = CatalogModelSelectContent::from_model(&model);
    assert_eq!(
        content
            .groups
            .iter()
            .map(|group| group.title.as_str())
            .collect::<Vec<_>>(),
        vec!["默认", "bigmodel", "openai"]
    );
    let bigmodel = &content.groups[1].items[0];
    assert!(bigmodel.matches("BIGMODEL"));
    assert!(bigmodel.matches("shared model"));
    assert!(bigmodel.matches("bigmodel/shared-model"));
    assert_eq!(
        bigmodel.choice,
        CatalogModelChoice::Model("bigmodel/shared-model".into())
    );
    assert_eq!(
        content.groups[2].items[0].choice,
        CatalogModelChoice::Model("openai/shared-model".into())
    );
    assert!(content.selected_index().is_some());
}

#[test]
fn catalog_content_keeps_an_unavailable_full_selector_visible() {
    let model = AppModel {
        conversation: crate::model::ConversationState {
            selected_harness: HarnessKind::Omp,
            model_override: Some("private-provider/custom-model".into()),
            model_catalog: ModelCatalogState::Ready(vec![omp_model(
                "public-provider",
                "public-provider/custom-model",
            )]),
            ..Default::default()
        },
        ..Default::default()
    };

    let content = CatalogModelSelectContent::from_model(&model);
    let current = &content.groups[1];
    assert_eq!(current.title, "当前选择");
    assert!(current.items[0].disabled);
    assert_eq!(
        current.items[0].choice,
        CatalogModelChoice::Model("private-provider/custom-model".into())
    );
    assert!(current.items[0].title.contains("不可用"));
    assert!(content.selected_index().is_some());
}

#[test]
fn picker_distinguishes_catalog_states_and_does_not_confuse_default_with_override() {
    let mut default = omp_model("provider", "default-model");
    default.is_default = true;
    let mut explicit = omp_model("provider", "explicit-model");
    explicit.display_name = "Chosen display name".into();
    let mut model = AppModel {
        conversation: crate::model::ConversationState {
            selected_harness: HarnessKind::Omp,
            model_override: Some("explicit-model".into()),
            model_override_name: Some(explicit.display_name.clone()),
            model_catalog: ModelCatalogState::Ready(vec![default, explicit]),
            ..Default::default()
        },
        ..Default::default()
    };
    let content = CatalogModelSelectContent::from_model(&model);
    assert!(content.groups[0].items[0].title.contains("default-model"));
    assert!(!content.groups[0].items[0].title.contains("explicit-model"));
    for (state, status) in [
        (ModelCatalogState::Idle, "会话目录尚未就绪"),
        (
            ModelCatalogState::Loading {
                request_id: Uuid::new_v4(),
                models: vec![],
            },
            "正在加载",
        ),
        (ModelCatalogState::Empty, "目录为空"),
        (
            ModelCatalogState::Failed {
                message: "test failure".into(),
                models: vec![],
            },
            "test failure",
        ),
        (
            ModelCatalogState::NotReady("CLI not ready".into()),
            "CLI not ready",
        ),
    ] {
        model.conversation.model_catalog = state;
        let content = CatalogModelSelectContent::from_model(&model);
        let items = content
            .groups
            .iter()
            .flat_map(|group| &group.items)
            .collect::<Vec<_>>();
        assert!(items.iter().any(|item| item.title.contains(status)));
        assert!(
            items
                .iter()
                .any(|item| item.disabled && item.title.contains("Chosen display name"))
        );
    }
    let mut default = omp_model("provider", "default-model");
    default.is_default = true;
    default.availability = nexus_domain::ModelAvailability::Unavailable {
        reason: "disabled by provider".into(),
    };
    model.conversation.model_catalog = ModelCatalogState::Ready(vec![default]);
    model.conversation.model_override = None;
    let content = CatalogModelSelectContent::from_model(&model);
    let default = &content.groups[0].items[0];
    assert!(default.title.contains("disabled by provider"));
    assert!(default.trigger_title.contains("不可用"));
}

#[gpui::test]
fn claude_picker_accepts_custom_ids_and_keeps_them_unverified(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (presenter, _runner, _directory) = fixture();
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    cx.simulate_resize(gpui::size(px(1040.), px(680.)));
    for (language, input) in [
        (Language::Chinese, "  moonshotai/Kimi-K2.5  "),
        (Language::English, "GLM-5"),
    ] {
        view.update_in(cx, |view, window, cx| {
            view.set_language(language, window, cx)
        });
        cx.run_until_parked();
        click_debug(cx, "composer-model");
        cx.simulate_input(input);
        cx.run_until_parked();
        view.read_with(cx, |view, cx| {
            let list = view.model_picker.list.read(cx).delegate();
            let item = list.item(IndexPath::new(0)).unwrap();
            assert_eq!(item.choice, CatalogModelChoice::Model(input.trim().into()));
            assert!(!item.disabled);
            assert!(list.item(IndexPath::new(1)).is_none());
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(cx.debug_bounds("model-picker-surface").is_none());
        view.read_with(cx, |view, _| {
            let model = view.presenter.model();
            assert_eq!(
                model.conversation.model_override.as_deref(),
                Some(input.trim())
            );
            assert!(model.can_submit());
            let content = CatalogModelSelectContent::from_model(model);
            let index = content.selected_index().unwrap();
            let selected = &content.groups[index.section].items[index.row];
            assert!(!selected.disabled);
            assert!(selected.trigger_title.contains(language.text("未验证")));
            assert!(!selected.trigger_title.contains(language.text("不可用")));
        });
    }
    click_debug(cx, "composer-model");
    cx.simulate_input("opus");
    cx.run_until_parked();
    view.read_with(cx, |view, cx| {
        let list = view.model_picker.list.read(cx).delegate();
        assert_eq!(list.sections_count(cx), 1);
        assert_eq!(list.items_count(0, cx), 1);
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |view, _| view
            .presenter
            .model()
            .conversation
            .model_override
            .clone()),
        Some("opus".into())
    );
    click_debug(cx, "composer-model");
    cx.simulate_input("default");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| {
        view.presenter.model().conversation.model_override.is_none()
    }));
}

#[gpui::test]
fn unified_picker_supports_keyboard_focus_and_minimum_window_bounds(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, runner, _directory) = fixture();
    presenter.select_harness(HarnessKind::Omp, "claude");
    let ModelCatalogState::Loading { request_id, .. } =
        presenter.model().conversation.model_catalog
    else {
        panic!("loading")
    };
    let mut long = omp_model("provider", "provider/needle-target");
    long.display_name = "Long model name · 很长的模型名称 ".repeat(40);
    runner.emit(Event::ModelCatalogLoaded {
        request_id,
        harness: HarnessKind::Omp,
        models: vec![long, omp_model("provider", "provider/other-model")],
    });
    presenter.drain_events();
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    for (width, height, theme, glass) in [
        (1040., 680., ThemePreference::Light, true),
        (1280., 800., ThemePreference::Dark, false),
    ] {
        cx.simulate_resize(gpui::size(px(width), px(height)));
        view.update_in(cx, |view, window, cx| {
            view.set_appearance(
                AppearanceSettings {
                    theme,
                    glass,
                    reduced_motion: true,
                },
                window,
                cx,
            );
            view.prompt_input
                .update(cx, |input, cx| input.focus(window, cx));
        });
        cx.run_until_parked();
        let trigger = cx.debug_bounds("composer-model").unwrap();
        assert!(trigger.right() <= px(width));
        assert!(trigger.size.width <= px(300.));
        cx.simulate_click(trigger.center(), Default::default());
        cx.run_until_parked();
        let bounds = cx.debug_bounds("model-picker-surface").unwrap();
        assert!(
            bounds.left() >= px(0.) && bounds.right() <= px(width),
            "{bounds:?}"
        );
        assert!(
            bounds.top() >= px(0.) && bounds.bottom() <= px(height),
            "{bounds:?}"
        );
        assert!(cx.debug_bounds("model-config-claude-cli").is_some());
        assert!(cx.debug_bounds("model-config-codex-cli").is_some());
        assert!(cx.debug_bounds("model-config-omp-cli").is_some());
        view.update_in(cx, |view, window, cx| {
            assert!(view.model_picker.list.focus_handle(cx).is_focused(window));
        });
        cx.simulate_keystrokes("down up escape");
        cx.run_until_parked();
        assert!(cx.debug_bounds("model-picker-surface").is_none());
        view.update_in(cx, |view, window, cx| {
            assert!(view.prompt_input.focus_handle(cx).is_focused(window));
        });
        cx.simulate_click(trigger.center(), Default::default());
        cx.run_until_parked();
        cx.simulate_input("NEEDLE-target");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(cx.debug_bounds("model-picker-surface").is_none());
        assert_eq!(
            view.read_with(cx, |view, _| view
                .presenter
                .model()
                .conversation
                .model_override
                .clone()),
            Some("provider/needle-target".into())
        );
    }
    let trigger = cx.debug_bounds("composer-model").unwrap();
    cx.simulate_click(trigger.center(), Default::default());
    cx.run_until_parked();
    let claude = cx.debug_bounds("model-config-claude-cli").unwrap();
    cx.simulate_click(claude.center(), Default::default());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |view, _| view
            .presenter
            .model()
            .conversation
            .selected_harness),
        HarnessKind::Claude
    );
    assert!(cx.debug_bounds("model-picker-surface").is_some());
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds("model-picker-surface").is_none());
}

#[gpui::test]
fn generation_model_settings_support_search_and_preserve_conversation_selection(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (presenter, runner, _directory) = fixture();
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    view.update_in(cx, |view, window, cx| {
        view.toggle_settings(window, cx);
        assert!(matches!(
            view.presenter.model().conversation.title_model_catalog,
            ModelCatalogState::Loading { .. }
        ));
    });
    for ((width, height, language), kind) in [
        (1040., 680., Language::Chinese),
        (1280., 800., Language::English),
    ]
    .into_iter()
    .flat_map(|layout| GenerationKind::ALL.map(|kind| (layout, kind)))
    {
        let harness_selector = match kind {
            GenerationKind::Title => "title-harness",
            GenerationKind::Commit => "commit-harness",
        };
        let model_selector = match kind {
            GenerationKind::Title => "title-model",
            GenerationKind::Commit => "commit-model",
        };
        let effort_selector = match kind {
            GenerationKind::Title => "title-effort",
            GenerationKind::Commit => "commit-effort",
        };
        let picker_selector = match kind {
            GenerationKind::Title => "title-model-picker-surface",
            GenerationKind::Commit => "commit-model-picker-surface",
        };
        cx.simulate_resize(gpui::size(px(width), px(height)));
        view.update_in(cx, |view, window, cx| {
            view.settings_open = true;
            view.settings_scroll.set_offset(gpui::point(px(0.), px(0.)));
            view.set_language(language, window, cx);
            view.presenter
                .select_generation_harness(kind, HarnessKind::Claude);
            view.reduced_motion = true;
            cx.notify();
        });
        cx.run_until_parked();
        let content = cx.debug_bounds("settings-content-general").unwrap();
        let breadcrumb = cx.debug_bounds("settings-breadcrumb-label").unwrap();
        assert_eq!(content.left(), breadcrumb.left());
        let harness = cx.debug_bounds(harness_selector).unwrap();
        let model = cx.debug_bounds(model_selector).unwrap();
        assert_eq!(model.left(), harness.left());
        assert_eq!(model.size, harness.size);
        for (first, second) in [
            ("language-zh-CN", "language-en"),
            ("update-channel-release", "update-channel-nightly"),
        ] {
            let first = cx.debug_bounds(first).unwrap();
            let second = cx.debug_bounds(second).unwrap();
            assert_eq!(first.left(), harness.left());
            assert_eq!(second.right(), harness.right());
            assert_eq!(first.size, second.size);
        }
        assert_eq!(
            cx.debug_bounds("update-check-on-startup").unwrap().left(),
            harness.left()
        );
        if kind == GenerationKind::Commit {
            view.update(cx, |view, cx| {
                view.settings_scroll
                    .set_offset(gpui::point(px(0.), px(160.) - harness.top()));
                cx.notify();
            });
            cx.run_until_parked();
        }
        click_debug(cx, harness_selector);

        cx.run_until_parked();
        cx.simulate_keystrokes("down down down enter");
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view
                .presenter
                .model()
                .generation_settings(kind)
                .harness),
            HarnessKind::Omp
        );
        click_debug(cx, model_selector);
        cx.run_until_parked();
        let request_id = view.read_with(cx, |view, _| {
            let ModelCatalogState::Loading { request_id, .. } =
                view.presenter.model().generation_catalog(kind).clone()
            else {
                panic!("loading")
            };
            request_id
        });
        let mut long = omp_model("provider", "provider/title-target");
        long.display_name = "Long title model name ".repeat(30);
        runner.emit(Event::ModelCatalogLoaded {
            request_id,
            harness: HarnessKind::Omp,
            models: vec![long],
        });
        view.update(cx, |view, cx| {
            view.presenter.drain_events();
            cx.notify();
        });
        cx.run_until_parked();
        let bounds = cx.debug_bounds(picker_selector).unwrap();
        assert!(
            bounds.left() >= px(0.) && bounds.right() <= px(width),
            "{bounds:?}"
        );
        assert!(
            bounds.top() >= px(0.) && bounds.bottom() <= px(height),
            "{bounds:?}"
        );
        cx.simulate_input("title-target");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(cx.debug_bounds(picker_selector).is_none());
        let effort_bounds = cx.debug_bounds(effort_selector).unwrap();
        assert!(effort_bounds.right() <= px(width));
        assert!(effort_bounds.bottom() <= px(height));
        click_debug(cx, effort_selector);
        cx.run_until_parked();
        cx.simulate_keystrokes("down down enter");
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.presenter
                    .model()
                    .generation_settings(kind)
                    .model
                    .as_deref(),
                Some("provider/title-target")
            );
            assert_eq!(
                view.presenter.model().conversation.selected_harness,
                HarnessKind::Claude
            );
            assert!(view.presenter.model().conversation.model_override.is_none());
            assert_eq!(
                view.presenter.model().generation_settings(kind).effort,
                ThinkingEffort::XHigh
            );
            assert_eq!(
                view.presenter.model().conversation.effort,
                ThinkingEffort::Default
            );
        });
        click_debug(cx, effort_selector);
        cx.run_until_parked();
        cx.simulate_keystrokes("down enter");
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view
                .presenter
                .model()
                .generation_settings(kind)
                .effort),
            ThinkingEffort::Default
        );
        let trigger = cx.debug_bounds(model_selector).unwrap();
        assert!(trigger.right() <= px(width));
        assert_eq!(trigger.size, harness.size);
        click_debug(cx, model_selector);
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(cx.debug_bounds(picker_selector).is_none());
        view.update(cx, |view, cx| {
            view.presenter
                .select_generation_harness(kind, HarnessKind::Claude);
            cx.notify();
        });
        cx.run_until_parked();
        click_debug(cx, model_selector);
        cx.simulate_input("GLM-5");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(cx.debug_bounds(picker_selector).is_none());
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.presenter
                    .model()
                    .generation_settings(kind)
                    .model
                    .as_deref(),
                Some("GLM-5")
            );
            assert!(view.presenter.model().conversation.model_override.is_none());
        });
    }
}

#[gpui::test]
fn catalog_select_syncs_after_a_catalog_response(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.select_harness(HarnessKind::Omp, "claude"));
    let ModelCatalogState::Loading { request_id, .. } =
        presenter.model().conversation.model_catalog
    else {
        panic!("expected loading catalog")
    };
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });

    runner.emit(Event::ModelCatalogLoaded {
        request_id,
        harness: HarnessKind::Omp,
        models: vec![omp_model("bigmodel", "bigmodel/shared-model")],
    });
    view.update_in(cx, |view, _, cx| {
        assert!(view.presenter.drain_events());
        cx.notify();
    });
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    view.update_in(cx, |view, _, cx| {
        view.presenter
            .select_catalog_model(Some("bigmodel/shared-model".into()));
        cx.notify();
    });
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });

    assert_eq!(
        view.read_with(cx, |view, cx| {
            view.model_picker
                .list
                .read(cx)
                .delegate()
                .selected_index()
                .and_then(|index| {
                    view.model_picker
                        .list
                        .read(cx)
                        .delegate()
                        .item(index)
                        .map(|item| item.choice.clone())
                })
        }),
        Some(CatalogModelChoice::Model("bigmodel/shared-model".into()))
    );
}
