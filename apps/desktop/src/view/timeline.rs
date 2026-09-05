use super::*;
use crate::model::tools::{TimelineItem, timeline_items};

impl NexusView {
    pub(super) fn render_timeline(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let model = self.presenter.model();
        let compact = window.viewport_size().height < px(740.);
        let history = model.selected_codex_thread.is_some();
        let empty = if history {
            model.codex_history_messages.is_empty()
        } else {
            model.messages.is_empty() && model.streaming_text.is_empty()
        };
        div()
            .relative()
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
                            .max_w(px(if empty { 880. } else { 784. }))
                            .min_h_full()
                            .mx_auto()
                            .px_8()
                            .py(px(if compact { 16. } else { 32. }))
                            .flex()
                            .flex_col()
                            .gap(px(20.))
                            .when(empty, |element| element.child(self.render_welcome(compact)))
                            .when(!history, |element| {
                                element.children(timeline_items(&model.messages).iter().map(
                                    |item| match item {
                                        TimelineItem::Message(message) => {
                                            self.render_message(message, window, cx)
                                        }
                                        TimelineItem::Tools(batch) => {
                                            self.render_tool_batch(batch, window, cx)
                                        }
                                    },
                                ))
                            })
                            .when(history, |element| {
                                element.children(
                                    model.codex_history_messages.iter().enumerate().map(
                                        |(index, message)| {
                                            self.render_history_message(index, message, window, cx)
                                        },
                                    ),
                                )
                            })
                            .when(!history && !model.streaming_text.is_empty(), |element| {
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
                                !history
                                    && model.active_run.is_some()
                                    && model.selected_task == model.active_task,
                                |element| {
                                    element.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_3()
                                            .text_sm()
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child(live_status_dot(
                                                rgb(ACCENT).into(),
                                                !self.reduced_motion,
                                            ))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .child(model.status.clone()),
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
                                                            .text_xs()
                                                            .text_color(rgb(MUTED))
                                                            .child("已运行")
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
                                    .outline()
                                    .small()
                                    .h(px(COMPACT_CONTROL_HEIGHT))
                                    .rounded(px(CONTROL_RADIUS))
                                    .icon(IconName::ArrowDown)
                                    .label("回到最新消息")
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.timeline_scroll.scroll_to_bottom();
                                        cx.notify();
                                    })),
                            ),
                    )
                },
            )
    }

    fn render_welcome(&self, compact: bool) -> impl IntoElement {
        let model = self.presenter.model();
        let history = model.selected_codex_thread.is_some();
        if !history {
            // Brand SVGs from LobeHub Icons; see assets/harness/LICENSE.
            let (icon, color): (&[u8], _) = match model.selected_harness {
                HarnessKind::Claude => (
                    include_bytes!("../../assets/harness/claude.svg"),
                    rgb(0xd97757),
                ),
                HarnessKind::Codex => (include_bytes!("../../assets/harness/codex.svg"), rgb(TEXT)),
                HarnessKind::Omp => (
                    include_bytes!("../../assets/harness/omp.svg"),
                    rgb(0xf97316),
                ),
            };
            return div()
                .flex_1()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .py(px(if compact { 8. } else { 32. }))
                .text_center()
                .child(
                    gpui::svg()
                        .data(icon)
                        .size(px(if compact { 40. } else { 48. }))
                        .flex_none()
                        .text_color(color),
                )
                .child(
                    div()
                        .mt_3()
                        .text_size(px(if compact { 24. } else { 28. }))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .line_height(relative(1.25))
                        .child(model.selected_harness.to_string()),
                )
                .map(|element| entrance(element, "empty-state-enter", !self.reduced_motion));
        }
        let (eyebrow, title, description) = if model.codex_thread_loading {
            (
                "CODEX HISTORY",
                "正在读取会话",
                "正在从本机加载消息，稍等片刻。",
            )
        } else {
            (
                "CODEX HISTORY",
                "此会话没有消息",
                "这条历史记录中没有可显示的用户或助手消息。",
            )
        };
        div()
            .flex_1()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .py(px(if compact { 8. } else { 32. }))
            .text_center()
            .child(brand_mark(if compact { 40. } else { 48. }))
            .when(!compact, |element| {
                element.child(
                    div()
                        .mt_6()
                        .text_xs()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(rgb(MUTED))
                        .child(eyebrow),
                )
            })
            .child(
                div()
                    .mt_3()
                    .text_size(px(if compact { 24. } else { 28. }))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .line_height(relative(1.25))
                    .child(title),
            )
            .child(
                div()
                    .mt_3()
                    .text_base()
                    .text_color(rgb(MUTED))
                    .line_height(relative(1.6))
                    .child(description),
            )
            .when(model.codex_thread_loading, |element| {
                element.child(
                    div()
                        .mt_8()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_color(rgb(MUTED))
                        .child(live_status_dot(rgb(ACCENT).into(), !self.reduced_motion))
                        .child("正在加载…"),
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
        assert!(presenter.submit(&"Long prompt for scrolling.\n\n".repeat(80), "claude"));
        let run_id = presenter.model().active_run.unwrap();
        let active_task = presenter.model().selected_task.unwrap();
        let now = Instant::now();
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = NexusView::new(presenter, window, cx);
            view.reduced_motion = true;
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
