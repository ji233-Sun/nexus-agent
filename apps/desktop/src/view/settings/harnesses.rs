use super::*;

impl NexusView {
    pub(super) fn render_agent_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let model = self.presenter.model();
        let probe = model.selected_probe();
        let selected_profile = model.selected_provider_profile();
        let profile_ready = selected_profile.is_some_and(|profile| profile.credential_configured);
        let harness_color: Hsla = probe
            .map(|probe| {
                if probe.available && (probe.authenticated || profile_ready) {
                    rgb(colors.success).into()
                } else {
                    rgb(colors.danger).into()
                }
            })
            .unwrap_or_else(|| rgb(colors.muted).into());
        let harness_status = match (probe, selected_profile) {
            (Some(probe), Some(profile)) if probe.available && profile.credential_configured => {
                locale.format(
                    "{0} 可执行文件已就绪，将使用 Provider Profile：{1}",
                    &[
                        ("0", (model.conversation.selected_harness).to_string()),
                        ("1", (profile.name).to_string()),
                    ],
                )
            }
            (Some(probe), _) => probe_status(probe).render(locale).to_owned(),
            (None, _) => locale.text("尚未探测").into(),
        };

        div()
            .flex()
            .flex_col()
            .gap_8()
            .child(self.render_harness_management(cx))
            .when(
                model.conversation.selected_harness.has_transport_choice(),
                |view| {
                    view.child(settings_group(
                        colors,
                        locale.text("接入方式"),
                        [settings_row(
                            colors,
                            locale.text("运行协议"),
                            locale.text("新会话默认使用 CLI；已有会话沿用原接入方式。"),
                            div().flex().gap_2().children(
                                [
                                    (nexus_domain::HarnessTransport::Cli, "CLI"),
                                    (nexus_domain::HarnessTransport::Acp, "ACP"),
                                ]
                                .map(|(transport, label)| {
                                    Button::new(label)
                                        .outline()
                                        .small()
                                        .label(label)
                                        .selected(
                                            self.presenter.harness_transport(
                                                model.conversation.selected_harness,
                                            ) == transport,
                                        )
                                        .disabled(
                                            model.active_run_count() > 0
                                                || model.harness_manager.busy,
                                        )
                                        .on_click(cx.listener(move |app, _, _, cx| {
                                            app.presenter.set_harness_transport(transport);
                                            cx.notify();
                                        }))
                                }),
                            ),
                        )],
                    ))
                },
            )
            .when(
                model.conversation.selected_harness == HarnessKind::Codebuddy,
                |view| {
                    view.child(settings_group(
                        colors,
                        locale.text("CodeBuddy 地区"),
                        [settings_row(
                            colors,
                            locale.text("账号地区"),
                            locale.text("使用对应地区的登录与服务端点，不修改 CLI 全局配置。"),
                            div().flex().gap_2().children(
                                [("", "跟随 CLI"), ("internal", "国内"), ("external", "国际")].map(
                                    |(region, label)| {
                                        Button::new(label)
                                            .outline()
                                            .small()
                                            .label(locale.text(label))
                                            .selected(self.presenter.codebuddy_region() == region)
                                            .disabled(
                                                model.active_run_count() > 0
                                                    || model.harness_manager.busy,
                                            )
                                            .on_click(cx.listener(move |app, _, _, cx| {
                                                app.presenter.set_codebuddy_region(region);
                                                cx.notify();
                                            }))
                                    },
                                ),
                            ),
                        )],
                    ))
                },
            )
            .child(settings_group(
                colors,
                locale.text("执行环境"),
                [
                    settings_row(
                        colors,
                        locale.text("执行引擎"),
                        locale.text("选择用于运行任务的本地 Agent。"),
                        self.harness_selector("settings-harness", false, cx),
                    ),
                    settings_row(
                        colors,
                        locale.text("可执行文件"),
                        locale.text("使用命令名或完整路径，修改后重新探测环境。"),
                        Input::new(&self.executable_input)
                            .disabled(model.active_run_count() > 0 || model.harness_manager.busy)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::SquareTerminal).small()),
                    ),
                    settings_row(
                        colors,
                        locale.text("环境检测"),
                        locale.text("检查可执行文件、版本和登录状态。"),
                        Button::new("probe")
                            .outline()
                            .small()
                            .h(px(CONTROL_HEIGHT))
                            .icon(IconName::RotateCw)
                            .label(locale.text("重新探测环境"))
                            .disabled(model.active_run_count() > 0 || model.harness_manager.busy)
                            .on_click(cx.listener(Self::probe)),
                    ),
                    div()
                        .py_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .gap_2()
                                .text_size(px(12.))
                                .text_color(rgb(colors.text_secondary))
                                .child(status_dot(harness_color))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .whitespace_normal()
                                        .child(harness_status),
                                ),
                        )
                        .when_some(
                            probe.and_then(|probe| probe.version.clone()),
                            |element, version| {
                                element.child(label_value(colors, locale.text("版本"), version))
                            },
                        )
                        .when_some(
                            probe.filter(|probe| {
                                locale == Language::English
                                    && (!probe.available || !probe.authenticated)
                            }),
                            |element, probe| {
                                element.child(label_value(
                                    colors,
                                    locale.text("原始诊断"),
                                    probe.message.clone(),
                                ))
                            },
                        ),
                ],
            ))
    }

    pub(super) fn render_harness_management(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let locale = model.language;
        let colors = palette(cx);
        let manager = &model.harness_manager;
        let disabled = manager.busy
            || model.active_run_count() > 0
            || self.executable_input.read(cx).value().trim() != model.conversation.executable;
        let cards = HarnessKind::ALL.map(|harness| {
            let harness_id = harness.as_str();
            let installation = manager.installations.get(&harness);
            let installed =
                installation.is_some_and(|installation| installation.executable.is_some());
            let update = installation.and_then(|installation| installation.update.as_ref());
            let update_available =
                installation.is_some_and(|installation| installation.available_update().is_some());
            let options = installation
                .map(|installation| installation.install_options.clone())
                .unwrap_or_default();
            let source = installation
                .map(|installation| installation.source.render(locale).to_owned())
                .unwrap_or_else(|| locale.text("尚未扫描").into());
            let version = installation
                .and_then(|installation| installation.version.clone())
                .unwrap_or_else(|| {
                    locale
                        .text(if installation.is_none() {
                            "尚未扫描"
                        } else if installed {
                            "版本未知"
                        } else {
                            "未安装"
                        })
                        .into()
                });
            let latest_version = installation
                .map(|installation| match &installation.latest_version {
                    Ok(version) => version.clone(),
                    Err(error) => error.render(locale).to_owned(),
                })
                .unwrap_or_else(|| locale.text("尚未扫描").into());
            let app = cx.entity();
            let actions = div()
                .flex()
                .items_center()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new(SharedString::from(format!("harness-docs-{harness_id}")))
                        .ghost()
                        .small()
                        .label(locale.text("安装文档"))
                        .on_click(move |_, _, cx| cx.open_url(documentation(harness))),
                )
                .when_some(update, |element, command| {
                    let display = command.display();
                    element
                        .child(
                            Button::new(SharedString::from(format!(
                                "harness-command-{harness_id}"
                            )))
                            .ghost()
                            .small()
                            .icon(IconName::Copy)
                            .label(locale.text("复制命令"))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(display.clone()))
                            }),
                        )
                        .when(update_available, |element| {
                            element.child(
                                Button::new(SharedString::from(format!(
                                    "harness-update-{harness_id}"
                                )))
                                .debug_selector(move || format!("harness-update-{harness_id}"))
                                .outline()
                                .small()
                                .icon(IconName::RotateCw)
                                .label(locale.text("更新"))
                                .disabled(disabled)
                                .on_click(cx.listener(
                                    move |app, _, window, cx| {
                                        app.confirm_harness_maintenance(harness, None, window, cx)
                                    },
                                )),
                            )
                        })
                })
                .when(!options.is_empty(), |element| {
                    element.child(AnimatedDropdown::new(
                        SharedString::from(format!("harness-install-menu-{harness_id}")),
                        Button::new(SharedString::from(format!("harness-install-{harness_id}")))
                            .debug_selector(move || format!("harness-install-{harness_id}"))
                            .primary()
                            .small()
                            .icon(IconName::Plus)
                            .label(locale.text("安装"))
                            .disabled(disabled),
                        self.reduced_motion,
                        move |menu, _, _| {
                            options.iter().fold(menu.min_w(px(200.)), |menu, option| {
                                let app = app.clone();
                                let method = option.method;
                                menu.item(PopupMenuItem::new(locale.text(method.label())).on_click(
                                    move |_, window, cx| {
                                        app.update(cx, |app, cx| {
                                            app.confirm_harness_maintenance(
                                                harness,
                                                Some(method),
                                                window,
                                                cx,
                                            )
                                        });
                                    },
                                ))
                            })
                        },
                    ))
                });
            div()
                .debug_selector(move || format!("harness-card-{harness_id}"))
                .border_1()
                .border_color(rgb(colors.border))
                .rounded_lg()
                .p_4()
                .flex()
                .flex_col()
                .gap_3()
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(harness_icon(harness, colors, 20.))
                        .child(div().flex_1().child(harness.to_string())),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_6()
                        .child(
                            div()
                                .debug_selector(move || {
                                    format!("harness-current-version-{harness_id}")
                                })
                                .child(label_value(colors, locale.text("当前版本"), version)),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .debug_selector(move || {
                                    format!("harness-latest-version-{harness_id}")
                                })
                                .child(label_value(
                                    colors,
                                    locale.text("最新版本"),
                                    latest_version,
                                )),
                        ),
                )
                .child(label_value(colors, locale.text("安装来源"), source))
                .when_some(
                    installation.and_then(|installation| installation.executable.as_ref()),
                    |element, path| {
                        element.child(label_value(
                            colors,
                            locale.text("实际路径"),
                            path.display().to_string(),
                        ))
                    },
                )
                .when_some(
                    installation.and_then(|installation| installation.diagnostic.as_ref()),
                    |element, diagnostic| {
                        element.child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(colors.muted))
                                .whitespace_normal()
                                .child(diagnostic.render(locale).to_owned()),
                        )
                    },
                )
                .child(actions)
        });
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .child(section_label(colors, locale.text("Harness 管理"))),
                    )
                    .child(
                        Button::new("scan-harness-installations")
                            .debug_selector(|| "scan-harness-installations".into())
                            .outline()
                            .small()
                            .icon(IconName::RotateCw)
                            .label(locale.text("重新扫描"))
                            .disabled(manager.busy || model.active_run_count() > 0)
                            .on_click(cx.listener(Self::probe)),
                    )
                    .when(manager.busy, |element| {
                        element.child(
                            Button::new("cancel-harness-maintenance")
                                .ghost()
                                .small()
                                .label(locale.text("取消"))
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.presenter.cancel_harness_maintenance();
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors.muted))
                    .child(
                        locale.text(
                            "扫描本机安装，更新时沿用原安装来源。任务运行期间暂停安装和更新。",
                        ),
                    ),
            )
            .when_some(manager.message.as_ref(), |element, message| {
                element.child(
                    div()
                        .debug_selector(|| "harness-management-status".into())
                        .text_size(px(12.))
                        .whitespace_normal()
                        .child(message.render(locale).to_owned()),
                )
            })
            .children(cards)
    }

    pub(super) fn confirm_harness_maintenance(
        &mut self,
        harness: HarnessKind,
        method: Option<InstallMethod>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self.presenter.harness_maintenance_request(harness, method) else {
            return;
        };
        let locale = self.presenter.model().language;
        let title = locale.format(
            "安装或更新 {harness}？",
            &[("harness", harness.to_string())],
        );
        let detail = request.command.display();
        let answer = window.prompt(
            PromptLevel::Info,
            &title,
            Some(&detail),
            &[
                PromptButton::ok(locale.text("执行命令")),
                PromptButton::cancel(locale.text("取消")),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() == Some(0) {
                let _ = this.update_in(cx, |app, _, cx| {
                    app.start_harness_maintenance(request, cx);
                });
            }
        })
        .detach();
    }

    pub(super) fn start_harness_maintenance(
        &mut self,
        request: MaintenanceRequest,
        cx: &mut Context<Self>,
    ) {
        self.presenter.maintain_harness(request);
        self.presenter.notify_remote_changed();
        cx.notify();
    }
}
