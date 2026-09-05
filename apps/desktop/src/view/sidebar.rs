use super::*;

impl NexusView {
    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let query = self.search_input.read(cx).value();
        let selected_project_id = model.selected_project.as_ref().map(|project| project.id);
        let history_status = if model.codex_history_loading {
            "正在读取本机会话…".to_owned()
        } else if let Some(error) = &model.codex_history_error {
            format!("历史不可用：{error}")
        } else if self.presenter.history_available() {
            format!("{} 条本机会话 · 只读浏览", model.codex_threads.len())
        } else {
            "等待检测 Codex CLI".to_owned()
        };
        let projects = SidebarMenu::new()
            .gap_1()
            .children(model.projects.iter().map(|project| {
                let selected = selected_project_id == Some(project.id);
                let project = project.clone();
                SidebarMenuItem::new(project.display_name.clone())
                    .icon(IconName::Folder)
                    .active(selected && model.selected_codex_thread.is_none())
                    .default_open(true)
                    .click_to_open(true)
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.select_project(project.clone());
                        cx.notify();
                    }))
                    .when(selected, |item| {
                        let tasks: Vec<_> = model
                            .tasks
                            .iter()
                            .filter(|task| matches_search(&task.title, &query))
                            .map(|task| {
                                let id = task.id;
                                let color = run_status_color(task.status);
                                SidebarMenuItem::new(task.title.clone())
                                    .active(
                                        model.selected_task == Some(id)
                                            && model.selected_codex_thread.is_none(),
                                    )
                                    .suffix(move |_, _| status_dot(color))
                                    .on_click(cx.listener(move |app, _, window, cx| {
                                        app.select_task(id, window, cx);
                                    }))
                            })
                            .collect();
                        item.children(if tasks.is_empty() {
                            vec![
                                SidebarMenuItem::new(if query.trim().is_empty() {
                                    "开始任务后，记录会出现在这里"
                                } else {
                                    "没有匹配的任务"
                                })
                                .disable(true),
                            ]
                        } else {
                            tasks
                        })
                    })
            }));
        let mut history: Vec<_> = model
            .codex_threads
            .iter()
            .filter(|thread| {
                matches_search(&format!("{} {}", thread.title, thread.detail()), &query)
            })
            .map(|thread| {
                let id = thread.id.clone();
                SidebarMenuItem::new(thread.title.clone())
                    .icon(IconName::FileText)
                    .active(model.selected_codex_thread.as_deref() == Some(thread.id.as_str()))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.select_codex_thread(id.clone(), cx);
                    }))
            })
            .collect();
        if history.is_empty() {
            history.push(
                SidebarMenuItem::new(if query.trim().is_empty() {
                    "暂无可显示的会话"
                } else {
                    "没有匹配的历史会话"
                })
                .disable(true),
            );
        }

        div()
            .h_full()
            .flex_none()
            .pt(px(if cfg!(target_os = "macos") { 36. } else { 0. }))
            .bg(rgb(SURFACE))
            .child(
                Sidebar::new("workspace-sidebar")
                    .w(px(260.))
                    .collapsible(false)
                    .header(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .pb_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .px_2()
                                    .py_2()
                                    .child(brand_mark(32.))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
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
                                                    .child("你的本地开发工作空间"),
                                            ),
                                    ),
                            )
                            .child(
                                Button::new("new-task")
                                    .primary()
                                    .small()
                                    .w_full()
                                    .h(px(CONTROL_HEIGHT))
                                    .accessibility_label("新建任务")
                                    .child(
                                        div()
                                            .w_full()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(Icon::new(IconName::Plus))
                                                    .child("新建任务"),
                                            )
                                            .child(div().text_xs().opacity(0.65).child(
                                                if cfg!(target_os = "macos") {
                                                    "⌘ N"
                                                } else {
                                                    "Ctrl N"
                                                },
                                            )),
                                    )
                                    .tooltip("在当前项目中开始新任务")
                                    .disabled(
                                        model.selected_project.is_none()
                                            || model.active_run.is_some(),
                                    )
                                    .on_click(
                                        cx.listener(|app, _, window, cx| app.new_task(window, cx)),
                                    ),
                            )
                            .child(
                                Input::new(&self.search_input)
                                    .small()
                                    .min_h(px(CONTROL_HEIGHT))
                                    .text_sm()
                                    .prefix(Icon::new(IconName::Search).small())
                                    .cleanable(true),
                            )
                            .child(div().px_2().pt_3().child(section_label("项目空间"))),
                    )
                    .child(
                        projects.child(
                            SidebarMenuItem::new("添加本地项目")
                                .icon(IconName::Plus)
                                .on_click(cx.listener(Self::choose_project)),
                        ),
                    )
                    .child(
                        SidebarMenu::new().child(
                            SidebarMenuItem::new("Codex 最近会话")
                                .icon(IconName::FileText)
                                .default_open(false)
                                .click_to_toggle(true)
                                .children(history),
                        ),
                    )
                    .footer(
                        div()
                            .w_full()
                            .pt_3()
                            .border_t_1()
                            .border_color(rgb(BORDER))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_xs()
                                            .text_color(rgb(MUTED))
                                            .line_clamp(2)
                                            .child(history_status),
                                    )
                                    .child(
                                        Button::new("refresh-codex-history")
                                            .ghost()
                                            .small()
                                            .size(px(COMPACT_CONTROL_HEIGHT))
                                            .icon(IconName::RotateCw)
                                            .tooltip("刷新本机 Codex 历史")
                                            .disabled(
                                                !self.presenter.history_available()
                                                    || model.codex_history_loading,
                                            )
                                            .on_click(cx.listener(Self::refresh_codex_history)),
                                    ),
                            )
                            .child(
                                Button::new("sidebar-environment")
                                    .ghost()
                                    .small()
                                    .w_full()
                                    .h(px(CONTROL_HEIGHT))
                                    .accessibility_label("环境与偏好")
                                    .child(
                                        div()
                                            .w_full()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(Icon::new(IconName::Settings2))
                                                    .child("环境与偏好"),
                                            )
                                            .child(status_dot(
                                                model
                                                    .selected_probe()
                                                    .map(|probe| {
                                                        if probe.available && probe.authenticated {
                                                            rgb(SUCCESS).into()
                                                        } else {
                                                            rgb(WARNING).into()
                                                        }
                                                    })
                                                    .unwrap_or_else(|| rgb(MUTED).into()),
                                            )),
                                    )
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.toggle_settings(window, cx)
                                    })),
                            ),
                    ),
            )
    }
}
