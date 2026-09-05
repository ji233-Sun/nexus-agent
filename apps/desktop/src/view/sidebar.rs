use super::*;

impl NexusView {
    pub(super) fn render_task_tree(&self, cx: &mut Context<Self>) -> gpui::Div {
        let model = self.presenter.model();
        div()
            .ml(px(17.))
            .pl_2()
            .border_l_1()
            .border_color(rgba(0xffffff0d))
            .flex()
            .flex_col()
            .gap_1()
            .when(model.tasks.is_empty(), |element| {
                element.child(
                    div()
                        .px_2()
                        .py_2()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("发送 Prompt 后，任务会显示在这里"),
                )
            })
            .children(model.tasks.iter().map(|task| {
                let task_id = task.id;
                let selected = model.selected_task == Some(task_id);
                div()
                    .id(SharedString::from(format!("task-{task_id}")))
                    .relative()
                    .h(px(34.))
                    .px_2()
                    .rounded(px(9.))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_color(rgb(TEXT_SECONDARY))
                    .when(selected, |element| {
                        element
                            .bg(rgba(0xffffff14))
                            .text_color(rgb(TEXT))
                            .shadow(selection_shadow())
                            .child(selection_indicator(SharedString::from(format!(
                                "task-selection-{task_id}"
                            ))))
                    })
                    .hover(|style| style.bg(rgba(0xffffff0d)).text_color(rgb(TEXT)))
                    .on_click(
                        cx.listener(move |app, _, window, cx| app.select_task(task_id, window, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(task.title.clone()),
                    )
                    .child(status_dot(run_status_color(task.status)))
            }))
    }

    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let selected_project_id = model.selected_project.as_ref().map(|project| project.id);
        let selected_codex_thread = model.selected_codex_thread.as_deref();
        let history_status = if model.codex_history_loading {
            "正在读取…".to_owned()
        } else if let Some(error) = &model.codex_history_error {
            format!("不可用：{error}")
        } else if self.presenter.history_available() {
            format!("{} 条本机会话", model.codex_threads.len())
        } else {
            "等待检测 Codex CLI".to_owned()
        };
        div()
            .w(px(268.))
            .h_full()
            .flex_none()
            .bg(rgba(0x1c1e1dde))
            .border_r_1()
            .border_color(rgba(0xffffff12))
            .pt(px(if cfg!(target_os = "macos") { 40. } else { 8. }))
            .px_3()
            .pb_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div().h(px(44.)).px_2().flex().items_center().child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .size(px(27.))
                                .rounded(px(9.))
                                .bg(rgba(0x9b7cff2b))
                                .border_1()
                                .border_color(rgba(0xb8a6ff38))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(rgb(ACCENT))
                                .child("N"),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(1.))
                                .child(
                                    div()
                                        .text_base()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child("Nexus Agent"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .child("Local workspace"),
                                ),
                        ),
                ),
            )
            .child(
                Button::new("new-task")
                    .ghost()
                    .w_full()
                    .justify_start()
                    .rounded(px(10.))
                    .child(button_label("＋   新任务", TEXT))
                    .disabled(model.selected_project.is_none() || model.active_run.is_some())
                    .on_click(cx.listener(Self::new_task)),
            )
            .child(
                div()
                    .id("project-tree")
                    .max_h(px(420.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .mt_2()
                            .px_2()
                            .py_1()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(section_label("PROJECTS"))
                            .child(
                                Button::new("open-project")
                                    .ghost()
                                    .small()
                                    .rounded(px(9.))
                                    .child(button_label("＋", TEXT_SECONDARY))
                                    .on_click(cx.listener(Self::choose_project)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .when(model.projects.is_empty(), |element| {
                                element.child(
                                    div()
                                        .rounded(px(12.))
                                        .border_1()
                                        .border_color(rgba(0xffffff0d))
                                        .bg(rgba(0xffffff06))
                                        .px_3()
                                        .py_4()
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .child("还没有项目，点击右上角 ＋ 添加。"),
                                )
                            })
                            .children(model.projects.iter().map(|project| {
                                let id = project.id;
                                let selected = selected_project_id == Some(id);
                                let display_name = project.display_name.clone();
                                let project = project.clone();
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .id(SharedString::from(format!("project-{id}")))
                                            .relative()
                                            .h(px(36.))
                                            .px_2()
                                            .rounded(px(10.))
                                            .cursor_pointer()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .text_sm()
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .when(selected, |element| {
                                                element
                                                    .bg(rgba(0xffffff12))
                                                    .text_color(rgb(TEXT))
                                                    .shadow(selection_shadow())
                                                    .child(selection_indicator(SharedString::from(
                                                        format!("project-selection-{id}"),
                                                    )))
                                            })
                                            .hover(|style| {
                                                style.bg(rgba(0xffffff0d)).text_color(rgb(TEXT))
                                            })
                                            .on_click(cx.listener(move |app, _, _, cx| {
                                                app.select_project(project.clone());
                                                cx.notify();
                                            }))
                                            .child(
                                                div()
                                                    .w(px(18.))
                                                    .text_color(if selected {
                                                        rgb(ACCENT)
                                                    } else {
                                                        rgb(MUTED)
                                                    })
                                                    .child("▱"),
                                            )
                                            .child(div().flex_1().truncate().child(display_name))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(MUTED))
                                                    .child(if selected { "⌄" } else { "›" }),
                                            ),
                                    )
                                    .when(selected, |element| {
                                        element.child(self.render_task_tree(cx))
                                    })
                            })),
                    ),
            )
            .child(
                div().flex_1().min_h_0().flex().flex_col().gap_1().child(
                    div()
                        .id("history-list")
                        .flex_1()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(section_label("CODEX RECENTS"))
                                .child(
                                    Button::new("refresh-codex-history")
                                        .ghost()
                                        .small()
                                        .rounded(px(9.))
                                        .child(button_label("↻", TEXT_SECONDARY))
                                        .disabled(
                                            !self.presenter.history_available()
                                                || model.codex_history_loading,
                                        )
                                        .on_click(cx.listener(Self::refresh_codex_history)),
                                ),
                        )
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .line_clamp(3)
                                .child(history_status),
                        )
                        .children(model.codex_threads.iter().map(|thread| {
                            let thread_id = thread.id.clone();
                            let selected = selected_codex_thread == Some(thread.id.as_str());
                            div()
                                .id(SharedString::from(format!("codex-thread-{}", thread.id)))
                                .relative()
                                .p_2()
                                .rounded(px(9.))
                                .cursor_pointer()
                                .text_color(rgb(TEXT_SECONDARY))
                                .when(selected, |element| {
                                    element
                                        .bg(rgba(0xffffff14))
                                        .text_color(rgb(TEXT))
                                        .shadow(selection_shadow())
                                        .child(selection_indicator(SharedString::from(format!(
                                            "codex-selection-{}",
                                            thread.id
                                        ))))
                                })
                                .hover(|style| style.bg(rgba(0xffffff0d)).text_color(rgb(TEXT)))
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.select_codex_thread(thread_id.clone(), cx)
                                }))
                                .child(div().text_sm().truncate().child(thread.title.clone()))
                                .child(
                                    div()
                                        .mt(px(3.))
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .truncate()
                                        .child(thread.detail()),
                                )
                        })),
                ),
            )
            .child(
                div()
                    .h(px(36.))
                    .px_2()
                    .rounded(px(10.))
                    .bg(rgba(0xffffff07))
                    .border_1()
                    .border_color(rgba(0xffffff0a))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(status_dot(rgb(SUCCESS).into()))
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(rgb(TEXT_SECONDARY))
                            .child("本地运行环境"),
                    )
                    .child(div().text_xs().text_color(rgb(MUTED)).child(
                        if cfg!(target_os = "macos") {
                            "⌘"
                        } else {
                            "Ctrl"
                        },
                    )),
            )
    }
}
