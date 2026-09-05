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
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(div()
            .id("timeline")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.timeline_scroll)
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
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
                                "想在这个项目里完成什么？",
                                "从一个具体目标开始。执行过程与结果都会留在这里。",
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
                                .child(div().size(px(48.)).mb_4().rounded(px(16.))
                                    .bg(rgba(0x9b7cff18)).border_1().border_color(rgba(0x9b7cff33))
                                    .flex().items_center().justify_center().text_xl().text_color(rgb(ACCENT)).child("✳"))
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
                                .when(!showing_codex_history && model.selected_project.is_none(), |element| element.child(
                                    Button::new("welcome-open-project").primary().mt_4().label("＋ 选择项目目录")
                                        .on_click(cx.listener(Self::choose_project)),
                                ))
                                .when(!showing_codex_history && model.selected_project.is_some() && model.active_run.is_none(), |element| element.child(
                                    div().mt_6().w_full().flex().flex_col().gap_2()
                                        .children([
                                            ("explore", "⌕  理解项目", "梳理项目结构，介绍主要模块及它们之间的关系。"),
                                            ("review", "◇  检查代码", "检查当前项目的代码，找出最值得优先修复的问题，先说明原因和修复方案。"),
                                            ("test", "✓  制定验证计划", "分析当前项目的测试与构建配置，给出验证主要功能的具体步骤。"),
                                        ].into_iter().map(|(id, title, prompt)| {
                                            Button::new(id).outline().w_full().h(px(48.)).justify_start().label(title)
                                                .tooltip(prompt)
                                                .disabled(!self.prompt_input.read(cx).value().trim().is_empty())
                                                .on_click(cx.listener(move |app, _, window, cx| {
                                                    app.prompt_input.update(cx, |input, cx| {
                                                        input.set_value(prompt, window, cx);
                                                        input.focus(window, cx);
                                                    });
                                                    cx.notify();
                                                }))
                                        })),
                                ))
                                .when(model.codex_thread_loading, |element| element.child(
                                    div().mt_6().w_full().flex().flex_col().gap_3()
                                        .children((0usize..3).map(|index| {
                                            let bar = div().h(px(12.)).w(px(220. + index as f32 * 55.)).rounded_full().bg(rgba(0xffffff12));
                                            if self.reduced_motion { bar.into_any_element() } else {
                                                bar.with_animation(("history-loading", index), Animation::new(Duration::from_millis(1400)).repeat().with_easing(pulsating_between(0.35, 0.8)), |bar, opacity| bar.opacity(opacity)).into_any_element()
                                            }
                                        })),
                                ))
                                .map(|element| entrance(element, "empty-state-enter", !self.reduced_motion)),
                        )
                    })
                    .when(!showing_codex_history, |element| {
                        element.children(
                            model
                                .messages
                                .iter()
                                .map(|message| self.render_message(message, window, cx)),
                        )
                    })
                    .when(showing_codex_history, |element| {
                        element.children(model.codex_history_messages.iter().enumerate().map(
                            |(index, message)| self.render_history_message(index, message, window, cx),
                        ))
                    })
                    .when(
                        !showing_codex_history && !model.streaming_text.is_empty(),
                        |element| {
                            element.child(self.message_card(
                                "streaming-message",
                                MessageRole::Assistant,
                                &model.streaming_text,
                                MessageKind::Text,
                                window,
                                cx,
                            ))
                        },
                    )
                    .when(!showing_codex_history && model.active_run.is_some() && model.selected_task == model.active_task, |element| element.child(
                        div().flex().items_center().gap_3().text_xs().text_color(rgb(TEXT_SECONDARY))
                            .child(live_status_dot(rgb(ACCENT).into(), !self.reduced_motion))
                            .child(model.status.clone()),
                    )),
            ))
            .when(self.timeline_scroll.max_offset().height + self.timeline_scroll.offset().y > px(48.), |element| element.child(
                div().absolute().bottom(px(12.)).left_0().right_0().flex().justify_center()
                    .child(Button::new("latest-message").outline().small().rounded_full().label("↓ 回到最新消息")
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.timeline_scroll.scroll_to_bottom();
                            cx.notify();
                        }))),
            ))
    }
}
