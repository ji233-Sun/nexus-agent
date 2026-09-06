use super::*;
use gpui_kit::component::scroll::ScrollableElement as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SettingsSection {
    General,
    Archived,
    Agent,
    Providers,
    Remote,
}

impl SettingsSection {
    const ALL: [Self; 5] = [
        Self::General,
        Self::Archived,
        Self::Agent,
        Self::Providers,
        Self::Remote,
    ];

    fn id(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Archived => "archived",
            Self::Agent => "agent",
            Self::Providers => "providers",
            Self::Remote => "remote",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::General => "通用",
            Self::Archived => "归档对话",
            Self::Agent => "执行引擎",
            Self::Providers => "凭据配置",
            Self::Remote => "远程访问",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::General => IconName::Settings2,
            Self::Archived => IconName::Inbox,
            Self::Agent => IconName::Bot,
            Self::Providers => IconName::Cpu,
            Self::Remote => IconName::Globe,
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
        let colors = palette(cx);
        let material = materials(cx);
        let section = self.settings_section;
        let content = match section {
            SettingsSection::General => self.render_general_settings(cx).into_any_element(),
            SettingsSection::Archived => self.render_archived_settings(cx).into_any_element(),
            SettingsSection::Agent => self.render_agent_settings(cx).into_any_element(),
            SettingsSection::Providers => self.render_provider_profiles(cx).into_any_element(),
            SettingsSection::Remote => self.render_remote_settings(cx).into_any_element(),
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
                                    .child(section_label(colors, "设置")),
                            )
                            .children(SettingsSection::ALL.map(|section| {
                                Button::new(section.id())
                                    .ghost()
                                    .small()
                                    .w_full()
                                    .h(px(CONTROL_HEIGHT))
                                    .accessibility_label(section.label())
                                    .child(
                                        div()
                                            .w_full()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .text_size(px(13.))
                                            .child(Icon::new(section.icon()).size(px(16.)))
                                            .child(section.label()),
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
                                .label("返回工作区")
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
                            .child(div().text_color(rgb(colors.muted)).child("设置"))
                            .child(div().text_color(rgb(colors.muted)).child("/"))
                            .child(section.label()),
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
                            .px_8()
                            .pt_8()
                            .pb_8()
                            .child(
                                div()
                                    .debug_selector(move || {
                                        format!("settings-content-{}", section.id())
                                    })
                                    .w_full()
                                    .max_w(px(CONTENT_WIDTH))
                                    .mx_auto()
                                    .child(content),
                            )
                            .vertical_scrollbar(&self.settings_scroll),
                    ),
            )
    }

    fn render_general_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = palette(cx);
        let model = self.presenter.model();
        let appearance = model.appearance;
        div()
            .flex()
            .flex_col()
            .gap_8()
            .child(settings_group(
                colors,
                "外观",
                [
                    div().py_4().flex().flex_col().gap_3().child("主题").child(
                        div().w_full().max_w(px(600.)).flex().gap_3().children(
                            [
                                (ThemePreference::System, "system", "系统"),
                                (ThemePreference::Light, "light", "浅色"),
                                (ThemePreference::Dark, "dark", "深色"),
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
                                    .accessibility_label(format!("{label}主题"))
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
                                                    .justify_center()
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
                        "玻璃效果",
                        if cfg!(target_os = "macos") {
                            "轻透的导航与浮层；系统减少透明度时自动使用实色。"
                        } else {
                            "此平台使用有边界的实色外观。偏好仍会保存。"
                        },
                        div().debug_selector(|| "appearance-glass".into()).child(
                            Switch::new("appearance-glass")
                                .accessibility_label("玻璃效果")
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
                        "减少动效",
                        "关闭装饰动画和平滑滚动，同时遵循系统减少动效设置。",
                        div().debug_selector(|| "reduce-motion".into()).child(
                            Switch::new("reduce-motion")
                                .accessibility_label("减少动效")
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
            ))
            .when_some(model.selected_project.as_ref(), |element, project| {
                element
                    .child(settings_group(
                        colors,
                        "项目空间",
                        [
                            settings_row(
                                colors,
                                "当前项目",
                                "正在使用的本地项目。",
                                project.display_name.clone(),
                            ),
                            settings_row(
                                colors,
                                "工作目录",
                                "Agent 执行任务时使用的目录。",
                                project.canonical_path.clone(),
                            ),
                        ],
                    ))
                    .when(model.project_dirty, |element| {
                        element.child(
                            Alert::warning("workspace-dirty", "Nexus 不会自动还原或提交。")
                                .title("目录存在未提交修改")
                                .small(),
                        )
                    })
            })
    }

    fn render_archived_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = palette(cx);
        let model = self.presenter.model();
        let active_run = model.active_run.is_some();
        let archived_count = model.archived_tasks.len();
        let archived_rows = if model.archived_tasks.is_empty() {
            vec![settings_row(
                colors,
                "暂无归档对话",
                "从工作区侧栏的对话菜单中可以归档对话。",
                div().text_size(px(12.)).child("空"),
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
                        .unwrap_or_else(|| "未知项目".into());
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
                                    .label("取消归档")
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
                                    .tooltip("永久删除")
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
                "归档管理",
                [settings_row(
                    colors,
                    "清空归档",
                    format!("当前共有 {archived_count} 个归档对话。此操作会永久删除其全部记录。"),
                    Button::new("delete-all-archived")
                        .debug_selector(|| "delete-all-archived".into())
                        .danger()
                        .outline()
                        .small()
                        .h(px(CONTROL_HEIGHT))
                        .icon(IconName::Delete)
                        .label("清空全部")
                        .disabled(active_run || archived_count == 0)
                        .on_click(cx.listener(Self::confirm_delete_archived_tasks)),
                )],
            ))
            .child(settings_group(colors, "已归档", archived_rows))
    }

    fn render_agent_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                format!(
                    "{} 可执行文件已就绪，将使用 Provider Profile：{}",
                    model.selected_harness, profile.name
                )
            }
            (Some(probe), _) => probe.message.clone(),
            (None, _) => "尚未探测".into(),
        };

        settings_group(
            colors,
            "执行环境",
            [
                settings_row(
                    colors,
                    "执行引擎",
                    "选择用于运行任务的本地 Agent。",
                    self.harness_selector("settings-harness", false, cx),
                ),
                settings_row(
                    colors,
                    "可执行文件",
                    "使用命令名或完整路径，修改后重新探测环境。",
                    Input::new(&self.executable_input)
                        .small()
                        .min_h(px(CONTROL_HEIGHT))
                        .text_size(px(13.))
                        .prefix(Icon::new(IconName::SquareTerminal).small()),
                ),
                settings_row(
                    colors,
                    "环境检测",
                    "检查可执行文件、版本和登录状态。",
                    Button::new("probe")
                        .outline()
                        .small()
                        .h(px(CONTROL_HEIGHT))
                        .icon(IconName::RotateCw)
                        .label("重新探测环境")
                        .disabled(model.active_run.is_some())
                        .on_click(cx.listener(Self::probe)),
                ),
                div()
                    .p_5()
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
                        |element, version| element.child(label_value(colors, "版本", version)),
                    ),
            ],
        )
    }

    fn render_provider_profiles(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            .map(|profile| format!("编辑 · {}", profile.name))
            .unwrap_or_else(|| "新建 Provider Profile".into());
        let api_key_label = match editing_profile {
            Some(profile) if profile.credential_configured => "API Key · 已安全保存",
            Some(_) => "API Key · 需要重新填写",
            None => "API Key",
        };

        div()
            .flex()
            .flex_col()
            .gap_8()
            .child(settings_group(
                colors,
                "当前配置",
                [
                    settings_row(
                        colors,
                        "执行引擎",
                        "每种引擎分别管理自己的 Provider Profile。",
                        self.harness_selector("settings-provider-harness", false, cx),
                    ),
                    settings_row(
                        colors,
                        "Provider Profile",
                        "选择已有配置，或使用 CLI 当前凭据。",
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
                        "名称",
                        "用于区分不同服务商或账户。",
                        Input::new(&self.provider_name_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::Bot).small()),
                    ),
                    settings_row(
                        colors,
                        "API Key 环境变量",
                        "目标引擎读取 API Key 的环境变量名。",
                        Input::new(&self.provider_api_key_env_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::SquareTerminal).small()),
                    ),
                    settings_row(
                        colors,
                        api_key_label,
                        "保存在系统凭据库；编辑时留空保留原值。",
                        Input::new(&self.provider_api_key_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::EyeOff).small()),
                    ),
                    settings_row(
                        colors,
                        "Base URL 环境变量",
                        "可选，目标引擎读取服务地址的环境变量名。",
                        Input::new(&self.provider_base_url_env_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::SquareTerminal).small()),
                    ),
                    settings_row(
                        colors,
                        "Base URL",
                        "可选，自定义 API 服务地址。",
                        Input::new(&self.provider_base_url_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::Globe).small()),
                    ),
                    settings_row(
                        colors,
                        "默认模型",
                        "可选，使用此配置时优先选择的模型。",
                        Input::new(&self.provider_model_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::Cpu).small()),
                    ),
                    div()
                        .p_5()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(
                            Button::new("new-provider-profile")
                                .ghost()
                                .small()
                                .h(px(CONTROL_HEIGHT))
                                .icon(IconName::Plus)
                                .label("新建配置")
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
                                        .tooltip("删除当前 Provider Profile")
                                        .disabled(active_run || editing_profile.is_none())
                                        .on_click(cx.listener(Self::delete_provider_profile)),
                                )
                                .child(
                                    Button::new("save-provider-profile")
                                        .primary()
                                        .small()
                                        .h(px(CONTROL_HEIGHT))
                                        .icon(IconName::Check)
                                        .label("保存并启用")
                                        .disabled(active_run)
                                        .on_click(cx.listener(Self::save_provider_profile)),
                                ),
                        ),
                ],
            ))
    }

    fn render_remote_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                        "本地服务",
                        "监听本机回环地址，可通过 FRP TCP 转发。",
                        remote_endpoint.unwrap_or_else(|| "服务不可用".into()),
                    ),
                    settings_row(
                        colors,
                        "访问令牌",
                        "连接远程页面时用于鉴权，请妥善保管。",
                        remote_token.unwrap_or_else(|| "不可用".into()),
                    ),
                    settings_row(
                        colors,
                        "远程连接",
                        "在浏览器中打开链接，即可访问远程页面。",
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("copy-remote-link")
                                    .outline()
                                    .small()
                                    .h(px(CONTROL_HEIGHT))
                                    .icon(IconName::ExternalLink)
                                    .label("复制链接")
                                    .disabled(!remote_available)
                                    .on_click(cx.listener(Self::copy_remote_link)),
                            )
                            .child(
                                Button::new("copy-remote-token")
                                    .outline()
                                    .small()
                                    .h(px(CONTROL_HEIGHT))
                                    .icon(IconName::Copy)
                                    .label("复制令牌")
                                    .disabled(!remote_available)
                                    .on_click(cx.listener(Self::copy_remote_token)),
                            ),
                    ),
                ],
            ))
            .when_some(remote_error, |element, error| {
                element.child(
                    Alert::error("remote-control-error", error)
                        .title("远程服务启动失败")
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
                .justify_end()
                .text_color(rgb(colors.text_secondary))
                .whitespace_normal()
                .child(control),
        )
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
