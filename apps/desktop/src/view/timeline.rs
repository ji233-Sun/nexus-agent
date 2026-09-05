use super::*;

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
                    .track_scroll(&self.timeline_scroll)
                    .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
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
                            .gap(px(24.))
                            .when(empty, |element| {
                                element.child(self.render_welcome(compact, cx))
                            })
                            .when(!history, |element| {
                                element.children(
                                    model
                                        .messages
                                        .iter()
                                        .map(|message| self.render_message(message, window, cx)),
                                )
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
                                    .rounded_full()
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

    fn render_welcome(&self, compact: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let history = model.selected_codex_thread.is_some();
        let (eyebrow, title, description) = if model.codex_thread_loading {
            (
                "CODEX HISTORY",
                "正在读取会话",
                "正在从本机加载消息，稍等片刻。",
            )
        } else if history {
            (
                "CODEX HISTORY",
                "此会话没有消息",
                "这条历史记录中没有可显示的用户或助手消息。",
            )
        } else if model.selected_project.is_some() {
            (
                "LET’S BUILD SOMETHING",
                "从一个想法，开始创造。",
                "理解项目、打磨代码，或解决一个棘手的问题。",
            )
        } else {
            (
                "YOUR LOCAL WORKSPACE",
                "想法在这里，变成现实。",
                "连接一个本地项目，和你的 Agent 一起开始工作。",
            )
        };
        div().flex_1().w_full().flex().flex_col().items_center().justify_center()
            .py(px(if compact { 8. } else { 32. })).text_center()
            .child(brand_mark(if compact { 48. } else { 64. }))
            .when(!compact, |element| element.child(div().mt_6().text_xs().font_weight(gpui::FontWeight::MEDIUM)
                .text_color(rgb(ACCENT)).child(eyebrow)))
            .child(div().mt_3().text_size(px(if compact { 28. } else { 34. })).font_weight(gpui::FontWeight::SEMIBOLD)
                .line_height(relative(1.25)).child(title))
            .child(div().mt_3().text_base().text_color(rgb(MUTED))
                .line_height(relative(1.6)).child(description))
            .when(!history && model.selected_project.is_none(), |element| element.child(
                Button::new("welcome-open-project").primary().small().mt_8().h(px(CONTROL_HEIGHT))
                    .icon(IconName::FolderOpen).label("选择项目目录")
                    .on_click(cx.listener(Self::choose_project))))
            .when(!history && model.selected_project.is_some() && model.active_run.is_none(), |element| {
                element.child(div().mt(px(if compact { 24. } else { 32. })).w_full().flex().flex_wrap().gap_3()
                    .children([
                        ("explore", IconName::BookOpen, "理解项目", "从全局看清代码结构", "梳理项目结构，介绍主要模块及它们之间的关系。"),
                        ("review", IconName::SquareTerminal, "检查代码", "发现值得优先解决的问题", "检查当前项目的代码，找出最值得优先修复的问题，先说明原因和修复方案。"),
                        ("test", IconName::CircleCheck, "制定验证计划", "让每一次修改更有把握", "分析当前项目的测试与构建配置，给出验证主要功能的具体步骤。"),
                    ].into_iter().map(|(id, icon, title, subtitle, prompt)| {
                        Button::new(id).outline().flex_1().min_w(px(180.)).h(px(if compact { 96. } else { 112. }))
                            .rounded(px(12.)).justify_start().p_3()
                            .child(div().w_full().flex().flex_col().items_start().gap_2()
                                .child(Icon::new(icon).size(px(20.)).text_color(rgb(ACCENT)))
                                .child(div().text_sm().font_weight(gpui::FontWeight::MEDIUM).child(title))
                                .child(div().text_xs().text_color(rgb(MUTED)).child(subtitle)))
                            .tooltip(prompt)
                            .disabled(!self.prompt_input.read(cx).value().trim().is_empty())
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.prompt_input.update(cx, |input, cx| {
                                    input.set_value(prompt, window, cx);
                                    input.focus(window, cx);
                                });
                                cx.notify();
                            }))
                    })))
            })
            .when(model.codex_thread_loading, |element| element.child(
                div().mt_8().flex().items_center().gap_2().text_color(rgb(MUTED))
                    .child(live_status_dot(rgb(ACCENT).into(), !self.reduced_motion))
                    .child("正在加载…")))
            .map(|element| entrance(element, "empty-state-enter", !self.reduced_motion))
    }
}
