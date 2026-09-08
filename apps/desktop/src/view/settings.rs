use super::*;
use crate::i18n::probe_status;
use crate::infrastructure::harness_installation::documentation;
use crate::model::harness_installation::{InstallMethod, MaintenanceRequest};
use crate::model::updates::{UpdateChannel, UpdateState, installed_tag};
use gpui_kit::component::scroll::ScrollableElement as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SettingsSection {
    General,
    Appearance,
    Agent,
    Providers,
    Remote,
    Archived,
}

impl SettingsSection {
    const ALL: [Self; 6] = [
        Self::General,
        Self::Appearance,
        Self::Agent,
        Self::Providers,
        Self::Remote,
        Self::Archived,
    ];

    fn id(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Appearance => "appearance",
            Self::Agent => "agent",
            Self::Providers => "providers",
            Self::Remote => "remote",
            Self::Archived => "archived",
        }
    }

    fn label(self, locale: Language) -> &'static str {
        match self {
            Self::General => locale.text("通用"),
            Self::Appearance => locale.text("外观"),
            Self::Agent => locale.text("执行引擎"),
            Self::Providers => locale.text("凭据配置"),
            Self::Remote => locale.text("远程访问"),
            Self::Archived => locale.text("归档对话"),
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::General => IconName::Settings2,
            Self::Appearance => IconName::Palette,
            Self::Agent => IconName::Bot,
            Self::Providers => IconName::Cpu,
            Self::Remote => IconName::Globe,
            Self::Archived => IconName::Inbox,
        }
    }
}

impl NexusView {
    fn select_settings_section(
        &mut self,
        section: SettingsSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_section == section {
            return;
        }
        self.settings_section = section;
        self.settings_scroll.set_offset(gpui::point(px(0.), px(0.)));
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    pub(super) fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let material = materials(cx);
        let section = self.settings_section;
        let content = match section {
            SettingsSection::General => self.render_general_settings(cx).into_any_element(),
            SettingsSection::Appearance => self.render_appearance_settings(cx).into_any_element(),
            SettingsSection::Agent => self.render_agent_settings(cx).into_any_element(),
            SettingsSection::Providers => self.render_provider_profiles(cx).into_any_element(),
            SettingsSection::Remote => self.render_remote_settings(cx).into_any_element(),
            SettingsSection::Archived => self.render_archived_settings(cx).into_any_element(),
        };
        let titlebar_inset = if cfg!(target_os = "macos") { 36. } else { 0. };

        div()
            .debug_selector(|| "settings-page".into())
            .size_full()
            .flex()
            .child(
                div()
                    .debug_selector(|| "settings-navigation".into())
                    .w(px(240.))
                    .h_full()
                    .flex_none()
                    .pt(px(titlebar_inset))
                    .bg(material.chrome)
                    .border_r(px(0.5))
                    .border_color(material.edge)
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(HEADER_HEIGHT))
                            .flex_none()
                            .px_5()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(brand_mark(colors, 28.))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("Nexus Agent"),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .px_3()
                            .py_5()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .px(px(10.))
                                    .pb_3()
                                    .child(section_label(colors, locale.text("设置"))),
                            )
                            .children(SettingsSection::ALL.map(|section| {
                                Button::new(section.id())
                                    .ghost()
                                    .small()
                                    .w_full()
                                    .h(px(CONTROL_HEIGHT))
                                    .accessibility_label(section.label(locale))
                                    .child(
                                        div()
                                            .w_full()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .text_size(px(13.))
                                            .child(Icon::new(section.icon()).size(px(16.)))
                                            .child(section.label(locale)),
                                    )
                                    .debug_selector(move || {
                                        format!("settings-nav-{}", section.id())
                                    })
                                    .selected(section == self.settings_section)
                                    .on_click(cx.listener(move |app, _, window, cx| {
                                        app.select_settings_section(section, window, cx);
                                    }))
                            })),
                    )
                    .child(
                        div().flex_none().p_3().child(
                            Button::new("back-to-workspace")
                                .debug_selector(|| "back-to-workspace".into())
                                .ghost()
                                .small()
                                .w_full()
                                .h(px(CONTROL_HEIGHT))
                                .icon(IconName::ArrowLeft)
                                .accessibility_label(locale.text("返回工作区"))
                                .child(control_label(locale.text("返回工作区")))
                                .on_click(cx.listener(|app, _, window, cx| {
                                    app.toggle_settings(window, cx)
                                })),
                        ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .debug_selector(|| "settings-breadcrumb".into())
                            .bg(material.chrome)
                            .border_b(px(0.5))
                            .border_color(material.edge)
                            .h(px(HEADER_HEIGHT))
                            .flex_none()
                            .px_8()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .debug_selector(|| "settings-breadcrumb-label".into())
                                    .text_color(rgb(colors.muted))
                                    .child(locale.text("设置")),
                            )
                            .child(div().text_color(rgb(colors.muted)).child("/"))
                            .child(section.label(locale)),
                    )
                    .child(
                        div()
                            .id("settings-scroll")
                            .bg(rgb(colors.canvas))
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .lock_scroll_axis()
                            .track_scroll(&self.settings_scroll)
                            .pt_8()
                            .pb_8()
                            .child(
                                div().min_w_0().px_8().child(
                                    div()
                                        .debug_selector(move || {
                                            format!("settings-content-{}", section.id())
                                        })
                                        .w_full()
                                        .max_w(px(CONTENT_WIDTH))
                                        .child(content),
                                ),
                            )
                            .vertical_scrollbar(&self.settings_scroll),
                    ),
            )
    }

    fn render_general_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(self.render_title_generation_settings(cx))
            .child(self.render_update_settings(cx))
            .when_some(model.selected_project.as_ref(), |element, project| {
                element
                    .child(settings_group(
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
                    .when(model.project_dirty, |element| {
                        element.child(
                            Alert::warning(
                                "workspace-dirty",
                                locale.text("Nexus 不会自动还原或提交。"),
                            )
                            .title(locale.text("目录存在未提交修改"))
                            .small(),
                        )
                    })
            })
    }

    fn render_title_generation_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let locale = model.language;
        let colors = palette(cx);
        let selected = model.title_generation.harness;
        let app = cx.entity();
        let harness = AnimatedDropdown::new(
            "title-harness",
            Button::new("title-harness")
                .debug_selector(|| "title-harness".into())
                .outline()
                .small()
                .w_full()
                .h(px(CONTROL_HEIGHT))
                .child(harness_icon(selected, colors, 16.))
                .child(control_label(selected.to_string()))
                .accessibility_label(locale.text("标题生成引擎")),
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
                                        if app.presenter.select_title_harness(harness) {
                                            app.presenter.refresh_title_model_catalog();
                                        }
                                        cx.notify();
                                    });
                                }),
                        )
                    })
            },
        );
        let content = &self.title_model_select_content;
        let label = content
            .selected_index()
            .and_then(|index| content.groups.get(index.section)?.items.get(index.row))
            .map(|item| item.trigger_title.clone())
            .unwrap_or_else(|| locale.text("跟随默认").into());
        let app = cx.entity();
        let material = materials(cx);
        let picker = Popover::new("title-model-picker")
            .anchor(Anchor::BottomRight)
            .appearance(false)
            .open(self.title_model_picker_open)
            .track_focus(&self.title_model_select.focus_handle(cx))
            .trigger(
                Button::new("title-model")
                    .debug_selector(|| "title-model".into())
                    .outline()
                    .small()
                    .w_full()
                    .min_w_0()
                    .h(px(CONTROL_HEIGHT))
                    .icon(IconName::Cpu)
                    .tooltip(label.clone())
                    .child(control_label(label))
                    .accessibility_label(locale.text("标题生成模型"))
                    .child(Icon::new(IconName::ChevronDown).small()),
            )
            .on_open_change(move |open, window, cx| {
                app.update(cx, |app, cx| {
                    app.title_model_picker_open = *open;
                    if *open {
                        app.presenter.refresh_title_model_catalog();
                        app.title_model_select.update(cx, |list, cx| {
                            list.set_query("", window, cx);
                            let selected = list.delegate().selected_index();
                            list.set_selected_index(selected, window, cx);
                            list.scroll_to_selected_item(window, cx);
                        });
                    }
                    cx.notify();
                });
            })
            .when(self.title_model_picker_open, |picker| {
                picker.child(
                    div()
                        .debug_selector(|| "title-model-picker-surface".into())
                        .w(px(360.))
                        .h(px(320.))
                        .rounded(px(CARD_RADIUS))
                        .overflow_hidden()
                        .bg(material.floating)
                        .border_1()
                        .border_color(material.edge)
                        .shadow(material.shadow())
                        .child(
                            List::new(&self.title_model_select)
                                .search_placeholder(locale.text("按 Provider、名称或模型 ID 搜索"))
                                .size_full(),
                        ),
                )
            });
        settings_group(
            colors,
            locale.text("对话标题"),
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
                            Button::new("title-model-refresh")
                                .ghost()
                                .small()
                                .icon(IconName::RotateCw)
                                .label(locale.text("刷新模型目录"))
                                .accessibility_label(locale.text("刷新模型目录"))
                                .disabled(model.selected_project.is_none())
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.presenter.refresh_title_model_catalog();
                                    cx.notify();
                                })),
                        ),
                ),
                settings_row(
                    colors,
                    locale.text("思考档位"),
                    locale.text("仅用于生成对话标题，支持的档位由模型提供。"),
                    self.title_effort_selector(cx),
                ),
            ],
        )
    }

    fn title_effort_selector(&self, cx: &mut Context<Self>) -> AnyElement {
        let model = self.presenter.model();
        let locale = model.language;
        let selected = model.title_generation.effort;
        let mut efforts = vec![ThinkingEffort::Default];
        if let Some(descriptor) = model.title_catalog_model(model.title_generation.model.as_deref())
        {
            efforts.extend(
                descriptor
                    .supported_reasoning_efforts
                    .iter()
                    .map(|option| option.effort),
            );
        }
        let supported = efforts.len() > 1;
        let button = Button::new("title-effort")
            .debug_selector(|| "title-effort".into())
            .outline()
            .small()
            .w_full()
            .h(px(CONTROL_HEIGHT))
            .icon(IconName::Cpu)
            .label(locale.effort(selected))
            .accessibility_label(locale.text("标题生成思考档位"))
            .tooltip(if supported {
                locale.text("标题生成思考档位")
            } else {
                locale.text("刷新模型目录后可选择支持的思考档位。")
            })
            .disabled(!supported && selected.is_default());
        if !supported && selected.is_default() {
            return button.into_any_element();
        }
        let app = cx.entity();
        AnimatedDropdown::new(
            "title-effort",
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
                                        app.presenter.select_title_effort(effort);
                                        cx.notify();
                                    });
                                }),
                        )
                    })
            },
        )
        .into_any_element()
    }

    fn render_update_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

    fn render_appearance_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let appearance = self.presenter.model().appearance;
        settings_group(
            colors,
            locale.text("外观"),
            [
                div()
                    .py_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(locale.text("主题"))
                    .child(
                        div().w_full().max_w(px(600.)).flex().gap_3().children(
                            [
                                (ThemePreference::System, "system", locale.text("系统")),
                                (ThemePreference::Light, "light", locale.text("浅色")),
                                (ThemePreference::Dark, "dark", locale.text("深色")),
                            ]
                            .map(|(theme, id, label)| {
                                let selected = appearance.theme == theme;
                                Button::new(id)
                                    .debug_selector(move || format!("appearance-theme-{id}"))
                                    .ghost()
                                    .p_1()
                                    .h_auto()
                                    .flex_1()
                                    .min_w_0()
                                    .accessibility_label(
                                        locale.format(
                                            "{label}主题",
                                            &[("label", (label).to_string())],
                                        ),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            .child(theme_preview(theme).border_2().border_color(
                                                rgb(if selected {
                                                    colors.accent
                                                } else {
                                                    colors.border
                                                }),
                                            ))
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .justify_start()
                                                    .gap_1()
                                                    .text_size(px(13.))
                                                    .child(label)
                                                    .child(
                                                        Icon::new(IconName::Check)
                                                            .size(px(14.))
                                                            .opacity(if selected {
                                                                1.
                                                            } else {
                                                                0.
                                                            }),
                                                    ),
                                            ),
                                    )
                                    .on_click(cx.listener(move |app, _, window, cx| {
                                        app.set_appearance(
                                            AppearanceSettings {
                                                theme,
                                                ..app.presenter.model().appearance
                                            },
                                            window,
                                            cx,
                                        );
                                    }))
                            }),
                        ),
                    ),
                settings_row(
                    colors,
                    locale.text("玻璃效果"),
                    if cfg!(target_os = "macos") {
                        locale.text("轻透的导航与浮层；系统减少透明度时自动使用实色。")
                    } else {
                        locale.text("此平台使用有边界的实色外观。偏好仍会保存。")
                    },
                    div().debug_selector(|| "appearance-glass".into()).child(
                        Switch::new("appearance-glass")
                            .accessibility_label(locale.text("玻璃效果"))
                            .small()
                            .checked(appearance.glass)
                            .on_click(cx.listener(|app, checked, window, cx| {
                                app.set_appearance(
                                    AppearanceSettings {
                                        glass: *checked,
                                        ..app.presenter.model().appearance
                                    },
                                    window,
                                    cx,
                                );
                            })),
                    ),
                ),
                settings_row(
                    colors,
                    locale.text("减少动效"),
                    locale.text("关闭装饰动画和平滑滚动，同时遵循系统减少动效设置。"),
                    div().debug_selector(|| "reduce-motion".into()).child(
                        Switch::new("reduce-motion")
                            .accessibility_label(locale.text("减少动效"))
                            .small()
                            .checked(appearance.reduced_motion)
                            .on_click(cx.listener(|app, checked, window, cx| {
                                app.set_appearance(
                                    AppearanceSettings {
                                        reduced_motion: *checked,
                                        ..app.presenter.model().appearance
                                    },
                                    window,
                                    cx,
                                );
                            })),
                    ),
                ),
            ],
        )
    }

    fn render_archived_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let model = self.presenter.model();
        let active_run = model.active_run.is_some();
        let archived_count = model.archived_tasks.len();
        let archived_rows = if model.archived_tasks.is_empty() {
            vec![settings_row(
                colors,
                locale.text("暂无归档对话"),
                locale.text("从工作区侧栏的对话菜单中可以归档对话。"),
                div().text_size(px(12.)).child(locale.text("空")),
            )]
        } else {
            model
                .archived_tasks
                .iter()
                .map(|task| {
                    let task_id = task.id;
                    let app = cx.entity().clone();
                    let restore_app = app.clone();
                    let project_name = model
                        .projects
                        .iter()
                        .find(|project| project.id == task.project_id)
                        .map(|project| project.display_name.clone())
                        .unwrap_or_else(|| locale.text("未知项目").into());
                    settings_row(
                        colors,
                        task.title.clone(),
                        project_name,
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new((ElementId::from(task_id), "restore-archived"))
                                    .debug_selector(move || format!("restore-archived-{task_id}"))
                                    .outline()
                                    .small()
                                    .h(px(CONTROL_HEIGHT))
                                    .icon(IconName::Undo)
                                    .label(locale.text("取消归档"))
                                    .disabled(active_run)
                                    .on_click(move |_, _, cx| {
                                        restore_app
                                            .update(cx, |app, cx| app.restore_task(task_id, cx));
                                    }),
                            )
                            .child(
                                Button::new((ElementId::from(task_id), "delete-archived"))
                                    .debug_selector(move || format!("delete-archived-{task_id}"))
                                    .danger()
                                    .outline()
                                    .small()
                                    .size(px(CONTROL_HEIGHT))
                                    .icon(IconName::Delete)
                                    .tooltip(locale.text("永久删除"))
                                    .disabled(active_run)
                                    .on_click(move |_, window, cx| {
                                        app.update(cx, |app, cx| {
                                            app.confirm_delete_task(task_id, window, cx)
                                        });
                                    }),
                            ),
                    )
                })
                .collect()
        };

        div()
            .flex()
            .flex_col()
            .gap_8()
            .child(settings_group(
                colors,
                locale.text("归档管理"),
                [settings_row(
                    colors,
                    locale.text("清空归档"),
                    locale.format(
                        "当前共有 {archived_count} 个归档对话。此操作会永久删除其全部记录。",
                        &[("archived_count", (archived_count).to_string())],
                    ),
                    Button::new("delete-all-archived")
                        .debug_selector(|| "delete-all-archived".into())
                        .danger()
                        .outline()
                        .small()
                        .h(px(CONTROL_HEIGHT))
                        .icon(IconName::Delete)
                        .label(locale.text("清空全部"))
                        .disabled(active_run || archived_count == 0)
                        .on_click(cx.listener(Self::confirm_delete_archived_tasks)),
                )],
            ))
            .child(settings_group(colors, locale.text("已归档"), archived_rows))
    }

    fn render_agent_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                        ("0", (model.selected_harness).to_string()),
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
                            .disabled(model.active_run.is_some() || model.harness_manager.busy)
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
                            .disabled(model.active_run.is_some() || model.harness_manager.busy)
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

    fn render_harness_management(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let locale = model.language;
        let colors = palette(cx);
        let manager = &model.harness_manager;
        let disabled = manager.busy
            || model.active_run.is_some()
            || self.executable_input.read(cx).value().trim() != model.executable;
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
                            .disabled(manager.busy || model.active_run.is_some())
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

    fn confirm_harness_maintenance(
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

    fn start_harness_maintenance(&mut self, request: MaintenanceRequest, cx: &mut Context<Self>) {
        self.presenter.maintain_harness(request);
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    fn render_provider_profiles(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let model = self.presenter.model();
        let active_run = model.active_run.is_some();
        let editing_profile = self.editing_provider_profile.and_then(|profile_id| {
            model
                .provider_profiles
                .iter()
                .find(|profile| profile.id == profile_id)
        });
        let form_title = editing_profile
            .map(|profile| locale.format("编辑 · {0}", &[("0", (profile.name).to_string())]))
            .unwrap_or_else(|| locale.text("新建 Provider Profile").into());
        let api_key_label = match editing_profile {
            Some(profile) if profile.credential_configured => locale.text("API Key · 已安全保存"),
            Some(_) => locale.text("API Key · 需要重新填写"),
            None => "API Key",
        };

        div()
            .flex()
            .flex_col()
            .gap_8()
            .child(settings_group(
                colors,
                locale.text("当前配置"),
                [
                    settings_row(
                        colors,
                        locale.text("执行引擎"),
                        locale.text("每种引擎分别管理自己的 Provider Profile。"),
                        self.harness_selector("settings-provider-harness", false, cx),
                    ),
                    settings_row(
                        colors,
                        "Provider Profile",
                        locale.text("选择已有配置，或使用 CLI 当前凭据。"),
                        self.provider_profile_selector(
                            "settings-provider-profile",
                            false,
                            true,
                            cx,
                        ),
                    ),
                ],
            ))
            .child(settings_group(
                colors,
                form_title,
                [
                    settings_row(
                        colors,
                        locale.text("名称"),
                        locale.text("用于区分不同服务商或账户。"),
                        Input::new(&self.provider_name_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::Bot).small()),
                    ),
                    settings_row(
                        colors,
                        locale.text("API Key 环境变量"),
                        locale.text("目标引擎读取 API Key 的环境变量名。"),
                        Input::new(&self.provider_api_key_env_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::SquareTerminal).small()),
                    ),
                    settings_row(
                        colors,
                        api_key_label,
                        locale.text("保存在系统凭据库；编辑时留空保留原值。"),
                        Input::new(&self.provider_api_key_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::EyeOff).small()),
                    ),
                    settings_row(
                        colors,
                        locale.text("Base URL 环境变量"),
                        locale.text("可选，目标引擎读取服务地址的环境变量名。"),
                        Input::new(&self.provider_base_url_env_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::SquareTerminal).small()),
                    ),
                    settings_row(
                        colors,
                        "Base URL",
                        locale.text("可选，自定义 API 服务地址。"),
                        Input::new(&self.provider_base_url_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::Globe).small()),
                    ),
                    settings_row(
                        colors,
                        locale.text("默认模型"),
                        locale.text("可选，使用此配置时优先选择的模型。"),
                        Input::new(&self.provider_model_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::Cpu).small()),
                    ),
                    div()
                        .py_4()
                        .flex()
                        .items_center()
                        .flex_wrap()
                        .gap_3()
                        .child(
                            Button::new("new-provider-profile")
                                .ghost()
                                .small()
                                .h(px(CONTROL_HEIGHT))
                                .icon(IconName::Plus)
                                .label(locale.text("新建配置"))
                                .disabled(active_run)
                                .on_click(cx.listener(Self::new_provider_profile)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    Button::new("delete-provider-profile")
                                        .danger()
                                        .outline()
                                        .small()
                                        .size(px(CONTROL_HEIGHT))
                                        .icon(IconName::Delete)
                                        .tooltip(locale.text("删除当前 Provider Profile"))
                                        .disabled(active_run || editing_profile.is_none())
                                        .on_click(cx.listener(Self::delete_provider_profile)),
                                )
                                .child(
                                    Button::new("save-provider-profile")
                                        .primary()
                                        .small()
                                        .h(px(CONTROL_HEIGHT))
                                        .icon(IconName::Check)
                                        .label(locale.text("保存并启用"))
                                        .disabled(active_run)
                                        .on_click(cx.listener(Self::save_provider_profile)),
                                ),
                        ),
                ],
            ))
    }

    fn render_remote_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let remote_endpoint = self.presenter.remote_endpoint();
        let remote_available = remote_endpoint.is_some();
        let remote_token = self.presenter.remote_token().map(masked_token);
        let remote_error = self.presenter.remote_control_error().map(str::to_owned);
        div()
            .flex()
            .flex_col()
            .gap_8()
            .child(settings_group(
                colors,
                "Remote Control",
                [
                    settings_row(
                        colors,
                        locale.text("本地服务"),
                        locale.text("监听本机回环地址，可通过 FRP TCP 转发。"),
                        remote_endpoint.unwrap_or_else(|| locale.text("服务不可用").into()),
                    ),
                    settings_row(
                        colors,
                        locale.text("访问令牌"),
                        locale.text("连接远程页面时用于鉴权，请妥善保管。"),
                        remote_token.unwrap_or_else(|| locale.text("不可用").into()),
                    ),
                    settings_row(
                        colors,
                        locale.text("远程连接"),
                        locale.text("在浏览器中打开链接，即可访问远程页面。"),
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("copy-remote-link")
                                    .outline()
                                    .small()
                                    .h(px(CONTROL_HEIGHT))
                                    .icon(IconName::ExternalLink)
                                    .label(locale.text("复制链接"))
                                    .disabled(!remote_available)
                                    .on_click(cx.listener(Self::copy_remote_link)),
                            )
                            .child(
                                Button::new("copy-remote-token")
                                    .outline()
                                    .small()
                                    .h(px(CONTROL_HEIGHT))
                                    .icon(IconName::Copy)
                                    .label(locale.text("复制令牌"))
                                    .disabled(!remote_available)
                                    .on_click(cx.listener(Self::copy_remote_token)),
                            ),
                    ),
                ],
            ))
            .when_some(remote_error, |element, error| {
                element.child(
                    Alert::error("remote-control-error", error)
                        .title(locale.text("远程服务启动失败"))
                        .small(),
                )
            })
    }
}

fn settings_group(
    colors: Palette,
    title: impl Into<SharedString>,
    rows: impl IntoIterator<Item = gpui::Div>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .min_w_0()
                .truncate()
                .child(section_label(colors, title)),
        )
        .child(
            div().children(rows.into_iter().enumerate().map(|(index, row)| {
                row.when(index > 0, |row| {
                    row.border_t(px(0.5)).border_color(rgb(colors.border))
                })
            })),
        )
}

fn settings_row(
    colors: Palette,
    label: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: impl IntoElement,
) -> gpui::Div {
    div()
        .min_h(px(64.))
        .py_4()
        .flex()
        .items_center()
        .gap_6()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(label.into()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(colors.muted))
                        .line_height(relative(1.6))
                        .child(description.into()),
                ),
        )
        .child(
            div()
                .w(px(320.))
                .flex_none()
                .flex()
                .justify_start()
                .text_color(rgb(colors.text_secondary))
                .whitespace_normal()
                .child(control),
        )
}

pub(super) fn control_label(label: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .text_left()
        .truncate()
        .child(label.into())
}

fn theme_preview(theme: ThemePreference) -> gpui::Div {
    let sidebar = Palette::for_dark(theme == ThemePreference::Dark);
    let content = Palette::for_dark(theme != ThemePreference::Light);
    div()
        .w_full()
        .h(px(124.))
        .rounded(px(10.))
        .overflow_hidden()
        .flex()
        .child(
            div()
                .w(px(46.))
                .h_full()
                .flex_none()
                .bg(rgb(sidebar.surface))
                .p_2()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().flex().gap(px(3.)).children(
                    (0..3).map(|_| div().size(px(4.)).rounded_full().bg(rgb(sidebar.border))),
                ))
                .children([24., 18., 26.].map(|width| {
                    div()
                        .w(px(width))
                        .h(px(3.))
                        .rounded_full()
                        .bg(rgb(sidebar.border))
                })),
        )
        .child(
            div()
                .flex_1()
                .h_full()
                .bg(rgb(content.canvas))
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .w(px(46.))
                        .h(px(15.))
                        .ml_auto()
                        .rounded(px(4.))
                        .bg(rgb(content.recessed)),
                )
                .children([1., 0.8, 0.55].map(|width| {
                    div()
                        .w(relative(width))
                        .h(px(3.))
                        .rounded_full()
                        .bg(rgb(content.border))
                }))
                .child(
                    div()
                        .mt_auto()
                        .w_full()
                        .h(px(22.))
                        .rounded(px(6.))
                        .bg(rgb(content.surface))
                        .border_1()
                        .border_color(rgb(content.border)),
                ),
        )
}

fn masked_token(token: &str) -> String {
    if token.chars().count() <= 10 {
        return "••••••••".into();
    }
    let prefix = token.chars().take(6).collect::<String>();
    let suffix = token.chars().rev().take(4).collect::<String>();
    format!("{prefix}••••{}", suffix.chars().rev().collect::<String>())
}
