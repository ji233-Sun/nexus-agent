use super::*;

impl NexusView {
    pub(super) fn submit(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.submit_prompt(window, cx);
    }

    pub(super) fn submit_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = self.prompt_input.read(cx).value().to_string();
        if !can_send_prompt(self.presenter.model(), &prompt) {
            return;
        }
        let executable = self.executable_input.read(cx).value().to_string();
        if self.presenter.submit(&prompt, &executable) {
            self.presenter.cancel_voice();
            self.prompt_input
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.expanded_messages.clear();
            self.timeline_scroll.scroll_to_bottom();
        }
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    pub(super) fn focus_prompt(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt_input
            .update(cx, |input, cx| input.focus(window, cx));
    }

    pub(super) fn cancel(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.presenter.cancel();
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    pub(super) fn render_message_queue(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let model = self.presenter.model();
        let colors = palette(cx);
        let queued = model
            .conversation
            .queued_messages
            .iter()
            .filter(|message| Some(message.task_id) == model.conversation.selected_task)
            .collect::<Vec<_>>();
        div().when(!queued.is_empty(), |element| {
            element
                .pb_2()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(colors.muted))
                        .child(
                            locale.format("排队消息 · {0}", &[("0", (queued.len()).to_string())]),
                        ),
                )
                .child(
                    div()
                        .id("message-queue")
                        .debug_selector(|| "message-queue".into())
                        .max_h(px(160.))
                        .overflow_y_scroll()
                        .children(queued.into_iter().map(|message| {
                            let id = message.id;
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .py_1()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(13.))
                                        .child(format!(
                                            "{} · {}",
                                            locale.permission_mode(message.permission_mode),
                                            message.prompt
                                        )),
                                )
                                .when(model.conversation.active_run.is_none(), |element| {
                                    element.child(
                                        Button::new((ElementId::from(id), "send-queued"))
                                            .ghost()
                                            .small()
                                            .label(locale.text("发送"))
                                            .on_click(cx.listener(move |app, _, _, cx| {
                                                app.presenter.send_queued_message(id);
                                                app.presenter.notify_remote_changed();
                                                cx.notify();
                                            })),
                                    )
                                })
                                .when(model.conversation.active_run.is_some(), |element| {
                                    element.child(
                                        Button::new((ElementId::from(id), "steer-queued"))
                                            .debug_selector(move || format!("steer-queued-{id}"))
                                            .ghost()
                                            .small()
                                            .label(
                                                if model.conversation.steering_message == Some(id) {
                                                    locale.text("等待工具完成…")
                                                } else {
                                                    locale.text("介入")
                                                },
                                            )
                                            .tooltip(locale.text("等待工具执行结束后介入当前对话"))
                                            .disabled(
                                                !model.can_queue()
                                                    || !message.attachments.is_empty()
                                                    || model
                                                        .conversation
                                                        .steering_message
                                                        .is_some()
                                                    || model.conversation.active_permission_mode
                                                        != Some(message.permission_mode),
                                            )
                                            .on_click(cx.listener(move |app, _, _, cx| {
                                                app.presenter.steer_queued_message(id);
                                                app.presenter.notify_remote_changed();
                                                cx.notify();
                                            })),
                                    )
                                })
                                .child(
                                    Button::new((ElementId::from(id), "remove-queued"))
                                        .ghost()
                                        .small()
                                        .icon(IconName::Close)
                                        .accessibility_label(locale.text("移除排队消息"))
                                        .disabled(model.conversation.steering_message == Some(id))
                                        .on_click(cx.listener(move |app, _, _, cx| {
                                            app.presenter.remove_queued_message(id);
                                            cx.notify();
                                        })),
                                )
                        })),
                )
        })
    }

    pub(super) fn render_working_directory(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let colors = palette(cx);
        let path = model.working_directory().map(str::to_owned);
        let app = cx.entity();
        div()
            .debug_selector(|| "composer-context".into())
            .w_full()
            .max_w(px(CONTENT_WIDTH))
            .mx_auto()
            .mb_2()
            .px_3()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                // Recreate tooltip state when switching to a different execution directory.
                div()
                    .id(SharedString::from(format!(
                        "composer-directory-{}",
                        path.as_deref().unwrap_or_default()
                    )))
                    .debug_selector(|| "composer-directory".into())
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(rgb(colors.muted))
                    .cursor_pointer()
                    .hover(|style| style.text_color(rgb(colors.text)))
                    .when_some(path, |element, path| {
                        element.tooltip(move |window, cx| {
                            let path = path.clone();
                            Tooltip::element(move |_, _| {
                                div()
                                    .debug_selector(|| "composer-directory-tooltip".into())
                                    .max_w(px(CONTENT_WIDTH))
                                    .whitespace_normal()
                                    .child(path.clone())
                            })
                            .build(window, cx)
                        })
                    })
                    .child(Icon::new(IconName::Folder).size(px(14.)).flex_none())
                    .child(
                        div()
                            .debug_selector(|| "composer-directory-name".into())
                            .min_w_0()
                            .truncate()
                            .child(working_directory_label(model).to_owned()),
                    )
                    .map(|trigger| {
                        Popover::new("project-picker")
                            .anchor(Anchor::BottomLeft)
                            .bottom_2()
                            .p_4()
                            .open(self.project_picker_open)
                            .track_focus(&self.project_search_input.focus_handle(cx))
                            .trigger(
                                Button::new("project-picker-trigger")
                                    .debug_selector(|| "project-picker-trigger".into())
                                    .ghost()
                                    .small()
                                    .h(px(COMPACT_CONTROL_HEIGHT))
                                    .min_w_0()
                                    .max_w_full()
                                    .accessibility_label(model.language.text("选择项目"))
                                    .child(trigger),
                            )
                            .on_open_change(move |open, window, cx| {
                                app.update(cx, |app, cx| {
                                    app.project_picker_open = *open;
                                    if *open {
                                        let locale = app.presenter.model().language;
                                        app.project_search_input.update(cx, |input, cx| {
                                            input.set_placeholder(
                                                locale.text("搜索项目"),
                                                window,
                                                cx,
                                            );
                                            input.set_value("", window, cx);
                                        });
                                    }
                                    cx.notify();
                                });
                            })
                            .when(self.project_picker_open, |popover| {
                                popover.child(self.render_project_picker(cx))
                            })
                            .map(|popover| div().min_w_0().child(popover))
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_none()
                    .gap_2()
                    .child(
                        self.working_directory_opener(
                            "composer-open-directory",
                            Button::new("composer-open-directory")
                                .debug_selector(|| "composer-open-directory".into())
                                .ghost()
                                .small()
                                .h(px(COMPACT_CONTROL_HEIGHT))
                                .label(model.language.text("打开方式")),
                            cx,
                        ),
                    )
                    .child(self.render_workspace_controls(cx)),
            )
    }

    pub(super) fn harness_selector(
        &self,
        id: &'static str,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let model = self.presenter.model();
        let selected = model.conversation.selected_harness;
        let app = cx.entity().clone();
        let button_id = id;
        Button::new(id)
            .icon(IconName::Bot)
            .disabled(model.conversation.active_run.is_some())
            .small()
            .when(compact, |button| {
                button
                    .ghost()
                    .h(px(COMPACT_CONTROL_HEIGHT))
                    .max_w(px(180.))
                    .label(selected.to_string())
            })
            .when(!compact, |button| {
                button
                    .debug_selector(move || id.into())
                    .outline()
                    .w_full()
                    .h(px(CONTROL_HEIGHT))
                    .accessibility_label(selected.to_string())
                    .child(settings::control_label(selected.to_string()))
            })
            .map(|button| {
                AnimatedDropdown::new(button_id, button, self.reduced_motion, move |menu, _, _| {
                    HarnessKind::ALL.into_iter().fold(
                        menu.min_w(if compact { px(160.) } else { px(220.) }),
                        |menu, harness| {
                            let app = app.clone();
                            menu.item(
                                PopupMenuItem::new(harness.to_string())
                                    .checked(harness == selected)
                                    .on_click(move |_, window, cx| {
                                        app.update(cx, |app, cx| {
                                            app.select_harness(harness, window, cx)
                                        });
                                    }),
                            )
                        },
                    )
                })
            })
    }

    pub(super) fn permission_selector(&self, cx: &mut Context<Self>) -> AnyElement {
        let model = self.presenter.model();
        let locale = model.language;
        let selected = model.conversation.permission_mode;
        let app = cx.entity().clone();
        let button_id = "composer-permissions";
        let button = Button::new(button_id)
            .debug_selector(|| "composer-permissions".into())
            .ghost()
            .small()
            .h(px(COMPACT_CONTROL_HEIGHT))
            .label(locale.permission_mode(selected))
            .tooltip(locale.text("设置下一条消息的权限，已开始的轮次保持原权限。"))
            .disabled(model.conversation.active_run.is_some() && !model.can_queue());
        AnimatedDropdown::new(button_id, button, self.reduced_motion, move |menu, _, _| {
            PermissionMode::ALL
                .into_iter()
                .fold(menu.min_w(px(160.)), |menu, mode| {
                    let app = app.clone();
                    menu.item(
                        PopupMenuItem::new(locale.permission_mode(mode))
                            .checked(mode == selected)
                            .on_click(move |_, _, cx| {
                                app.update(cx, |app, cx| {
                                    app.presenter.select_permission_mode(mode);
                                    cx.notify();
                                });
                            }),
                    )
                })
        })
        .into_any_element()
    }

    pub(super) fn effort_selector(&self, cx: &mut Context<Self>) -> AnyElement {
        let model = self.presenter.model();
        let locale = model.language;
        let selected = model.conversation.effort;
        let mut efforts = vec![ThinkingEffort::Default];
        if let Some(descriptor) = model.selected_catalog_model() {
            efforts.extend(
                descriptor
                    .supported_reasoning_efforts
                    .iter()
                    .map(|option| option.effort),
            );
        }
        let resolved = model.resolved_model_selection().effort;
        let label = if selected.is_default() && !resolved.is_default() {
            format!("{} · {}", locale.effort(selected), locale.effort(resolved))
        } else {
            locale.effort(resolved).to_owned()
        };
        let supported = efforts.len() > 1;
        let app = cx.entity().clone();
        let button_id = "composer-effort";
        let button = Button::new(button_id)
            .ghost()
            .small()
            .h(px(COMPACT_CONTROL_HEIGHT))
            .icon(IconName::Cpu)
            .label(label)
            .tooltip(if supported {
                locale.text("思考档位")
            } else {
                locale.text("当前模型未确认支持独立思考设置，将使用默认行为。")
            })
            .disabled(model.conversation.active_run.is_some() || !supported);
        if model.conversation.active_run.is_some() || !supported {
            return button.into_any_element();
        }
        AnimatedDropdown::new(button_id, button, self.reduced_motion, move |menu, _, _| {
            efforts
                .iter()
                .copied()
                .fold(menu.min_w(px(140.)), |menu, effort| {
                    let app = app.clone();
                    menu.item(
                        PopupMenuItem::new(locale.effort(effort))
                            .checked(effort == selected)
                            .on_click(move |_, _, cx| {
                                app.update(cx, |app, cx| app.select_effort(effort, cx));
                            }),
                    )
                })
        })
        .into_any_element()
    }
}
