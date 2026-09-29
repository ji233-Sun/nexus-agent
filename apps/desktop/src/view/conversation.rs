use super::*;

impl NexusView {
    pub(super) fn render_workspace(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let page = if let Some(provider) = self.presenter.model().opened_issues() {
            Some(self.render_issues(provider, cx).into_any_element())
        } else {
            self.review_pages
                .get(&self.presenter.model().conversation.id)
                .map(|page| self.render_workspace_review(page, cx))
        };
        if let Some(page) = page {
            return div()
                .size_full()
                .bg(materials(cx).chrome)
                .flex()
                .child(
                    self.sidebar_pane.clone().cached(
                        gpui::StyleRefinement::default()
                            .w(px(SIDEBAR_WIDTH))
                            .h_full()
                            .flex_none(),
                    ),
                )
                .child(page)
                .into_any_element();
        }
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let material = materials(cx);
        let model = self.presenter.model();
        let voice_status = model.voice.status.render(locale);
        let can_submit = can_send_prompt(model, &self.prompt_input.read(cx).value());
        let prompt_focused = self
            .prompt_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let composer_hint = if model.conversation.active_run.is_some() {
            locale.text("Agent 正在执行 · 发送后排队，每轮结束后发送一条")
        } else if !model.can_submit() {
            locale.text("Agent 尚未就绪 · 打开设置检查探测和登录状态")
        } else if cfg!(target_os = "macos") {
            locale.text("⌘ Enter 发送消息 · Enter 换行")
        } else {
            locale.text("Ctrl Enter 发送消息 · Enter 换行")
        };
        let selected_task = model.conversation.selected_task.and_then(|task_id| {
            model
                .conversation
                .tasks
                .iter()
                .find(|task| task.id == task_id)
        });
        let header_project = model
            .conversation
            .selected_project
            .as_ref()
            .map(|project| project.display_name.clone())
            .unwrap_or_else(|| locale.text("未关联项目").into());
        let header_project_tooltip = model.conversation.selected_project.as_ref().map_or_else(
            || header_project.clone(),
            |project| format!("{}\n{}", project.display_name, project.canonical_path),
        );
        let header_task = selected_task
            .map(|task| task.title.clone())
            .unwrap_or_else(|| locale.text("新建任务").into());
        div()
            .debug_selector(|| "workspace-page".into())
            .on_drop(cx.listener(|app, paths: &gpui::ExternalPaths, _, cx| {
                for path in paths.paths().iter().filter(|path| {
                    path.extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
                }) {
                    app.open_pdf(path.clone(), cx);
                }
            }))
            .size_full()
            .bg(material.chrome)
            .flex()
            .child(
                self.sidebar_pane.clone().cached(
                    gpui::StyleRefinement::default()
                        .w(px(SIDEBAR_WIDTH))
                        .h_full()
                        .flex_none(),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .my_2()
                    .mr_2()
                    .rounded(px(16.))
                    .border_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.canvas))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .debug_selector(|| "workspace-header".into())
                            .h(px(HEADER_HEIGHT))
                            .flex_none()
                            .border_b(px(0.5))
                            .border_color(material.edge)
                            .px_5()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Icon::new(IconName::Folder).text_color(rgb(colors.muted)),
                                    )
                                    .child(
                                        div()
                                            .id("workspace-header-project")
                                            .debug_selector(|| "workspace-header-project".into())
                                            .max_w(px(200.))
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(13.))
                                            .text_color(rgb(colors.text_secondary))
                                            .tooltip(move |window, cx| {
                                                Tooltip::new(header_project_tooltip.clone())
                                                    .build(window, cx)
                                            })
                                            .child(header_project),
                                    )
                                    .map(|element| {
                                        let tooltip = header_task.clone();
                                        element
                                            .child(
                                                Icon::new(IconName::ChevronRight)
                                                    .size(px(14.))
                                                    .text_color(rgb(colors.muted)),
                                            )
                                            .child(
                                                div()
                                                    .id("workspace-header-task")
                                                    .debug_selector(|| {
                                                        "workspace-header-task".into()
                                                    })
                                                    .flex_1()
                                                    .min_w_0()
                                                    .truncate()
                                                    .font_weight(gpui::FontWeight::MEDIUM)
                                                    .tooltip(move |window, cx| {
                                                        Tooltip::new(tooltip.clone())
                                                            .build(window, cx)
                                                    })
                                                    .child(header_task),
                                            )
                                    }),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .pl_3()
                                    .when(model.conversation.selected_project.is_some(), |element| {
                                        element.child(
                                            Button::new("toggle-changes-sidebar")
                                                .debug_selector(|| "toggle-changes-sidebar".into())
                                                .ghost()
                                                .small()
                                                .icon(IconName::PanelRight)
                                                .tooltip(locale.text("环境"))
                                                .accessibility_label(locale.text("环境"))
                                                .selected(model.conversation.changes_sidebar_open)
                                                .on_click(cx.listener(|app, _, _, cx| {
                                                    app.presenter.toggle_changes_sidebar();
                                                    cx.notify();
                                                })),
                                        )
                                    })
                                    .child(
                                        Button::new("open-settings")
                                            .debug_selector(|| "open-settings".into())
                                            .ghost()
                                            .small()
                                            .size(px(COMPACT_CONTROL_HEIGHT))
                                            .p_0()
                                            .icon(IconName::Settings2)
                                            .accessibility_label(locale.text("设置"))
                                            .tooltip(if cfg!(target_os = "macos") {
                                                locale.text("打开设置 · ⌘ ,")
                                            } else {
                                                locale.text("打开设置 · Ctrl ,")
                                            })
                                            .on_click(cx.listener(|app, _, window, cx| {
                                                app.toggle_settings(window, cx)
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        self.timeline_pane
                            .clone()
                            .cached(gpui::StyleRefinement::default().flex_1().min_h_0()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px(px(24.))
                            .pt_3()
                            .pb_4()
                            .child(self.render_user_asks(window, cx))
                            .child(self.render_working_directory(cx))
                            .child(self.render_workspace_hints(cx))
                            .child(
                                div()
                                    .debug_selector(|| "composer-surface".into())
                                    .capture_action(cx.listener(Self::paste_attachments))
                                    .on_drop(cx.listener(|app, paths: &gpui::ExternalPaths, _, cx| {
                                        cx.stop_propagation();
                                        app.import_attachments(paths.paths().to_vec(), Vec::new(), cx);
                                    }))
                                    .relative()
                                    .w_full()
                                    .max_w(px(CONTENT_WIDTH))
                                    .mx_auto()
                                    .rounded(px(16.))
                                    .bg(material.floating)
                                    .border_1()
                                    .border_color(rgb(colors.input_border).opacity(0.45))
                                    .when(prompt_focused, |element| {
                                        element.border_color(rgb(colors.accent))
                                    })
                                    .shadow(material.shadow())
                                    .p_4()
                                    .flex()
                                    .flex_col()
                                    .child(self.render_message_queue(cx))
                                    .child(self.render_attachments(
                                        &model.conversation.attachments,
                                        None,
                                        cx,
                                    ))
                                    .when(model.conversation.attachments_loading, |element| {
                                        element.child(div().text_size(px(12.)).child(locale.text("正在添加附件，请稍候。")))
                                    })
                                    .when_some(model.conversation.attachment_error.as_ref(), |element, error| {
                                        element.child(
                                            div()
                                                .text_size(px(12.))
                                                .child(error.render(locale).to_owned()),
                                        )
                                    })
                                    .when(!voice_status.is_empty(), |element| {
                                        element.child(
                                            div()
                                                .mb_2()
                                                .text_size(px(12.))
                                                .text_color(rgb(colors.text_secondary))
                                                .child(voice_status.to_owned()),
                                        )
                                    })
                                    .child(
                                        Textarea::new(&self.prompt_input)
                                            .appearance(false)
                                            .bordered(false)
                                            .text_size(px(15.))
                                            .line_height(relative(1.65))
                                            .aria_label(locale.text("任务描述")),
                                    )
                                    .child(
                                        div()
                                            .min_h(px(COMPACT_CONTROL_HEIGHT))
                                            .mt_3()
                                            .pt_3()
                                            .border_t(px(0.5))
                                            .border_color(rgb(colors.border))
                                            .flex()
                                            .flex_wrap()
                                            .gap_2()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_wrap()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(self.model_selector(window, cx))
                                                    .child(self.effort_selector(cx))
                                                    .child(self.permission_selector(cx)),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(self.render_voice_controls(cx))
                                                    .child(
                                                        Button::new("attach-files")
                                                            .ghost()
                                                            .small()
                                                            .label(locale.text("附件"))
                                                            .debug_selector(|| "attach-files".into())
                                                            .tooltip(locale.text("添加图片或文件，也可拖入或粘贴到输入框"))
                                                            .disabled(model.conversation.attachments_loading)
                                                            .on_click(cx.listener(|app, _, _, cx| app.choose_attachments(cx))),
                                                    )
                                                    .child(
                                                        Button::new("open-pdf")
                                                            .ghost()
                                                            .small()
                                                            .label("PDF")
                                                            .debug_selector(|| "open-pdf".into())
                                                            .tooltip(locale.text("打开 PDF"))
                                                            .on_click(cx.listener(
                                                                |app, _, _, cx| app.choose_pdf(cx),
                                                            )),
                                                    )
                                                    .when(model.conversation.active_run.is_some(), |element| {
                                                        element.child(
                                                            Button::new("composer-cancel")
                                                                .debug_selector(|| {
                                                                    "composer-cancel".into()
                                                                })
                                                                .danger()
                                                                .outline()
                                                                .small()
                                                                .h(px(COMPACT_CONTROL_HEIGHT))
                                                                .icon(IconName::Pause)
                                                                .label(locale.text("停止"))
                                                                .disabled(model.conversation.run_cancelling)
                                                                .tooltip(locale.text(
                                                                    "停止当前运行，保留已有输出",
                                                                ))
                                                                .on_click(
                                                                    cx.listener(Self::cancel),
                                                                ),
                                                        )
                                                    })
                                                    .child(
                                                        Button::new("submit")
                                                            .debug_selector(|| {
                                                                "composer-submit".into()
                                                            })
                                                            .primary()
                                                            .small()
                                                            .size(px(CONTROL_HEIGHT))
                                                            .rounded(px(10.))
                                                            .p_0()
                                                            .icon(IconName::ArrowUp)
                                                            .accessibility_label(
                                                                if model.conversation.active_run.is_some() {
                                                                    locale.text("加入消息队列")
                                                                } else {
                                                                    locale.text("发送任务")
                                                                },
                                                            )
                                                            .tooltip(composer_hint)
                                                            .when(!can_submit, |button| {
                                                                button.opacity(0.42)
                                                            })
                                                            .disabled(!can_submit)
                                                            .on_click(cx.listener(Self::submit)),
                                                    ),
                                            ),
                                    )
                                    .map(|element| {
                                        entrance(element, "composer-enter", !self.reduced_motion)
                                    }),
                            )
                            .child(
                                div()
                                    .max_w(px(CONTENT_WIDTH))
                                    .mx_auto()
                                    .mt_2()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors.muted))
                                            .child(composer_hint),
                                    )
                                    .when(
                                        model.conversation.selected_project.is_some()
                                            && !model.can_submit()
                                            && model.conversation.active_run.is_none(),
                                        |element| {
                                            element.child(
                                                Button::new("setup-agent")
                                                    .ghost()
                                                    .small()
                                                    .h(px(COMPACT_CONTROL_HEIGHT))
                                                    .label(locale.text("检查环境"))
                                                    .on_click(cx.listener(|app, _, window, cx| {
                                                        app.toggle_settings(window, cx);
                                                    })),
                                            )
                                        },
                                    ),
                            ),
                    ),
            )
            .when(model.conversation.changes_sidebar_open, |element| {
                element.child(self.render_changes_sidebar(cx))
            })
            .into_any_element()
    }
}
