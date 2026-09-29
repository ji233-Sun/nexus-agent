use super::*;

impl NexusView {
    pub(super) fn render_general_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let model = self.presenter.model();
        div()
            .flex()
            .flex_col()
            .gap_8()
            .child(settings_group(
                colors,
                locale.text("语言"),
                [settings_row(
                    colors,
                    locale.text("界面语言"),
                    locale.text("立即生效，并在下次启动时保留。"),
                    div().w_full().flex().gap_2().children(
                        [
                            (Language::Chinese, "简体中文"),
                            (Language::English, "English"),
                        ]
                        .map(|(language, label)| {
                            Button::new(language.as_str())
                                .debug_selector(move || format!("language-{}", language.as_str()))
                                .outline()
                                .small()
                                .flex_1()
                                .min_w_0()
                                .h(px(CONTROL_HEIGHT))
                                .accessibility_label(label)
                                .child(control_label(label))
                                .selected(locale == language)
                                .on_click(cx.listener(move |app, _, window, cx| {
                                    app.set_language(language, window, cx);
                                }))
                        }),
                    ),
                )],
            ))
            .children(
                GenerationKind::ALL
                    .map(|kind| self.render_generation_settings(kind, cx).into_any_element()),
            )
            .child(self.render_update_settings(cx))
            .child(settings_group(
                colors,
                locale.text("提示音"),
                [settings_row(
                    colors,
                    locale.text("任务完成提示音"),
                    locale.text("任务完成后播放一次提示音。"),
                    div().debug_selector(|| "task-complete-sound".into()).child(
                        Switch::new("task-complete-sound")
                            .accessibility_label(locale.text("任务完成提示音"))
                            .small()
                            .checked(model.sound.task_complete)
                            .on_click(cx.listener(|app, checked, _, cx| {
                                app.presenter.set_sound(crate::model::SoundSettings {
                                    task_complete: *checked,
                                });
                                cx.notify();
                            })),
                    ),
                )],
            ))
            .child(settings_group(
                colors,
                locale.text("命令行"),
                [settings_row(
                    colors,
                    "nexus-desktop",
                    locale.text(
                        "将命令加入当前用户的 PATH，在任意目录运行 nexus-desktop . 打开项目。",
                    ),
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap_2()
                        .child(
                            Button::new("install-cli")
                                .debug_selector(|| "install-cli".into())
                                .outline()
                                .small()
                                .disabled(
                                    model.cli_installation_busy
                                        || model.updates.state.is_installing(),
                                )
                                .label(locale.text(if model.cli_installation_busy {
                                    "正在安装 CLI…"
                                } else {
                                    "安装 CLI"
                                }))
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.presenter.install_cli();
                                    cx.notify();
                                })),
                        )
                        .when_some(
                            model.cli_installation_message.as_ref(),
                            |element, message| {
                                element.child(
                                    div()
                                        .debug_selector(|| "cli-installation-message".into())
                                        .w_full()
                                        .min_w_0()
                                        .text_sm()
                                        .text_color(rgb(colors.muted))
                                        .child(message.render(locale).to_owned()),
                                )
                            },
                        ),
                )],
            ))
            .when_some(
                model.conversation.selected_project.as_ref(),
                |element, project| {
                    element.child(settings_group(
                        colors,
                        locale.text("项目空间"),
                        [
                            settings_row(
                                colors,
                                locale.text("当前项目"),
                                locale.text("正在使用的本地项目。"),
                                project.display_name.clone(),
                            ),
                            settings_row(
                                colors,
                                locale.text("工作目录"),
                                locale.text("Agent 执行任务时使用的目录。"),
                                project.canonical_path.clone(),
                            ),
                        ],
                    ))
                },
            )
    }

    pub(super) fn render_update_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let updates = &self.presenter.model().updates;
        let busy = updates.state.is_busy();
        settings_group(
            colors,
            locale.text("软件更新"),
            [
                settings_row(
                    colors,
                    locale.text("当前版本"),
                    locale.text("此应用的构建标签。"),
                    div().debug_selector(|| "installed-version".into()).w_full().child(installed_tag()),
                ),
                settings_row(
                    colors,
                    locale.text("更新频道"),
                    locale.text(
                        "Release 包含版本号预发布；Nightly 跟随每日构建。切换后点击检查更新。",
                    ),
                    div().w_full().flex().gap_2().children(
                        [UpdateChannel::Release, UpdateChannel::Nightly].map(|channel| {
                            Button::new(channel.as_str())
                                .debug_selector(move || {
                                    format!("update-channel-{}", channel.as_str())
                                })
                                .outline()
                                .small()
                                .flex_1()
                                .min_w_0()
                                .h(px(CONTROL_HEIGHT))
                                .accessibility_label(channel.label())
                                .child(control_label(channel.label()))
                                .selected(updates.channel == channel)
                                .disabled(busy)
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.presenter.set_update_channel(channel);
                                    cx.notify();
                                }))
                        }),
                    ),
                ),
                settings_row(
                    colors,
                    locale.text("启动时检查更新"),
                    locale.text("自动检查所选频道，点击更新后才会下载并安装。"),
                    div()
                        .debug_selector(|| "update-check-on-startup".into())
                        .child(
                            Switch::new("update-check-on-startup")
                                .accessibility_label(locale.text("启动时检查更新"))
                                .small()
                                .checked(updates.check_on_startup)
                                .on_click(cx.listener(|app, checked, _, cx| {
                                    app.presenter.set_update_check_on_startup(*checked);
                                    cx.notify();
                                })),
                        ),
                ),
                div()
                    .py_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .debug_selector(|| "update-status".into())
                            .text_size(px(13.))
                            .text_color(rgb(colors.text_secondary))
                            .child(updates.state.message(locale)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                Button::new("check-for-updates")
                                    .debug_selector(|| "check-for-updates".into())
                                    .outline()
                                    .small()
                                    .label(locale.text("检查更新"))
                                    .disabled(busy)
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.presenter.check_for_updates();
                                        cx.notify();
                                    })),
                            )
                            .when(
                                matches!(updates.state, UpdateState::Available(_)),
                                |element| {
                                    element.child(
                                        Button::new("download-update")
                                            .debug_selector(|| "download-update".into())
                                            .primary()
                                            .small()
                                            .label(locale.text("更新并重启"))
                                            .on_click(cx.listener(|app, _, _, cx| {
                                                app.presenter.download_update();
                                                cx.notify();
                                            })),
                                    )
                                },
                            ),
                    )
                    .when_some(updates.state.package(), |element, package| {
                        let url = package.release_url();
                        element.child(
                            div().w_full().min_w_0().flex().flex_col().gap_3()
                                .child(div().flex().items_center().justify_between()
                                    .child(section_label(colors, locale.text("更新日志")))
                                    .child(Button::new("release-notes-link")
                                        .debug_selector(|| "release-notes-link".into())
                                        .ghost().small().label(locale.text("完整发布说明"))
                                        .on_click(move |_, _, cx| cx.open_url(&url))))
                                .child(div().text_size(px(12.)).text_color(rgb(colors.muted)).child(package.tag.clone()))
                                .child(div().id("release-notes-scroll")
                                    .debug_selector(|| "release-notes".into())
                                    .max_h(px(260.)).overflow_y_scroll().p_4()
                                    .border_1().border_color(rgb(colors.border)).rounded(px(CONTROL_RADIUS))
                                    .child(TextView::markdown("release-notes-markdown", if package.notes.trim().is_empty() {
                                        locale.text("此版本未提供更新日志，可查看完整发布说明。").to_owned()
                                    } else {
                                        package.notes.clone()
                                    }).text_size(px(13.))))
                                .when(matches!(updates.state, UpdateState::Available(_)), |element| {
                                    element.child(div().text_size(px(12.)).text_color(rgb(colors.muted))
                                        .child(locale.text("更新包校验后会自动安装并重启；运行中的任务结束后再安装。")))
                                }),
                        )
                    }),
            ],
        )
    }
}
