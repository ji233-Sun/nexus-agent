use super::*;

#[gpui::test]
#[ignore]
fn scroll_frame_cost(cx: &mut TestAppContext) {
    let (view, cx) = scroll_test_view(cx);
    for (name, handle, pane) in view.read_with(cx, |view, _| {
        [
            (
                "sidebar",
                view.sidebar_scroll.clone(),
                view.sidebar_pane.clone(),
            ),
            (
                "timeline",
                view.timeline_scroll.clone(),
                view.timeline_pane.clone(),
            ),
        ]
    }) {
        assert!(handle.max_offset().y > px(60.));
        handle.set_offset(point(px(0.), px(0.)));
        pane.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        let mut samples = Vec::new();
        let before = pane.read_with(cx, |pane, _| pane.render_count);
        for frame in 0..140 {
            let delta = px(if frame % 40 < 20 { -3. } else { 3. });
            let previous = handle.offset().y;
            let started = Instant::now();
            cx.simulate_event(ScrollWheelEvent {
                position: handle.bounds().center(),
                delta: ScrollDelta::Pixels(point(px(0.), delta)),
                touch_phase: gpui::TouchPhase::Moved,
                ..Default::default()
            });
            if frame >= 20 {
                samples.push(started.elapsed().as_secs_f64() * 1000.);
            }
            assert_eq!(handle.offset().y, previous + delta);
        }
        let renders = pane.read_with(cx, |pane, _| pane.render_count) - before;
        assert!(renders >= 140);
        samples.sort_by(f64::total_cmp);
        eprintln!(
            "{name}: CPU ms/event median={:.2}, p95={:.2}; pane renders={}",
            samples[samples.len() / 2],
            samples[samples.len() * 95 / 100],
            renders,
        );
    }
}

#[gpui::test]
fn sidebar_scrollbar_tracks_scrolling_and_dragging(cx: &mut TestAppContext) {
    let (view, cx) = scroll_test_view(cx);
    cx.update(|window, cx| {
        gpui_kit::component::Theme::set_scrollbar_mode(
            gpui_kit::component::scroll::ScrollbarMode::Always,
            cx,
        );
        window.refresh();
        let _ = window.draw(cx);
    });
    let scroll = view.read_with(cx, |view, _| view.sidebar_scroll.clone());
    let viewport = scroll.bounds();
    let device_pixel = cx.update(|window, _| px(1. / window.scale_factor()));
    let content_height = scroll.bounds_for_item(0).unwrap().size.height;
    let max_offset = (content_height - viewport.size.height).max(px(0.));
    assert_eq!(
        scroll.max_offset().y,
        max_offset,
        "only navigation content may contribute to the scroll range"
    );
    let painted_thumbs = |cx: &mut gpui::VisualTestContext| painted_scrollbar_thumbs(cx, viewport);
    let thumb_bounds = |cx: &mut gpui::VisualTestContext| {
        let thumbs = painted_thumbs(cx);
        assert_eq!(thumbs.len(), 1, "expected one painted sidebar thumb");
        thumbs[0]
    };
    let initial_thumb = thumb_bounds(cx);
    assert!((initial_thumb.top() - viewport.top()).abs() < px(8.));
    for delta in [
        ScrollDelta::Pixels(point(px(0.), px(-80.))),
        ScrollDelta::Lines(point(0., -3.)),
    ] {
        let before = thumb_bounds(cx);
        cx.simulate_event(ScrollWheelEvent {
            position: viewport.center(),
            delta,
            ..Default::default()
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let after = thumb_bounds(cx);
        assert!(
            after.top() > before.top(),
            "thumb must follow downward scrolling"
        );
        // Painting snaps each edge separately, so translation can change the
        // painted height by one device pixel without changing the layout.
        assert!(
            (after.size.height - initial_thumb.size.height).abs() <= device_pixel,
            "thumb height must stay constant within one device pixel: initial={initial_thumb:?}, after={after:?}"
        );
        assert_eq!(scroll.bounds(), viewport);
    }

    let thumb = thumb_bounds(cx);
    let before_drag = scroll.offset().y;
    let destination = thumb.center() + point(px(0.), px(40.));
    cx.simulate_mouse_down(thumb.center(), gpui::MouseButton::Left, Default::default());
    cx.simulate_mouse_move(destination, gpui::MouseButton::Left, Default::default());
    cx.simulate_mouse_up(destination, gpui::MouseButton::Left, Default::default());
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(
        scroll.offset().y < before_drag,
        "dragging must scroll the sidebar"
    );
    assert!(thumb_bounds(cx).top() > thumb.top());

    scroll.scroll_to_bottom();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    let bottom_thumb = thumb_bounds(cx);
    assert!(bottom_thumb.top() > initial_thumb.top());
    assert!((bottom_thumb.bottom() - viewport.bottom()).abs() < px(8.));
    assert_eq!(scroll.offset().y, -max_offset);
    assert_eq!(
        scroll.bounds_for_item(0).unwrap().bottom() + scroll.offset().y,
        viewport.bottom(),
        "the last content edge must meet the viewport bottom without blank overscroll"
    );

    for (destination_y, expected_offset, delta_y) in [
        (viewport.top() - px(500.), px(0.), 10_000.),
        (viewport.bottom() + px(500.), -max_offset, -10_000.),
    ] {
        let thumb = thumb_bounds(cx);
        let destination = point(thumb.center().x, destination_y);
        cx.simulate_mouse_down(thumb.center(), gpui::MouseButton::Left, Default::default());
        cx.simulate_mouse_move(destination, gpui::MouseButton::Left, Default::default());
        cx.simulate_mouse_up(destination, gpui::MouseButton::Left, Default::default());
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert_eq!(scroll.offset().y, expected_offset);
        for delta in [
            ScrollDelta::Pixels(point(px(0.), px(delta_y))),
            ScrollDelta::Lines(point(0., delta_y)),
        ] {
            cx.simulate_event(ScrollWheelEvent {
                position: viewport.center(),
                delta,
                ..Default::default()
            });
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert_eq!(scroll.offset().y, expected_offset);
            assert_eq!(scroll.max_offset().y, max_offset);
            let thumb = thumb_bounds(cx);
            assert!(thumb.top() >= viewport.top());
            assert!(thumb.bottom() <= viewport.bottom());
        }
    }

    view.update(cx, |view, cx| {
        let project_id = view
            .presenter
            .model()
            .conversation
            .selected_project
            .as_ref()
            .unwrap()
            .id;
        view.collapsed_projects.insert(project_id);
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(scroll.bounds_for_item(0).unwrap().size.height < viewport.size.height);
    assert_eq!(scroll.max_offset().y, px(0.));
    assert_eq!(scroll.offset().y, px(0.));
    assert!(painted_thumbs(cx).is_empty());
    for delta_y in [-10_000., 10_000.] {
        cx.simulate_event(ScrollWheelEvent {
            position: viewport.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(delta_y))),
            ..Default::default()
        });
        assert_eq!(scroll.offset().y, px(0.));
    }
}

#[gpui::test]
fn settings_scrollbar_tracks_scrolling_and_dragging(cx: &mut TestAppContext) {
    let (view, cx) = scroll_test_view(cx);
    view.update_in(cx, |view, window, cx| {
        gpui_kit::component::Theme::set_scrollbar_mode(
            gpui_kit::component::scroll::ScrollbarMode::Always,
            cx,
        );
        view.toggle_settings(window, cx);
    });
    let scroll = view.read_with(cx, |view, _| view.settings_scroll.clone());
    let device_pixel = cx.update(|window, _| px(1. / window.scale_factor()));

    for size in [
        gpui::size(px(1040.), px(680.)),
        gpui::size(px(1280.), px(800.)),
    ] {
        cx.simulate_resize(size);
        for section in [
            SettingsSection::General,
            SettingsSection::Providers,
            SettingsSection::Appearance,
        ] {
            view.update_in(cx, |view, window, cx| {
                view.select_settings_section(section, window, cx);
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let viewport = scroll.bounds();
            let max_offset = scroll.max_offset().y;
            assert!(max_offset > px(80.));
            let thumb_bounds = |cx: &mut gpui::VisualTestContext| {
                let thumbs = painted_scrollbar_thumbs(cx, viewport);
                assert_eq!(thumbs.len(), 1, "expected one painted settings thumb");
                thumbs[0]
            };
            let initial_thumb = thumb_bounds(cx);
            assert!((initial_thumb.top() - viewport.top()).abs() < px(8.));

            for delta in [
                ScrollDelta::Pixels(point(px(0.), px(-80.))),
                ScrollDelta::Lines(point(0., -3.)),
            ] {
                let before = thumb_bounds(cx);
                cx.simulate_event(ScrollWheelEvent {
                    position: viewport.center(),
                    delta,
                    ..Default::default()
                });
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                let after = thumb_bounds(cx);
                assert!(
                    after.top() > before.top(),
                    "settings thumb must follow downward scrolling: before={before:?}, after={after:?}"
                );
                assert!((after.size.height - initial_thumb.size.height).abs() <= device_pixel);
                assert_eq!(scroll.bounds(), viewport);
                assert_eq!(scroll.max_offset().y, max_offset);
            }

            for (destination_y, expected_offset) in [
                (viewport.bottom() + px(500.), -max_offset),
                (viewport.top() - px(500.), px(0.)),
            ] {
                let thumb = thumb_bounds(cx);
                let destination = point(thumb.center().x, destination_y);
                cx.simulate_mouse_down(thumb.center(), gpui::MouseButton::Left, Default::default());
                // Scrollbar drag updates use a wall-clock 120 Hz throttle.
                std::thread::sleep(Duration::from_millis(16));
                cx.simulate_mouse_move(destination, gpui::MouseButton::Left, Default::default());
                cx.simulate_mouse_up(destination, gpui::MouseButton::Left, Default::default());
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                assert_eq!(scroll.offset().y, expected_offset);
                assert_eq!(scroll.max_offset().y, max_offset);
                let thumb = thumb_bounds(cx);
                assert!(thumb.top() >= viewport.top());
                assert!(thumb.bottom() <= viewport.bottom());
                if expected_offset == -max_offset {
                    assert!((thumb.bottom() - viewport.bottom()).abs() < px(8.));
                    assert_eq!(
                        scroll.bounds_for_item(0).unwrap().bottom() + scroll.offset().y,
                        viewport.bottom(),
                        "padded settings content must end at the viewport bottom"
                    );
                } else {
                    assert!((thumb.top() - viewport.top()).abs() < px(8.));
                }
            }
        }

        view.update_in(cx, |view, window, cx| {
            view.select_settings_section(SettingsSection::Archived, window, cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert_eq!(scroll.max_offset().y, px(0.));
        assert_eq!(scroll.offset().y, px(0.));
        assert!(painted_scrollbar_thumbs(cx, scroll.bounds()).is_empty());
    }
}

#[gpui::test]
fn scroll_regions_keep_offsets_and_rendering_independent(cx: &mut TestAppContext) {
    let (view, cx) = scroll_test_view(cx);
    // Keep the provider form taller than the viewport so it really scrolls.
    cx.simulate_resize(gpui::size(px(1280.), px(800.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let scroll = view.read_with(cx, |view, _| view.timeline_scroll.clone());
    assert!(scroll.max_offset().y > px(100.));
    scroll.set_offset(point(px(0.), px(-100.)));
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let (sidebar_scroll, timeline_pane, sidebar_pane) = view.read_with(cx, |view, _| {
        (
            view.sidebar_scroll.clone(),
            view.timeline_pane.clone(),
            view.sidebar_pane.clone(),
        )
    });
    let timeline_renders = timeline_pane.read_with(cx, |pane, _| pane.render_count);
    let sidebar_renders = sidebar_pane.read_with(cx, |pane, _| pane.render_count);
    let before = scroll.offset();
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(100.), px(350.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-80.))),
        ..Default::default()
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert_eq!(scroll.offset(), before);
    assert_eq!(sidebar_scroll.offset().y, px(-80.));
    assert_eq!(
        timeline_pane.read_with(cx, |pane, _| pane.render_count),
        timeline_renders
    );
    assert!(sidebar_pane.read_with(cx, |pane, _| pane.render_count) > sidebar_renders);

    cx.simulate_event(ScrollWheelEvent {
        position: scroll.bounds().center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
        ..Default::default()
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert_eq!(scroll.offset().y, before.y - px(40.));
    assert_eq!(sidebar_scroll.offset().y, px(-80.));
    let workspace_bounds = cx.debug_bounds("workspace-page").unwrap();
    let settings_button = cx.debug_bounds("sidebar-settings").unwrap().center();
    assert!(workspace_bounds.contains(&settings_button));
    cx.simulate_click(settings_button, Default::default());
    assert!(view.read_with(cx, |view, _| view.settings_open));
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    let providers_tab = cx.debug_bounds("settings-nav-providers").unwrap().center();
    cx.simulate_click(providers_tab, Default::default());
    cx.run_until_parked();
    let settings_scroll = view.read_with(cx, |view, _| view.settings_scroll.clone());
    assert_eq!(cx.debug_bounds("settings-page").unwrap(), workspace_bounds);
    assert!(cx.debug_bounds("workspace-page").is_none());
    assert!(cx.debug_bounds("sidebar-settings").is_none());
    assert!(settings_scroll.max_offset().y > px(40.));
    let back_button = cx.debug_bounds("back-to-workspace").unwrap();
    let navigation = cx.debug_bounds("settings-navigation").unwrap();
    let breadcrumb = cx.debug_bounds("settings-breadcrumb").unwrap();
    assert!(navigation.right() <= settings_scroll.bounds().left());
    let timeline_renders = timeline_pane.read_with(cx, |pane, _| pane.render_count);
    let sidebar_renders = sidebar_pane.read_with(cx, |pane, _| pane.render_count);
    let before = scroll.offset();
    cx.simulate_event(ScrollWheelEvent {
        position: settings_scroll.bounds().center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
        ..Default::default()
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert_eq!(scroll.offset(), before);
    assert_eq!(sidebar_scroll.offset().y, px(-80.));
    assert_eq!(settings_scroll.offset().y, px(-40.));
    assert_eq!(cx.debug_bounds("back-to-workspace").unwrap(), back_button);
    assert_eq!(cx.debug_bounds("settings-navigation").unwrap(), navigation);
    assert_eq!(cx.debug_bounds("settings-breadcrumb").unwrap(), breadcrumb);
    assert_eq!(
        timeline_pane.read_with(cx, |pane, _| pane.render_count),
        timeline_renders
    );
    assert_eq!(
        sidebar_pane.read_with(cx, |pane, _| pane.render_count),
        sidebar_renders
    );

    cx.simulate_event(ScrollWheelEvent {
        position: navigation.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
        ..Default::default()
    });
    cx.simulate_click(providers_tab, Default::default());
    cx.run_until_parked();
    assert_eq!(settings_scroll.offset().y, px(-40.));
    let general_tab = cx.debug_bounds("settings-nav-general").unwrap().center();
    cx.simulate_click(general_tab, Default::default());
    cx.run_until_parked();
    assert_eq!(settings_scroll.offset().y, px(0.));
    assert!(cx.debug_bounds("settings-content-general").is_some());
    assert!(cx.debug_bounds("settings-content-providers").is_none());
    cx.simulate_click(providers_tab, Default::default());
    cx.run_until_parked();
    assert_eq!(settings_scroll.offset().y, px(0.));

    cx.simulate_click(back_button.center(), Default::default());
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("settings-page").is_none());
    assert_eq!(cx.debug_bounds("workspace-page").unwrap(), workspace_bounds);
    assert_eq!(scroll.offset(), before);
    assert_eq!(sidebar_scroll.offset().y, px(-80.));
    let before = scroll.offset();
    cx.simulate_event(ScrollWheelEvent {
        position: scroll.bounds().center(),
        delta: ScrollDelta::Pixels(point(px(-80.), px(0.))),
        touch_phase: gpui::TouchPhase::Started,
        ..Default::default()
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert_eq!(scroll.offset(), before);
}

#[gpui::test]
fn trackpad_preserves_small_diagonal_deltas_and_momentum(cx: &mut TestAppContext) {
    let (view, cx) = scroll_test_view(cx);
    let handles = view.read_with(cx, |view, _| {
        [view.sidebar_scroll.clone(), view.timeline_scroll.clone()]
    });
    for reduced_motion in [true, false] {
        for scroll in &handles {
            scroll.set_offset(point(px(0.), px(-100.)));
            view.update_in(cx, |view, window, cx| {
                view.set_appearance(
                    AppearanceSettings {
                        reduced_motion,
                        ..view.presenter.model().appearance
                    },
                    window,
                    cx,
                );
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            for (dx, dy, phase) in [
                (0.9, -0.5, gpui::TouchPhase::Started),
                (0.1, -0.25, gpui::TouchPhase::Moved),
                (0.1, 0.125, gpui::TouchPhase::Moved),
                (0., -0.0625, gpui::TouchPhase::Ended),
                (0., -0.03125, gpui::TouchPhase::Moved),
                (3., 0., gpui::TouchPhase::Moved),
            ] {
                let before = scroll.offset();
                cx.simulate_event(ScrollWheelEvent {
                    position: scroll.bounds().center(),
                    delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
                    touch_phase: phase,
                    ..Default::default()
                });
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                assert_eq!(scroll.offset(), point(before.x, before.y + px(dy)));
            }
        }
    }
}

#[gpui::test]
fn wheel_smoothing_preserves_native_trackpad_input(cx: &mut TestAppContext) {
    let (view, cx) = scroll_test_view(cx);
    let scroll = view.read_with(cx, |view, _| view.sidebar_scroll.clone());
    let marker = view.read_with(cx, |view, _| {
        format!(
            "sidebar-task-{}",
            view.presenter.model().conversation.tasks[3].id
        )
        .leak()
    });
    let marker_y = cx.debug_bounds(marker).unwrap().top();
    let event = ScrollWheelEvent {
        position: scroll.bounds().center(),
        delta: ScrollDelta::Lines(point(0., -3.)),
        ..Default::default()
    };
    cx.simulate_event(event.clone());
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let native_target = scroll.offset().y;
    assert!(native_target < px(0.));
    assert_eq!(
        cx.debug_bounds(marker).unwrap().top(),
        marker_y + native_target
    );
    scroll.set_offset(point(px(0.), px(0.)));
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
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.simulate_event(event.clone());
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(scroll.offset().y > native_target);
    assert!(scroll.offset().y <= px(0.));
    std::thread::sleep(Duration::from_millis(160));
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    assert_eq!(scroll.offset().y, native_target);

    cx.simulate_event(event.clone());
    let before = scroll.offset().y;
    cx.simulate_event(ScrollWheelEvent {
        position: scroll.bounds().center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-7.5))),
        touch_phase: gpui::TouchPhase::Started,
        ..Default::default()
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert_eq!(scroll.offset().y, before - px(7.5));
    let after = scroll.offset();
    std::thread::sleep(Duration::from_millis(160));
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    assert_eq!(scroll.offset(), after);

    cx.simulate_event(event.clone());
    let reversal_from = scroll.offset().y;
    cx.simulate_event(ScrollWheelEvent {
        delta: ScrollDelta::Lines(point(0., 1.)),
        ..event.clone()
    });
    std::thread::sleep(Duration::from_millis(160));
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    assert_eq!(
        scroll.offset().y,
        (reversal_from - native_target / 3.).min(px(0.))
    );

    cx.simulate_event(event);
    let manual_offset = point(px(0.), -scroll.max_offset().y / 2.);
    scroll.set_offset(manual_offset);
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    assert_eq!(scroll.offset(), manual_offset);
}
