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
                                            .child(model.status.clone()),
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
