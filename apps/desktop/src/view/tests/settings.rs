use super::*;

#[gpui::test]
fn voice_settings_select_before_configure_and_draft_insertion_is_undoable(
    cx: &mut gpui::TestAppContext,
) {
    use crate::model::voice::Provider;
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (presenter, _, _directory) = fixture();
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    cx.simulate_resize(gpui::size(px(1040.), px(680.)));
    view.update_in(cx, |view, window, cx| {
        view.set_language(Language::English, window, cx);
    });
    cx.run_until_parked();
    let voice = cx.debug_bounds("voice-record").unwrap();
    let submit = cx.debug_bounds("composer-submit").unwrap();
    let composer = cx.debug_bounds("composer-surface").unwrap();
    assert_eq!(voice.center().y, submit.center().y);
    assert!(voice.left() >= composer.left() && voice.right() <= submit.left());
    assert!(voice.top() >= composer.top() && voice.bottom() <= composer.bottom());
    click_debug(cx, "voice-record");
    cx.run_until_parked();
    assert!(cx.debug_bounds("voice-settings").is_some());
    assert!(cx.debug_bounds("settings-nav-voice").is_some());
    assert_eq!(
        Language::English.text("配置语音输入"),
        "Configure voice input"
    );
    assert!(cx.debug_bounds("voice-mimo-config").is_none());
    #[cfg(target_os = "macos")]
    {
        view.update(cx, |view, cx| {
            view.presenter
                .select_voice_provider(Provider::MacOs)
                .unwrap();
            cx.notify();
        });
        cx.run_until_parked();
        let trigger = cx.debug_bounds("voice-locale").unwrap();
        let position = gpui::point(trigger.left() + px(16.), trigger.center().y);
        cx.simulate_click(position, Default::default());
        cx.run_until_parked();
        let settings_offset = view.read_with(cx, |view, _| view.settings_scroll.offset());
        cx.simulate_event(gpui::ScrollWheelEvent {
            position,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-10000.))),
            ..Default::default()
        });
        cx.run_until_parked();
        cx.simulate_click(position, Default::default());
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            let locales = crate::infrastructure::voice::native_locales();
            let selected = view
                .presenter
                .model()
                .voice
                .settings
                .locale
                .as_ref()
                .unwrap();
            assert!(
                locales[locales.len() / 2..].contains(selected),
                "scrolling the language menu should reach its lower entries: {selected}"
            );
            assert_eq!(view.settings_scroll.offset(), settings_offset);
        });
    }
    view.update(cx, |view, cx| {
        view.presenter
            .select_voice_provider(Provider::Mimo)
            .unwrap();
        cx.notify();
    });
    cx.run_until_parked();
    let bounds = cx.debug_bounds("voice-mimo-config").unwrap();
    assert!(bounds.left() >= px(0.) && bounds.right() <= px(1040.));
    view.update_in(cx, |view, window, cx| {
        assert!(!view.presenter.model().voice.ready());
        view.settings_open = false;
        view.prompt_input.update(cx, |input, cx| {
            input.set_value("已有草稿：用户刚编辑", window, cx)
        });
        view.append_voice_text("检查 src/main.rs 与 parseHTTP", window, cx);
        assert_eq!(
            view.prompt_input.read(cx).value(),
            "已有草稿：用户刚编辑\n检查 src/main.rs 与 parseHTTP"
        );
        assert!(
            view.presenter
                .model()
                .conversation
                .queued_messages
                .is_empty()
        );
        assert!(view.presenter.model().conversation.active_run.is_none());
        view.focus_prompt(window, cx);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("voice-record").is_some());
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-z"
    } else {
        "ctrl-z"
    });
    view.read_with(cx, |view, cx| {
        assert_eq!(view.prompt_input.read(cx).value(), "已有草稿：用户刚编辑")
    });
}

#[gpui::test]
fn cli_installation_settings_show_the_action_and_completion_feedback(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, _, _directory) = fixture();
    let sender = crate::presenter::tests::pending_cli_installation(&mut presenter);
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    cx.simulate_resize(gpui::size(px(1040.), px(680.)));
    view.update_in(cx, |view, _, cx| {
        view.settings_open = true;
        view.settings_section = SettingsSection::General;
        cx.notify();
    });
    cx.run_until_parked();
    view.update_in(cx, |view, _, cx| {
        view.settings_scroll
            .set_offset(gpui::point(px(0.), px(-10_000.)));
        cx.notify();
    });
    cx.run_until_parked();
    click_debug(cx, "install-cli");
    assert!(view.read_with(cx, |view, _| view.presenter.model().cli_installation_busy));
    assert!(cx.debug_bounds("cli-installation-message").is_none());
    sender.send(Ok(())).unwrap();
    for (width, height, language) in [
        (1040., 680., Language::Chinese),
        (1280., 900., Language::English),
    ] {
        cx.simulate_resize(gpui::size(px(width), px(height)));
        view.update_in(cx, |view, window, cx| {
            view.presenter.drain_events();
            view.set_language(language, window, cx);
            view.settings_scroll
                .set_offset(gpui::point(px(0.), px(-10_000.)));
            cx.notify();
        });
        cx.run_until_parked();
        for selector in ["install-cli", "cli-installation-message"] {
            let bounds = cx.debug_bounds(selector).unwrap();
            assert!(
                bounds.left() >= px(0.) && bounds.right() <= px(width),
                "{selector}: {bounds:?}"
            );
            assert!(
                bounds.top() >= px(0.) && bounds.bottom() <= px(height),
                "{selector}: {bounds:?}"
            );
        }
        assert!(!view.read_with(cx, |view, _| view.presenter.model().cli_installation_busy));
    }
}

#[gpui::test]
fn harness_settings_show_versions_sources_and_only_available_actions(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, _, _directory) = fixture();
    crate::presenter::tests::seed_harness_installations(&mut presenter);
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    for (width, height, language) in [
        (1040., 680., Language::Chinese),
        (1280., 900., Language::English),
    ] {
        cx.simulate_resize(gpui::size(px(width), px(height)));
        view.update_in(cx, |view, window, cx| {
            view.settings_open = true;
            view.settings_section = SettingsSection::Agent;
            view.set_language(language, window, cx);
            view.reduced_motion = true;
            cx.notify();
        });
        cx.run_until_parked();
        for selector in [
            "harness-card-claude",
            "harness-card-codex",
            "harness-card-omp",
            "harness-current-version-claude",
            "harness-latest-version-claude",
            "harness-current-version-codex",
            "harness-latest-version-codex",
            "harness-current-version-omp",
            "harness-latest-version-omp",
        ] {
            let bounds = cx.debug_bounds(selector).unwrap();
            assert!(
                bounds.left() >= px(0.) && bounds.right() <= px(width),
                "{selector}: {bounds:?}"
            );
        }
        assert!(cx.debug_bounds("harness-update-claude").is_some());
        assert!(cx.debug_bounds("harness-install-codex").is_some());
        assert!(cx.debug_bounds("harness-update-omp").is_none());
        assert!(cx.debug_bounds("harness-install-claude").is_none());
        for latest in [
            Ok("1.0.0"),
            Ok("0.9.0"),
            Err("最新版本检测失败：网络不可用"),
        ] {
            view.update_in(cx, |view, _, cx| {
                crate::presenter::tests::seed_harness_installations(&mut view.presenter)
                    .get_mut(&HarnessKind::Claude)
                    .unwrap()
                    .latest_version = latest
                    .map(str::to_owned)
                    .map_err(|error| error.to_owned().into());
                cx.notify();
            });
            cx.run_until_parked();
            assert!(cx.debug_bounds("harness-update-claude").is_none());
            assert!(cx.debug_bounds("harness-latest-version-claude").is_some());
            assert!(cx.debug_bounds("harness-install-codex").is_some());
        }
        view.update_in(cx, |view, _, cx| {
            crate::presenter::tests::seed_harness_installations(&mut view.presenter);
            cx.notify();
        });
        cx.run_until_parked();
    }
    click_debug(cx, "harness-install-codex");
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |view, _| view
            .presenter
            .model()
            .conversation
            .selected_harness),
        HarnessKind::Claude
    );
}
