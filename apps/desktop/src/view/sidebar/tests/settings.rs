use super::*;

#[gpui::test]
fn software_update_settings_show_release_notes_and_installation_progress(cx: &mut TestAppContext) {
    use crate::{
        model::updates::{UpdateChannel, UpdateState},
        presenter::tests::{pending_update, update_package},
    };
    let (view, cx) = scroll_test_view(cx);
    cx.simulate_resize(gpui::size(px(1040.), px(680.)));
    view.update_in(cx, |view, window, cx| view.toggle_settings(window, cx));
    cx.run_until_parked();
    view.update_in(cx, |view, _, cx| {
        view.settings_scroll.scroll_to_bottom();
        cx.notify();
    });
    cx.run_until_parked();
    // The installed channel follows the compiled release tag, so switch explicitly
    // instead of relying on the ambient default to already be Nightly.
    view.update_in(cx, |view, _, cx| {
        assert!(view.presenter.set_update_channel(UpdateChannel::Nightly));
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |view, _| view.presenter.model().updates.channel),
        UpdateChannel::Nightly
    );
    let sender = view.update_in(cx, |view, _, cx| {
        let sender = pending_update(&mut view.presenter);
        cx.notify();
        sender
    });
    cx.run_until_parked();
    let other_channel = cx.debug_bounds("update-channel-release").unwrap().center();
    cx.simulate_click(other_channel, Default::default());
    assert_eq!(
        view.read_with(cx, |view, _| view.presenter.model().updates.channel),
        UpdateChannel::Nightly
    );
    sender
        .send(UpdateState::Available(update_package()))
        .unwrap();
    view.update_in(cx, |view, window, cx| {
        view.poll_events(Instant::now(), cx);
        view.settings_scroll.scroll_to_bottom();
        view.set_language(Language::English, window, cx);
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("download-update").is_some());
    assert!(cx.debug_bounds("release-notes").is_some());
    assert!(cx.debug_bounds("release-notes-link").is_some());
    let sender = view.update_in(cx, |view, _, cx| {
        let sender = pending_update(&mut view.presenter);
        sender
            .send(UpdateState::Installing(update_package()))
            .unwrap();
        view.poll_events(Instant::now(), cx);
        sender
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("download-update").is_none());
    assert!(cx.debug_bounds("release-notes").is_some());
    view.update_in(cx, |view, window, cx| view.toggle_settings(window, cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("update-ready-badge").is_some());
    drop(sender);
}

#[gpui::test]
fn runtime_log_settings_show_newest_entries_and_refresh_while_open(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, runner, _directory) = crate::presenter::tests::fixture();
    for _ in 0..30 {
        presenter.new_task();
    }
    let count = presenter.model().runtime_log.len();
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = NexusView::new(presenter, window, cx);
        view.settings_open = true;
        view.settings_section = SettingsSection::RuntimeLog;
        view
    });
    for (size, language) in [
        (gpui::size(px(1040.), px(680.)), Language::Chinese),
        (gpui::size(px(1280.), px(800.)), Language::English),
    ] {
        cx.simulate_resize(size);
        view.update_in(cx, |view, window, cx| {
            view.set_language(language, window, cx)
        });
        cx.run_until_parked();
        let content = cx.debug_bounds("settings-content-runtime-log").unwrap();
        let newest = cx
            .debug_bounds(format!("runtime-log-entry-{}", count - 1).leak())
            .unwrap();
        let previous = cx
            .debug_bounds(format!("runtime-log-entry-{}", count - 2).leak())
            .unwrap();
        assert!(newest.bottom() <= previous.top());
        assert!(newest.left() >= content.left() && newest.right() <= content.right());
        let navigation = cx.debug_bounds("settings-nav-runtime-log").unwrap();
        assert!(navigation.bottom() <= size.height);
        assert!(view.read_with(cx, |view, _| view.settings_scroll.max_offset().y) > px(0.));
    }
    runner.emit(Event::RunnerReady);
    view.update(cx, |view, cx| {
        assert!(view.presenter.drain_events());
        cx.notify();
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds(format!("runtime-log-entry-{count}").leak())
            .is_some()
    );
    view.update(cx, |view, cx| {
        view.settings_scroll.scroll_to_bottom();
        cx.notify();
    });
    cx.run_until_parked();
    let oldest = cx.debug_bounds("runtime-log-entry-0").unwrap();
    assert!(
        view.read_with(cx, |view, _| view.settings_scroll.bounds())
            .contains(&oldest.center())
    );
}

#[gpui::test]
fn appearance_controls_and_system_changes_preserve_workspace_state(cx: &mut TestAppContext) {
    let (view, cx) = scroll_test_view(cx);
    let assert_preview_corners = |cx: &mut gpui::VisualTestContext| {
        for selector in [
            "appearance-theme-preview-system",
            "appearance-theme-preview-light",
            "appearance-theme-preview-dark",
        ] {
            let bounds = cx.debug_bounds(selector).expect(selector);
            cx.update(|window, _| {
                let bounds = bounds.scale(window.scale_factor());
                let quads = window.painted_quads();
                let border = quads.iter().find(|quad| quad.bounds == bounds).unwrap();
                let inner = gpui::Bounds::from_corners(
                    bounds.origin + point(border.border_widths.left, border.border_widths.top),
                    bounds.bottom_right()
                        - point(border.border_widths.right, border.border_widths.bottom),
                );
                let left = quads
                    .iter()
                    .find(|quad| {
                        quad.bounds.origin == inner.origin && quad.bounds.bottom() == inner.bottom()
                    })
                    .unwrap();
                let right = quads
                    .iter()
                    .find(|quad| {
                        quad.bounds.top() == inner.top()
                            && quad.bounds.bottom_right() == inner.bottom_right()
                    })
                    .unwrap();
                assert_eq!(left.bounds.right(), right.bounds.left());
                let radii = border
                    .corner_radii
                    .map(|radius| *radius - border.border_widths.top);
                assert_eq!(
                    left.corner_radii,
                    gpui::Corners {
                        top_left: radii.top_left,
                        bottom_left: radii.bottom_left,
                        ..Default::default()
                    },
                    "{selector}: left background must meet the rounded border without gaps"
                );
                assert_eq!(
                    right.corner_radii,
                    gpui::Corners {
                        top_right: radii.top_right,
                        bottom_right: radii.bottom_right,
                        ..Default::default()
                    },
                    "{selector}: right background must meet the rounded border without gaps"
                );
            });
        }
    };
    let task = view.read_with(cx, |view, _| {
        view.presenter.model().conversation.selected_task
    });
    view.update_in(cx, |view, window, cx| {
        view.timeline_scroll.set_offset(point(px(0.), px(-120.)));
        view.sidebar_scroll.set_offset(point(px(0.), px(-60.)));
        view.prompt_input
            .update(cx, |input, cx| input.set_value("Keep my draft", window, cx));
        view.toggle_settings(window, cx);
    });
    cx.run_until_parked();
    let appearance_tab = cx.debug_bounds("settings-nav-appearance").unwrap().center();
    cx.simulate_click(appearance_tab, Default::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("settings-content-appearance").is_some());
    assert!(cx.debug_bounds("settings-content-general").is_none());
    for size in [
        gpui::size(px(1040.), px(680.)),
        gpui::size(px(1280.), px(800.)),
    ] {
        cx.simulate_resize(size);
        cx.run_until_parked();
        assert_preview_corners(cx);
        let page = cx.debug_bounds("settings-page").unwrap();
        let navigation = cx.debug_bounds("settings-navigation").unwrap();
        let content = cx.debug_bounds("settings-content-appearance").unwrap();
        let settings_scroll = view.read_with(cx, |view, _| view.settings_scroll.clone());
        assert!(navigation.right() <= settings_scroll.bounds().left());
        assert_eq!(
            content.left(),
            cx.debug_bounds("settings-breadcrumb-label").unwrap().left()
        );
        assert!(content.right() <= page.right());
        assert_eq!(settings_scroll.max_offset().x, px(0.));
        assert_eq!(
            cx.debug_bounds("appearance-glass").unwrap().left(),
            cx.debug_bounds("reduce-motion").unwrap().left()
        );
        for (selector, theme) in [
            ("appearance-theme-dark", ThemePreference::Dark),
            ("appearance-theme-light", ThemePreference::Light),
            ("appearance-theme-system", ThemePreference::System),
        ] {
            let bounds = cx.debug_bounds(selector).unwrap();
            assert!(
                cx.debug_bounds("settings-page")
                    .unwrap()
                    .contains(&bounds.center())
            );
            cx.simulate_mouse_move(bounds.center(), None, Default::default());
            cx.run_until_parked();
            assert_preview_corners(cx);
            cx.simulate_click(bounds.center(), Default::default());
            cx.run_until_parked();
            assert_preview_corners(cx);
            view.read_with(cx, |view, cx| {
                assert_eq!(view.presenter.model().appearance.theme, theme);
                assert_eq!(view.presenter.model().conversation.selected_task, task);
                assert_eq!(view.prompt_input.read(cx).value(), "Keep my draft");
                assert_eq!(view.timeline_scroll.offset().y, px(-120.));
                assert_eq!(view.sidebar_scroll.offset().y, px(-60.));
            });
        }
        for selector in [
            "appearance-glass",
            "appearance-glass",
            "reduce-motion",
            "reduce-motion",
        ] {
            let bounds = cx.debug_bounds(selector).unwrap();
            cx.simulate_click(bounds.center(), Default::default());
            cx.run_until_parked();
            view.read_with(cx, |view, cx| {
                assert_eq!(
                    cx.reduce_motion(),
                    view.presenter.model().appearance.reduced_motion
                );
                assert_eq!(view.timeline_scroll.offset().y, px(-120.));
                assert_eq!(view.presenter.model().conversation.selected_task, task);
                assert_eq!(view.prompt_input.read(cx).value(), "Keep my draft");
            });
        }
    }
    for system in [gpui::WindowAppearance::Dark, gpui::WindowAppearance::Light] {
        view.update_in(cx, |view, window, cx| {
            let appearance = ResolvedAppearance::resolve(
                view.presenter.model().appearance,
                system,
                SystemAccessibility::default(),
                true,
                cfg!(target_os = "macos"),
            );
            apply_theme(appearance, cx);
            window.refresh();
            cx.notify();
        });
        cx.run_until_parked();
        assert_preview_corners(cx);
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<ResolvedAppearance>().dark,
                system == gpui::WindowAppearance::Dark,
            )
        });
    }
    view.update_in(cx, |view, _, cx| {
        view.settings_scroll.scroll_to_bottom();
        cx.notify();
    });
    cx.run_until_parked();
    for selector in ["font-reading-select", "font-code-select"] {
        let bounds = cx.debug_bounds(selector).unwrap();
        cx.simulate_click(bounds.center(), Default::default());
        cx.run_until_parked();
        let query = view.read_with(cx, |view, _| {
            view.presenter.model().language.text("系统界面字体")
        });
        cx.simulate_input(query);
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
    }
    view.read_with(cx, |view, cx| {
        assert_eq!(
            view.presenter.model().fonts.reading.as_deref(),
            Some(".SystemUIFont")
        );
        assert_eq!(
            view.presenter.model().fonts.code.as_deref(),
            Some(".SystemUIFont")
        );
        assert_eq!(reading_font(cx).family.as_ref(), ".SystemUIFont");
        assert_eq!(mono_font(cx).as_ref(), ".SystemUIFont");
        assert_eq!(view.presenter.model().conversation.selected_task, task);
        assert_eq!(view.prompt_input.read(cx).value(), "Keep my draft");
        assert_eq!(view.timeline_scroll.offset().y, px(-120.));
    });
    assert!(cx.debug_bounds("font-preview-reading").is_some());
    assert!(cx.debug_bounds("font-preview-code").is_some());
    let preview = cx.debug_bounds("font-preview-code").unwrap();
    view.read_with(cx, |view, _| {
        assert!(view.settings_scroll.offset().y < px(0.));
        assert!(view.settings_scroll.bounds().contains(&preview.center()));
    });
    let reset = cx.debug_bounds("font-reset").unwrap().center();
    cx.simulate_click(reset, Default::default());
    cx.run_until_parked();
    view.read_with(cx, |view, cx| {
        assert_eq!(
            view.presenter.model().fonts,
            crate::model::FontSettings::default()
        );
        assert!(
            view.font_controls
                .reading
                .read(cx)
                .selected_value()
                .is_none()
        );
        assert!(view.font_controls.code.read(cx).selected_value().is_none());
    });
    let counts = view.read_with(cx, |view, cx| {
        (
            view.sidebar_pane.read(cx).render_count,
            view.timeline_pane.read(cx).render_count,
        )
    });
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    view.read_with(cx, |view, cx| {
        assert_eq!(
            counts,
            (
                view.sidebar_pane.read(cx).render_count,
                view.timeline_pane.read(cx).render_count
            )
        );
    });
}

#[gpui::test]
fn settings_navigation_preserves_drafts_and_restores_visible_focus(cx: &mut TestAppContext) {
    let (view, cx) = scroll_test_view(cx);
    cx.simulate_resize(gpui::size(px(1040.), px(680.)));
    cx.run_until_parked();
    let selected_task = view.read_with(cx, |view, _| {
        view.presenter.model().conversation.selected_task
    });
    view.update_in(cx, |view, window, cx| {
        view.prompt_input.update(cx, |input, cx| {
            input.set_value("Keep this draft", window, cx);
            input.focus(window, cx);
        });
    });
    cx.run_until_parked();
    let settings_button = cx.debug_bounds("open-settings").unwrap().center();
    cx.simulate_click(settings_button, Default::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("settings-content-general").is_some());
    let agent_tab = cx.debug_bounds("settings-nav-agent").unwrap().center();
    cx.simulate_click(agent_tab, Default::default());
    view.update_in(cx, |view, window, cx| {
        assert!(view.settings_open);
        assert_eq!(view.settings_section, SettingsSection::Agent);
        assert!(view.focus_handle.is_focused(window));
        assert!(
            !view
                .prompt_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
        view.executable_input.update(cx, |input, cx| {
            input.set_value("custom-agent", window, cx);
            input.focus(window, cx);
        });
        view.provider_form.name.update(cx, |input, cx| {
            input.set_value("Keep this provider draft", window, cx);
        });
        view.provider_form.api_key.update(cx, |input, cx| {
            input.set_value("unsaved-test-key", window, cx);
        });
    });
    for (selector, section) in [
        ("settings-nav-providers", SettingsSection::Providers),
        ("settings-nav-remote", SettingsSection::Remote),
        ("settings-nav-appearance", SettingsSection::Appearance),
        ("settings-nav-general", SettingsSection::General),
        ("settings-nav-runtime-log", SettingsSection::RuntimeLog),
        ("settings-nav-providers", SettingsSection::Providers),
    ] {
        cx.run_until_parked();
        let tab = cx.debug_bounds(selector).unwrap().center();
        cx.simulate_click(tab, Default::default());
        view.update_in(cx, |view, window, cx| {
            assert_eq!(view.settings_section, section);
            assert!(view.focus_handle.is_focused(window));
            assert_eq!(view.executable_input.read(cx).value(), "custom-agent");
            assert_eq!(
                view.provider_form.name.read(cx).value(),
                "Keep this provider draft"
            );
            assert_eq!(
                view.provider_form.api_key.read(cx).value(),
                "unsaved-test-key"
            );
            if section == SettingsSection::Providers {
                view.provider_form
                    .name
                    .update(cx, |input, cx| input.focus(window, cx));
            }
        });
        if section == SettingsSection::Providers {
            cx.run_until_parked();
            let harness = cx.debug_bounds("settings-provider-harness").unwrap();
            let profile = cx.debug_bounds("settings-provider-profile").unwrap();
            assert_eq!(harness.left(), profile.left());
            assert_eq!(harness.size, profile.size);
        }
        if section == SettingsSection::General {
            for (selector, language, prompt_placeholder, search_placeholder, group_title) in [
                (
                    "language-en",
                    Language::English,
                    "Describe a goal for the agent…",
                    "Search tasks…",
                    "Default",
                ),
                (
                    "language-zh-CN",
                    Language::Chinese,
                    "描述一个目标，让 Agent 开始工作…",
                    "搜索任务…",
                    "默认",
                ),
            ] {
                cx.run_until_parked();
                let language_button = cx.debug_bounds(selector).unwrap();
                cx.simulate_click(language_button.center(), Default::default());
                cx.run_until_parked();
                view.read_with(cx, |view, cx| {
                    assert_eq!(view.presenter.model().language, language);
                    assert_eq!(
                        view.prompt_input.read(cx).presentation().placeholder(),
                        prompt_placeholder
                    );
                    assert_eq!(
                        view.search_input.read(cx).presentation().placeholder(),
                        search_placeholder
                    );
                    assert_eq!(view.model_picker.content.groups[0].title, group_title);
                    assert_eq!(view.prompt_input.read(cx).value(), "Keep this draft");
                    assert_eq!(view.executable_input.read(cx).value(), "custom-agent");
                    assert_eq!(
                        view.provider_form.name.read(cx).value(),
                        "Keep this provider draft"
                    );
                    assert_eq!(
                        view.provider_form.api_key.read(cx).value(),
                        "unsaved-test-key"
                    );
                    assert_eq!(
                        view.presenter.model().conversation.selected_task,
                        selected_task
                    );
                });
            }
        }
    }
    cx.run_until_parked();
    let settings_shortcut = if cfg!(target_os = "macos") {
        "cmd-,"
    } else {
        "ctrl-,"
    };
    cx.simulate_keystrokes(settings_shortcut);
    view.update_in(cx, |view, window, cx| {
        assert!(!view.settings_open);
        assert!(
            view.prompt_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
        assert_eq!(view.prompt_input.read(cx).value(), "Keep this draft");
        assert_eq!(view.executable_input.read(cx).value(), "custom-agent");
        assert_eq!(
            view.presenter.model().conversation.selected_task,
            selected_task
        );
    });
    cx.simulate_keystrokes(settings_shortcut);
    view.read_with(cx, |view, cx| {
        assert!(view.settings_open);
        assert_eq!(view.settings_section, SettingsSection::Providers);
        assert_eq!(
            view.provider_form.name.read(cx).value(),
            "Keep this provider draft"
        );
    });
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-k"
    } else {
        "ctrl-k"
    });
    view.update_in(cx, |view, window, cx| {
        assert!(!view.settings_open);
        assert!(
            view.search_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
        assert_eq!(
            view.presenter.model().conversation.selected_task,
            selected_task
        );
    });
    cx.simulate_keystrokes(settings_shortcut);
    assert!(view.read_with(cx, |view, _| view.settings_open));
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-n"
    } else {
        "ctrl-n"
    });
    view.update_in(cx, |view, window, cx| {
        assert!(!view.settings_open);
        assert!(view.presenter.model().conversation.selected_task.is_none());
        assert!(
            view.prompt_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
        assert_eq!(view.prompt_input.read(cx).value(), "Keep this draft");
    });
}
