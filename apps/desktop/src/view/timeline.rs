use super::*;

impl NexusView {
    pub(super) fn render_timeline(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let model = self.presenter.model();
        let showing_codex_history = model.selected_codex_thread.is_some();
        let empty = if showing_codex_history {
            model.codex_history_messages.is_empty()
        } else {
            model.messages.is_empty() && model.streaming_text.is_empty()
        };
        div()
            .id("timeline")
            .flex_1()
            .h_full()
            .overflow_y_scroll()
            .bg(rgba(0x101211f2))
            .child(
                div()
                    .w_full()
                    .max_w(px(720.))
                    .min_h_full()
                    .mx_auto()
                    .px_8()
                    .pt_8()
                    .pb_12()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .when(empty, |element| {
                        let (title, description) = if model.codex_thread_loading {
                            ("正在读取 Codex 历史", "正在从本机 Codex 会话中加载消息。")
                        } else if showing_codex_history {
                            ("此会话没有消息", "没有可显示的用户或助手消息。")
                        } else if model.selected_project.is_some() {
                            (
                                "准备开始新任务",
                                "在下方描述目标，Agent 的执行过程会显示在这里。",
                            )
                        } else {
                            (
                                "选择一个本地项目",
                                "选择项目开始任务，或从左侧浏览 Codex 历史。",
                            )
                        };
                        element.child(
                            div()
                                .relative()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap_2()
                                .text_center()
                                .child(
                                    div()
                                        .text_xl()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .max_w(px(420.))
                                        .text_sm()
                                        .text_color(rgb(MUTED))
                                        .line_height(relative(1.5))
                                        .child(description),
                                )
                                .with_animation(
                                    "empty-state-enter",
                                    Animation::new(Duration::from_millis(260))
                                        .with_easing(ease_out_quint()),
                                    |element, delta| {
                                        element.opacity(delta).top(px(8.) - delta * px(8.))
                                    },
                                ),
                        )
                    })
                    .when(!showing_codex_history, |element| {
                        element.children(
                            model
                                .messages
                                .iter()
                                .map(|message| render_message(message, window, cx)),
                        )
                    })
                    .when(showing_codex_history, |element| {
                        element.children(model.codex_history_messages.iter().enumerate().map(
                            |(index, message)| render_history_message(index, message, window, cx),
                        ))
                    })
                    .when(
                        !showing_codex_history && !model.streaming_text.is_empty(),
                        |element| {
                            element.child(message_card(
                                "streaming-message",
                                MessageRole::Assistant,
                                "Agent · 正在生成",
                                &model.streaming_text,
                                MessageKind::Text,
                                window,
                                cx,
                            ))
                        },
                    ),
            )
    }
}
