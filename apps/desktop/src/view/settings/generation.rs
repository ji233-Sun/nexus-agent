use super::*;

impl NexusView {
    pub(super) fn render_generation_settings(
        &self,
        kind: GenerationKind,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let model = self.presenter.model();
        let locale = model.language;
        let colors = palette(cx);
        let selected = model.generation_settings(kind).harness;
        let state = &self.generation_pickers[&kind];
        let prefix = kind.prefix();
        let app = cx.entity();
        let harness = AnimatedDropdown::new(
            SharedString::from(format!("{prefix}-harness")),
            Button::new(SharedString::from(format!("{prefix}-harness")))
                .debug_selector(move || format!("{prefix}-harness"))
                .outline()
                .small()
                .w_full()
                .h(px(CONTROL_HEIGHT))
                .child(harness_icon(selected, colors, 16.))
                .child(control_label(selected.to_string()))
                .accessibility_label(locale.text("执行引擎")),
            self.reduced_motion,
            move |menu, _, _| {
                HarnessKind::ALL
                    .into_iter()
                    .fold(menu.min_w(px(220.)), |menu, harness| {
                        let app = app.clone();
                        menu.item(
                            PopupMenuItem::new(harness.to_string())
                                .checked(harness == selected)
                                .on_click(move |_, _, cx| {
                                    app.update(cx, |app, cx| {
                                        if app.presenter.select_generation_harness(kind, harness) {
                                            app.presenter.refresh_generation_model_catalog(kind);
                                        }
                                        cx.notify();
                                    });
                                }),
                        )
                    })
            },
        );
        let content = &state.content;
        let label = content
            .selected_index()
            .and_then(|index| content.groups.get(index.section)?.items.get(index.row))
            .map(|item| item.trigger_title.clone())
            .unwrap_or_else(|| locale.text("跟随默认").into());
        let app = cx.entity();
        let material = materials(cx);
        let picker = Popover::new(SharedString::from(format!("{prefix}-model-picker")))
            .anchor(Anchor::BottomRight)
            .appearance(false)
            .open(state.open)
            .track_focus(&state.list.focus_handle(cx))
            .trigger(
                Button::new(SharedString::from(format!("{prefix}-model")))
                    .debug_selector(move || format!("{prefix}-model"))
                    .outline()
                    .small()
                    .w_full()
                    .min_w_0()
                    .h(px(CONTROL_HEIGHT))
                    .icon(IconName::Cpu)
                    .tooltip(label.clone())
                    .child(control_label(label))
                    .accessibility_label(locale.text("模型"))
                    .child(Icon::new(IconName::ChevronDown).small()),
            )
            .on_open_change(move |open, window, cx| {
                app.update(cx, |app, cx| {
                    app.generation_pickers.get_mut(&kind).unwrap().open = *open;
                    if *open {
                        app.presenter.refresh_generation_model_catalog(kind);
                        app.generation_pickers[&kind].list.update(cx, |list, cx| {
                            list.set_query("", window, cx);
                            let selected = list.delegate().selected_index();
                            list.set_selected_index(selected, window, cx);
                            list.scroll_to_selected_item(window, cx);
                        });
                    }
                    cx.notify();
                });
            })
            .when(state.open, |picker| {
                picker.child(
                    div()
                        .debug_selector(move || format!("{prefix}-model-picker-surface"))
                        .w(px(360.))
                        .h(px(320.))
                        .rounded(px(CARD_RADIUS))
                        .overflow_hidden()
                        .bg(material.floating)
                        .border_1()
                        .border_color(material.edge)
                        .shadow(material.shadow())
                        .child(
                            List::new(&state.list)
                                .search_placeholder(state.content.search_placeholder())
                                .size_full(),
                        ),
                )
            });
        settings_group(
            colors,
            locale.text(match kind {
                GenerationKind::Title => "对话标题",
                GenerationKind::Commit => "Git 提交说明",
            }),
            [
                settings_row(colors, locale.text("执行引擎"), "", harness),
                settings_row(
                    colors,
                    locale.text("模型"),
                    model.provider_profile_for(selected).map_or_else(
                        || locale.text("CLI 默认配置").to_owned(),
                        |profile| profile.name.clone(),
                    ),
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap_2()
                        .child(div().w_full().min_w_0().child(picker))
                        .child(
                            Button::new(SharedString::from(format!("{prefix}-model-refresh")))
                                .ghost()
                                .small()
                                .icon(IconName::RotateCw)
                                .label(locale.text("刷新模型目录"))
                                .accessibility_label(locale.text("刷新模型目录"))
                                .disabled(model.working_directory().is_none())
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.presenter.refresh_generation_model_catalog(kind);
                                    cx.notify();
                                })),
                        ),
                ),
                settings_row(
                    colors,
                    locale.text("思考档位"),
                    locale.text(match kind {
                        GenerationKind::Title => "仅用于生成对话标题，支持的档位由模型提供。",
                        GenerationKind::Commit => "仅用于生成提交说明，支持的档位由模型提供。",
                    }),
                    self.generation_effort_selector(kind, cx),
                ),
            ],
        )
    }

    pub(super) fn generation_effort_selector(
        &self,
        kind: GenerationKind,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let model = self.presenter.model();
        let locale = model.language;
        let settings = model.generation_settings(kind);
        let selected = settings.effort;
        let prefix = kind.prefix();
        let mut efforts = vec![ThinkingEffort::Default];
        if let Some(descriptor) = model.generation_catalog_model(kind, settings.model.as_deref()) {
            efforts.extend(
                descriptor
                    .supported_reasoning_efforts
                    .iter()
                    .map(|option| option.effort),
            );
        }
        let supported = efforts.len() > 1;
        let button = Button::new(SharedString::from(format!("{prefix}-effort")))
            .debug_selector(move || format!("{prefix}-effort"))
            .outline()
            .small()
            .w_full()
            .h(px(CONTROL_HEIGHT))
            .icon(IconName::Cpu)
            .label(locale.effort(selected))
            .accessibility_label(locale.text("思考档位"))
            .tooltip(if supported {
                locale.text("思考档位")
            } else {
                locale.text("刷新模型目录后可选择支持的思考档位。")
            })
            .disabled(!supported && selected.is_default());
        if !supported && selected.is_default() {
            return button.into_any_element();
        }
        let app = cx.entity();
        AnimatedDropdown::new(
            SharedString::from(format!("{prefix}-effort")),
            button,
            self.reduced_motion,
            move |menu, _, _| {
                efforts
                    .iter()
                    .copied()
                    .fold(menu.min_w(px(180.)), |menu, effort| {
                        let app = app.clone();
                        menu.item(
                            PopupMenuItem::new(locale.effort(effort))
                                .checked(effort == selected)
                                .on_click(move |_, _, cx| {
                                    app.update(cx, |app, cx| {
                                        app.presenter.select_generation_effort(kind, effort);
                                        cx.notify();
                                    });
                                }),
                        )
                    })
            },
        )
        .into_any_element()
    }
}
