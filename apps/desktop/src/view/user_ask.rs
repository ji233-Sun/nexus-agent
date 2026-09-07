use super::*;

impl NexusView {
    pub(super) fn render_user_asks(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let model = self.presenter.model();
        if model.active_task != model.selected_task || model.pending_user_asks.is_empty() {
            return div().into_any_element();
        }
        let requests = model.pending_user_asks.clone();
        let total = requests.len();
        let mut stack = div()
            .id("user-ask-stack-scroll")
            .debug_selector(|| "user-ask-stack".into())
            .w_full()
            .max_w(px(CONTENT_WIDTH))
            .mx_auto()
            .mb_3()
            .max_h(window.viewport_size().height * 0.36)
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2()
            .pr_1();
        for (index, request) in requests.iter().enumerate() {
            stack = stack.child(self.render_user_ask_request(request, index, total, window, cx));
        }
        stack.into_any_element()
    }

    fn render_user_ask_request(
        &self,
        request: &PendingUserAsk,
        request_index: usize,
        request_count: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let material = materials(cx);
        let request_id = request.request_id;
        let title = if request_count > 1 {
            locale.format(
                "Agent 提问 · {current}/{total}",
                &[
                    ("current", (request_index + 1).to_string()),
                    ("total", request_count.to_string()),
                ],
            )
        } else {
            locale.text("Agent 提问").to_owned()
        };
        let (status, status_color) = match request.submission {
            UserAskSubmissionState::Pending if request.error.is_some() => {
                (locale.text("提交失败"), rgb(colors.danger))
            }
            UserAskSubmissionState::Pending => (locale.text("等待回答"), rgb(colors.warning)),
            UserAskSubmissionState::Submitting => (locale.text("正在提交…"), rgb(colors.accent)),
            UserAskSubmissionState::Sent => (locale.text("已发送"), rgb(colors.success)),
        };
        let collapse_selector = format!("user-ask-collapse-{request_id}");
        let mut panel = div()
            .debug_selector({
                let selector = format!("user-ask-panel-{request_id}");
                move || selector.clone()
            })
            .w_full()
            .rounded(px(CONTROL_RADIUS))
            .border_1()
            .border_color(material.edge)
            .bg(material.floating)
            .shadow(material.shadow())
            .overflow_hidden()
            .child(
                div()
                    .min_h(px(40.))
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::Bot)
                            .size(px(16.))
                            .text_color(rgb(colors.accent)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(status_color)
                            .child(status),
                    )
                    .child(
                        Button::new(SharedString::from(collapse_selector.clone()))
                            .debug_selector(move || collapse_selector.clone())
                            .ghost()
                            .small()
                            .size(px(COMPACT_CONTROL_HEIGHT))
                            .p_0()
                            .icon(if request.collapsed {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronUp
                            })
                            .accessibility_label(if request.collapsed {
                                locale.text("展开提问")
                            } else {
                                locale.text("收起提问")
                            })
                            .tooltip(if request.collapsed {
                                locale.text("展开提问")
                            } else {
                                locale.text("收起提问")
                            })
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.toggle_user_ask(request_id, cx);
                            })),
                    ),
            );
        if request.collapsed {
            return panel.into_any_element();
        }

        let Some(question) = request.questions.get(
            request
                .active_question
                .min(request.questions.len().saturating_sub(1)),
        ) else {
            return panel
                .child(
                    div()
                        .px_3()
                        .pb_3()
                        .text_size(px(13.))
                        .text_color(rgb(colors.danger))
                        .child(locale.text("此提问没有可回答的问题。")),
                )
                .into_any_element();
        };
        let question_index = request.active_question.min(request.questions.len() - 1);
        let editable = request.submission == UserAskSubmissionState::Pending;
        let mode_label = match question.answer_mode {
            UserAskAnswerMode::Text => locale.text("文本回答"),
            UserAskAnswerMode::Choice { multiple: true, .. } => locale.text("多选"),
            UserAskAnswerMode::Choice {
                multiple: false, ..
            } => locale.text("单选"),
        };
        let progress = locale.format(
            "问题 {current}/{total}",
            &[
                ("current", (question_index + 1).to_string()),
                ("total", request.questions.len().to_string()),
            ],
        );
        let mut answers = div().mt_3().flex().flex_col().gap_2();
        if let UserAskAnswerMode::Choice {
            multiple,
            allow_custom,
        } = question.answer_mode
        {
            let selected = match request.drafts.get(&question.id) {
                Some(UserAskAnswerValue::Selected(selected)) => selected.as_slice(),
                _ => &[],
            };
            for option in &question.options {
                let checked = selected.iter().any(|id| id == &option.id);
                let question_id = question.id.clone();
                let option_id = option.id.clone();
                let control_id = SharedString::from(format!(
                    "user-ask-option-{request_id}-{question_id}-{option_id}"
                ));
                let selector = control_id.clone();
                let label = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .whitespace_normal()
                            .overflow_hidden()
                            .child(option.label.clone()),
                    )
                    .when_some(
                        option
                            .description
                            .as_deref()
                            .filter(|description| !description.is_empty()),
                        |element, description| {
                            element.child(
                                div()
                                    .whitespace_normal()
                                    .overflow_hidden()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors.muted))
                                    .child(description.to_owned()),
                            )
                        },
                    );
                let app = cx.entity().clone();
                let control = if multiple {
                    let question_id_for_click = question_id.clone();
                    let option_id_for_click = option_id.clone();
                    Checkbox::new(control_id)
                        .debug_selector(move || selector.to_string())
                        .w_full()
                        .p_2()
                        .rounded(px(6.))
                        .bg(rgb(colors.recessed))
                        .checked(checked)
                        .disabled(!editable)
                        .accessibility_label(option.label.clone())
                        .on_click(move |checked, window, cx| {
                            app.update(cx, |app, cx| {
                                app.select_user_ask_option(
                                    request_id,
                                    &question_id_for_click,
                                    &option_id_for_click,
                                    *checked,
                                    window,
                                    cx,
                                );
                            });
                        })
                        .child(label)
                        .into_any_element()
                } else {
                    let question_id_for_click = question_id.clone();
                    let option_id_for_click = option_id.clone();
                    Radio::new(control_id)
                        .debug_selector(move || selector.to_string())
                        .w_full()
                        .p_2()
                        .rounded(px(6.))
                        .bg(rgb(colors.recessed))
                        .checked(checked)
                        .disabled(!editable)
                        .accessibility_label(option.label.clone())
                        .on_click(move |checked, window, cx| {
                            app.update(cx, |app, cx| {
                                app.select_user_ask_option(
                                    request_id,
                                    &question_id_for_click,
                                    &option_id_for_click,
                                    *checked,
                                    window,
                                    cx,
                                );
                            });
                        })
                        .child(label)
                        .into_any_element()
                };
                answers = answers.child(control);
            }
            if allow_custom {
                answers = answers.child(self.render_user_ask_text_input(
                    request_id,
                    &question.id,
                    locale.text("其他回答"),
                    editable,
                ));
            }
        } else {
            answers = answers.child(self.render_user_ask_text_input(
                request_id,
                &question.id,
                locale.text("回答"),
                editable,
            ));
        }

        let previous_index = question_index.saturating_sub(1);
        let next_index = question_index.saturating_add(1);
        let previous_selector = format!("user-ask-previous-{request_id}");
        let next_selector = format!("user-ask-next-{request_id}");
        let submit_selector = format!("user-ask-submit-{request_id}");
        let can_submit = self.presenter.can_submit_user_ask(request_id);
        let submit_label = match request.submission {
            UserAskSubmissionState::Pending => locale.text("提交回答"),
            UserAskSubmissionState::Submitting => locale.text("正在提交…"),
            UserAskSubmissionState::Sent => locale.text("已发送"),
        };
        panel = panel.child(
            div()
                .px_3()
                .pb_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .text_size(px(12.))
                        .text_color(rgb(colors.muted))
                        .child(progress)
                        .child(mode_label),
                )
                .child(
                    div()
                        .debug_selector({
                            let selector =
                                format!("user-ask-question-{request_id}-{}", question.id);
                            move || selector.clone()
                        })
                        .mt_2()
                        .min_w_0()
                        .whitespace_normal()
                        .overflow_hidden()
                        .text_size(px(14.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .line_height(relative(1.45))
                        .child(question.prompt.clone()),
                )
                .child(answers)
                .when_some(request.error.clone(), |element, error| {
                    element.child(
                        div()
                            .debug_selector(move || format!("user-ask-error-{request_id}"))
                            .mt_2()
                            .flex()
                            .items_start()
                            .gap_2()
                            .text_size(px(12.))
                            .text_color(rgb(colors.danger))
                            .child(Icon::new(IconName::TriangleAlert).size(px(14.)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .whitespace_normal()
                                    .overflow_hidden()
                                    .child(error),
                            ),
                    )
                })
                .child(
                    div()
                        .mt_3()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    Button::new(SharedString::from(previous_selector.clone()))
                                        .debug_selector(move || previous_selector.clone())
                                        .ghost()
                                        .small()
                                        .size(px(COMPACT_CONTROL_HEIGHT))
                                        .p_0()
                                        .icon(IconName::ArrowLeft)
                                        .accessibility_label(locale.text("上一题"))
                                        .tooltip(locale.text("上一题"))
                                        .disabled(question_index == 0)
                                        .on_click(cx.listener(move |app, _, _, cx| {
                                            app.select_user_ask_question(
                                                request_id,
                                                previous_index,
                                                cx,
                                            );
                                        })),
                                )
                                .child(
                                    Button::new(SharedString::from(next_selector.clone()))
                                        .debug_selector(move || next_selector.clone())
                                        .ghost()
                                        .small()
                                        .size(px(COMPACT_CONTROL_HEIGHT))
                                        .p_0()
                                        .icon(IconName::ArrowRight)
                                        .accessibility_label(locale.text("下一题"))
                                        .tooltip(locale.text("下一题"))
                                        .disabled(next_index >= request.questions.len())
                                        .on_click(cx.listener(move |app, _, _, cx| {
                                            app.select_user_ask_question(
                                                request_id, next_index, cx,
                                            );
                                        })),
                                ),
                        )
                        .child(
                            Button::new(SharedString::from(submit_selector.clone()))
                                .debug_selector(move || submit_selector.clone())
                                .primary()
                                .small()
                                .h(px(COMPACT_CONTROL_HEIGHT))
                                .min_w(px(104.))
                                .icon(IconName::ArrowUp)
                                .label(submit_label)
                                .disabled(!can_submit)
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.submit_user_ask(request_id, cx);
                                })),
                        ),
                ),
        );
        panel.into_any_element()
    }

    fn render_user_ask_text_input(
        &self,
        request_id: Uuid,
        question_id: &str,
        label: &'static str,
        editable: bool,
    ) -> AnyElement {
        let selector = format!("user-ask-input-{request_id}-{question_id}");
        let Some(input) = self
            .user_ask_inputs
            .get(&(request_id, question_id.to_owned()))
        else {
            return div().into_any_element();
        };
        div()
            .debug_selector(move || selector.clone())
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_size(px(12.)).child(label))
            .child(
                Textarea::new(input)
                    .disabled(!editable)
                    .h(px(72.))
                    .aria_label(label),
            )
            .into_any_element()
    }
}
