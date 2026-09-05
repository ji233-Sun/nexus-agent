use super::*;
use gpui_kit::component::scroll::ScrollableElement as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SettingsSection {
    General,
    Agent,
    Providers,
    Remote,
}

impl SettingsSection {
    const ALL: [Self; 4] = [Self::General, Self::Agent, Self::Providers, Self::Remote];

    fn id(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Agent => "agent",
            Self::Providers => "providers",
            Self::Remote => "remote",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::General => "通用",
            Self::Agent => "执行引擎",
            Self::Providers => "凭据配置",
            Self::Remote => "远程访问",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::General => IconName::Settings2,
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
        let section = self.settings_section;
        let content = match section {
            SettingsSection::General => self.render_general_settings(cx).into_any_element(),
            SettingsSection::Agent => self.render_agent_settings(cx).into_any_element(),
            SettingsSection::Providers => self.render_provider_profiles(cx).into_any_element(),
            SettingsSection::Remote => self.render_remote_settings(cx).into_any_element(),
        };
        let titlebar_inset = if cfg!(target_os = "macos") { 36. } else { 0. };

        div()
            .debug_selector(|| "settings-page".into())
            .size_full()
            .bg(rgb(CANVAS))
            .flex()
            .child(
                div()
                    .debug_selector(|| "settings-navigation".into())
                    .w(px(240.))
                    .h_full()
                    .flex_none()
                    .pt(px(titlebar_inset))
                    .bg(rgb(SURFACE))
                    .border_r_1()
                    .border_color(rgb(BORDER))
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
                            .child(brand_mark(28.))
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
                            .child(div().px(px(10.)).pb_3().child(section_label("设置")))
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
                            .h(px(HEADER_HEIGHT + titlebar_inset))
                            .pt(px(titlebar_inset))
                            .flex_none()
                            .px_8()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(div().text_color(rgb(MUTED)).child("设置"))
                            .child(div().text_color(rgb(MUTED)).child("/"))
                            .child(section.label()),
                    )
                    .child(
                        div()
                            .id("settings-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .lock_scroll_axis()
                            .track_scroll(&self.settings_scroll)
                            .px_8()
                            .pt_5()
                            .pb_8()
                            .child(
                                div()
                                    .debug_selector(move || {
                                        format!("settings-content-{}", section.id())
                                    })
                                    .w_full()
                                    .max_w(px(960.))
                                    .mx_auto()
                                    .child(content),
                            )
                            .vertical_scrollbar(&self.settings_scroll),
                    ),
            )
    }

    fn render_general_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        div()
            .flex()
            .flex_col()
            .gap_8()
            .child(settings_group(
                "交互偏好",
                [settings_row(
                    "减少动效",
                    "关闭入场位移和状态呼吸效果。本次窗口内生效。",
                    Switch::new("reduce-motion")
                        .accessibility_label("减少动效")
                        .small()
                        .checked(self.reduced_motion)
                        .on_click(cx.listener(|app, checked, _, cx| {
                            app.reduced_motion = *checked;
                            cx.notify();
                        })),
                )],
            ))
            .when_some(model.selected_project.as_ref(), |element, project| {
                element
                    .child(settings_group(
                        "项目空间",
                        [
                            settings_row(
                                "当前项目",
                                "正在使用的本地项目。",
                                project.display_name.clone(),
                            ),
                            settings_row(
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

    fn render_agent_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let probe = model.selected_probe();
        let selected_profile = model.selected_provider_profile();
        let profile_ready = selected_profile.is_some_and(|profile| profile.credential_configured);
        let harness_color: Hsla = probe
            .map(|probe| {
                if probe.available && (probe.authenticated || profile_ready) {
                    rgb(SUCCESS).into()
                } else {
                    rgb(DANGER).into()
                }
            })
            .unwrap_or_else(|| rgb(MUTED).into());
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
            "执行环境",
            [
                settings_row(
                    "执行引擎",
                    "选择用于运行任务的本地 Agent。",
                    self.harness_selector("settings-harness", false, cx),
                ),
                settings_row(
                    "可执行文件",
                    "使用命令名或完整路径，修改后重新探测环境。",
                    Input::new(&self.executable_input)
                        .small()
                        .min_h(px(CONTROL_HEIGHT))
                        .text_sm()
                        .prefix(Icon::new(IconName::SquareTerminal).small()),
                ),
                settings_row(
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
                            .text_xs()
                            .text_color(rgb(TEXT_SECONDARY))
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
                        |element, version| element.child(label_value("版本", version)),
                    ),
            ],
        )
    }

    fn render_provider_profiles(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                "当前配置",
                [
                    settings_row(
                        "执行引擎",
                        "每种引擎分别管理自己的 Provider Profile。",
                        self.harness_selector("settings-provider-harness", false, cx),
                    ),
                    settings_row(
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
                form_title,
                [
                    settings_row(
                        "名称",
                        "用于区分不同服务商或账户。",
                        Input::new(&self.provider_name_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_sm()
                            .prefix(Icon::new(IconName::Bot).small()),
                    ),
                    settings_row(
                        "API Key 环境变量",
                        "目标引擎读取 API Key 的环境变量名。",
                        Input::new(&self.provider_api_key_env_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_sm()
                            .prefix(Icon::new(IconName::SquareTerminal).small()),
                    ),
                    settings_row(
                        api_key_label,
                        "保存在系统凭据库；编辑时留空保留原值。",
                        Input::new(&self.provider_api_key_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_sm()
                            .prefix(Icon::new(IconName::EyeOff).small()),
                    ),
                    settings_row(
                        "Base URL 环境变量",
                        "可选，目标引擎读取服务地址的环境变量名。",
                        Input::new(&self.provider_base_url_env_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_sm()
                            .prefix(Icon::new(IconName::SquareTerminal).small()),
                    ),
                    settings_row(
                        "Base URL",
                        "可选，自定义 API 服务地址。",
                        Input::new(&self.provider_base_url_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_sm()
                            .prefix(Icon::new(IconName::Globe).small()),
                    ),
                    settings_row(
                        "默认模型",
                        "可选，使用此配置时优先选择的模型。",
                        Input::new(&self.provider_model_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_sm()
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
        let remote_endpoint = self.presenter.remote_endpoint();
        let remote_available = remote_endpoint.is_some();
        let remote_token = self.presenter.remote_token().map(masked_token);
        let remote_error = self.presenter.remote_control_error().map(str::to_owned);
        div()
            .flex()
            .flex_col()
            .gap_8()
            .child(settings_group(
                "Remote Control",
                [
                    settings_row(
                        "本地服务",
                        "监听本机回环地址，可通过 FRP TCP 转发。",
                        remote_endpoint.unwrap_or_else(|| "服务不可用".into()),
                    ),
                    settings_row(
                        "访问令牌",
                        "连接远程页面时用于鉴权，请妥善保管。",
                        remote_token.unwrap_or_else(|| "不可用".into()),
                    ),
                    settings_row(
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
                .px_5()
                .truncate()
                .child(section_label(title)),
        )
        .child(
            div()
                .bg(rgb(SURFACE))
                .border_1()
                .border_color(rgb(BORDER))
                .rounded(px(CARD_RADIUS))
                .children(rows.into_iter().enumerate().map(|(index, row)| {
                    row.when(index > 0, |row| row.border_t_1().border_color(rgb(BORDER)))
                })),
        )
}

fn settings_row(
    label: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: impl IntoElement,
) -> gpui::Div {
    div()
        .px_5()
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
                        .text_xs()
                        .text_color(rgb(MUTED))
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
                .text_color(rgb(TEXT_SECONDARY))
                .whitespace_normal()
                .child(control),
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
