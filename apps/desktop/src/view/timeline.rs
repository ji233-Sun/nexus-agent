use super::*;
use crate::model::tools::{TimelineItem, timeline_items};
use gpui_kit::component::button::ButtonCustomVariant;

impl NexusView {
    pub(super) fn render_timeline(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let model = self.presenter.model();
        let compact = window.viewport_size().height < px(740.);
        let empty = model.messages.is_empty() && model.streaming_text.is_empty();
        div()
            .relative()
            .bg(rgb(colors.canvas))
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("timeline")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .lock_scroll_axis()
                    .track_scroll(&self.timeline_scroll)
                    .child(
                        div()
                            .w_full()
                            .max_w(px(CONTENT_WIDTH + 48.))
                            .min_h_full()
                            .mx_auto()
                            .px(px(24.))
                            .py(px(if compact { 16. } else { 32. }))
                            .flex()
                            .flex_col()
                            .gap(px(24.))
                            .when(empty, |element| {
                                element.child(self.render_welcome(colors, compact, cx))
                            })
                            .children(timeline_items(&model.messages).iter().map(
                                |item| match item {
                                    TimelineItem::Message(message) => {
                                        self.render_message(message, window, cx)
                                    }
                                    TimelineItem::Tools(batch) => {
                                        self.render_tool_batch(batch, window, cx)
                                    }
                                },
                            ))
                            .when(!model.streaming_text.is_empty(), |element| {
                                element.child(self.message_card(
                                    "streaming-message",
                                    MessageRole::Assistant,
                                    &model.streaming_text,
                                    MessageKind::Text,
                                    window,
                                    cx,
                                ))
                            })
                            .when(
                                model.active_run.is_some()
                                    && model.selected_task == model.active_task,
                                |element| {
                                    element.child(
                                        div()
                                            .debug_selector(|| "conversation-run-status".into())
                                            .flex()
                                            .items_center()
                                            .gap_3()
                                            .text_size(px(13.))
                                            .text_color(rgb(colors.text_secondary))
                                            .child(live_status_dot(
                                                rgb(colors.accent).into(),
                                                !self.reduced_motion,
                                            ))
                                            .child(
                                                div().flex_1().min_w_0().child(
                                                    model.run_status.render(locale).to_owned(),
                                                ),
                                            )
                                            .when_some(
                                                model.active_run_elapsed_seconds,
                                                |element, seconds| {
                                                    element.child(
                                                        div()
                                                            .debug_selector(|| "run-elapsed".into())
                                                            .w(px(152.))
                                                            .flex_none()
                                                            .flex()
                                                            .items_center()
                                                            .justify_end()
                                                            .gap_2()
                                                            .text_size(px(12.))
                                                            .text_color(rgb(colors.muted))
                                                            .child(locale.text("已运行"))
                                                            .child(
                                                                div().font_family(MONO_FONT).child(
                                                                    format_run_elapsed(seconds),
                                                                ),
                                                            ),
                                                    )
                                                },
                                            ),
                                    )
                                },
                            ),
                    ),
            )
            .when(
                !empty
                    && self.timeline_scroll.max_offset().y + self.timeline_scroll.offset().y
                        > px(48.),
                |element| {
                    let latest_message_background: Hsla = rgb(0x202020).into();
                    element.child(
                        div()
                            .absolute()
                            .bottom(px(12.))
                            .left_0()
                            .right_0()
                            .flex()
                            .justify_center()
                            .child(
                                Button::new("latest-message")
                                    .custom(
                                        ButtonCustomVariant::new(cx)
                                            .color(latest_message_background)
                                            .foreground(rgb(0xffffff).into())
                                            .hover(rgb(0x303030).into())
                                            .active(rgb(0x101010).into()),
                                    )
                                    .outline()
                                    .small()
                                    .h(px(COMPACT_CONTROL_HEIGHT))
                                    .rounded(px(CONTROL_RADIUS))
                                    // Custom variants soften their normal fill; keep this overlay opaque.
                                    .bg(latest_message_background)
                                    .shadow(materials(cx).shadow())
                                    .icon(IconName::ArrowDown)
                                    .label(locale.text("回到最新消息"))
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.timeline_scroll.scroll_to_bottom();
                                        cx.notify();
                                    })),
                            ),
                    )
                },
            )
    }

    fn render_welcome(
        &self,
        colors: Palette,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let model = self.presenter.model();
        let project = model.selected_project.as_ref();
        let has_project = project.is_some();
        let selector = if has_project {
            "workspace-empty-agent-status"
        } else {
            "workspace-empty-no-project"
        };
        let context = project
            .map(|project| project.display_name.clone())
            .unwrap_or_else(|| "Nexus Agent".into());
        let title = if has_project {
            locale.text("今天想完成什么？")
        } else {
            locale.text("从一个想法开始")
        };
        div()
            .debug_selector(move || selector.into())
            .flex_1()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .py(px(if compact { 8. } else { 32. }))
            .text_center()
            .child(brand_mark(if compact { 48. } else { 64. }))
            .child(
                div()
                    .debug_selector(|| "workspace-empty-context".into())
                    .mt_5()
                    .max_w_full()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(rgb(colors.muted))
                    .child(context),
            )
            .child(
                div()
                    .mt_3()
                    .text_size(px(if compact { 26. } else { 32. }))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .line_height(relative(1.25))
                    .child(title),
            )
            .when(!has_project, |element| {
                element.child(
                    div()
                        .mt_3()
                        .max_w(px(520.))
                        .text_size(px(14.))
                        .text_color(rgb(colors.muted))
                        .line_height(relative(1.55))
                        .child(locale.text("先选择本地项目，再描述你希望完成的工作。")),
                )
            })
            .when(has_project, |element| {
                element.child(
                    div()
                        .mt_5()
                        .px_3()
                        .py_2()
                        .rounded(px(CONTROL_RADIUS))
                        .bg(rgb(colors.elevated))
                        .border_1()
                        .border_color(rgb(colors.border))
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(12.))
                        .text_color(rgb(colors.text_secondary))
                        .child(harness_icon(model.selected_harness, colors, 16.))
                        .child(model.selected_harness.to_string()),
                )
            })
            .when(!has_project, |element| {
                element.child(
                    Button::new("welcome-choose-project")
                        .debug_selector(|| "welcome-choose-project".into())
                        .mt_5()
                        .primary()
                        .h(px(CONTROL_HEIGHT))
                        .icon(IconName::FolderOpen)
                        .label(locale.text("选择项目"))
                        .on_click(cx.listener(Self::choose_project)),
                )
            })
            .map(|element| entrance(element, "empty-state-enter", !self.reduced_motion))
    }
}

fn format_run_elapsed(seconds: u64) -> String {
    if seconds < 3600 {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    } else {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presenter::tests::fixture;
    use gpui::{TestAppContext, point, size};
    use nexus_protocol::Event;

    #[gpui::test]
    fn run_elapsed_repaints_without_output_or_scroll_jumps_and_clears_on_exit(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, runner, _directory) = fixture();
        assert!(presenter.submit("previous task", "claude"));
        let previous_task = presenter.model().selected_task.unwrap();
        runner.emit(Event::RunExited {
            run_id: presenter.model().active_run.unwrap(),
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        presenter.new_task();
        assert!(presenter.submit(&"Long prompt for scrolling.\n\n".repeat(80), "claude"));
        let run_id = presenter.model().active_run.unwrap();
        let active_task = presenter.model().selected_task.unwrap();
        let now = Instant::now();
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = NexusView::new(presenter, window, cx);
            view.set_appearance(
                AppearanceSettings {
                    reduced_motion: true,
                    ..view.presenter.model().appearance
                },
                window,
                cx,
            );
            view
        });
        for (width, height, seconds) in [(1040., 680., 65), (1280., 800., 3601)] {
            cx.simulate_resize(size(px(width), px(height)));
            cx.run_until_parked();
            let (scroll, timeline) = view.read_with(cx, |view, _| {
                (view.timeline_scroll.clone(), view.timeline_pane.clone())
            });
            scroll.scroll_to_bottom();
            timeline.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            let timer_bounds = cx.debug_bounds("run-elapsed").unwrap();
            assert_eq!(timer_bounds.size.width, px(152.));
            assert!(scroll.bounds().contains(&timer_bounds.center()));
            let before = point(px(0.), scroll.offset().y + px(20.));
            scroll.set_offset(before);
            timeline.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            let timeline_renders = timeline.read_with(cx, |pane, _| pane.render_count);

            view.update(cx, |view, cx| {
                view.poll_events(now + Duration::from_secs(seconds), cx);
                assert!(view.presenter.model().active_run_elapsed_seconds.unwrap() >= seconds);
            });
            cx.run_until_parked();

            assert_eq!(scroll.offset(), before);
            assert_eq!(
                cx.debug_bounds("run-elapsed").unwrap().size,
                timer_bounds.size
            );
            assert!(timeline.read_with(cx, |pane, _| pane.render_count) > timeline_renders);
        }

        view.update(cx, |view, cx| {
            view.presenter.select_task(previous_task);
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("run-elapsed").is_none());
        view.update(cx, |view, cx| {
            view.presenter.select_task(active_task);
            view.timeline_scroll.scroll_to_bottom();
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("run-elapsed").is_some());

        runner.emit(Event::RunExited {
            run_id,
            status: RunStatus::Cancelled,
            exit_code: None,
        });
        view.update(cx, |view, cx| {
            view.poll_events(now + Duration::from_secs(3602), cx)
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("run-elapsed").is_none());
        let timeline = view.read_with(cx, |view, _| view.timeline_pane.clone());
        let renders = timeline.read_with(cx, |pane, _| pane.render_count);
        view.update(cx, |view, cx| {
            view.poll_events(now + Duration::from_secs(3603), cx)
        });
        cx.run_until_parked();
        assert_eq!(timeline.read_with(cx, |pane, _| pane.render_count), renders);
    }

    #[test]
    fn run_elapsed_formats_seconds_minutes_and_hours_without_wrapping() {
        for (seconds, expected) in [
            (0, "00:00"),
            (59, "00:59"),
            (60, "01:00"),
            (3599, "59:59"),
            (3600, "1:00:00"),
            (3661, "1:01:01"),
            (90_061, "25:01:01"),
        ] {
            assert_eq!(format_run_elapsed(seconds), expected);
        }
    }
}
