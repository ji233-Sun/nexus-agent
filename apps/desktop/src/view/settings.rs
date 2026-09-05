use super::*;
use gpui_kit::component::scroll::ScrollableElement as _;

impl NexusView {
    pub(super) fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
        let remote_endpoint = self.presenter.remote_endpoint();
        let remote_available = remote_endpoint.is_some();
        let remote_token = self.presenter.remote_token().map(masked_token);
        let remote_error = self.presenter.remote_control_error().map(str::to_owned);
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
        let content = div()
            .w_full()
            .max_w(px(880.))
            .mx_auto()
            .bg(rgb(SURFACE))
            .border_1()
            .border_color(rgb(BORDER))
            .rounded(px(CARD_RADIUS))
            .p_6()
            .flex()
            .flex_col()
            .gap_6()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("环境与偏好"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("管理当前工作空间的执行环境"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(section_label("AGENT · 执行引擎"))
                    .child(self.harness_selector("settings-harness", false, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(section_label("可执行文件"))
                            .child(
                                Input::new(&self.executable_input)
                                    .small()
                                    .min_h(px(CONTROL_HEIGHT))
                                    .text_sm()
                                    .prefix(Icon::new(IconName::SquareTerminal).small()),
                            ),
                    )
                    .child(
                        Button::new("probe")
                            .outline()
                            .small()
                            .h(px(CONTROL_HEIGHT))
                            .w_full()
                            .icon(IconName::RotateCw)
                            .label("重新探测环境")
                            .disabled(model.active_run.is_some())
                            .on_click(cx.listener(Self::probe)),
                    )
                    .child(
                        div()
                            .rounded(px(CONTROL_RADIUS))
                            .bg(rgb(RECESSED))
                            .p_3()
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
                    ),
            )
            .child(self.render_provider_profiles(cx))
            .child(
                div()
                    .pt_5()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(section_label("REMOTE CONTROL · 远程访问"))
                    .when_some(remote_endpoint, |element, endpoint| {
                        element.child(label_value("本地服务", endpoint))
                    })
                    .when_some(remote_token, |element, token| {
                        element.child(label_value("访问令牌", token))
                    })
                    .when(remote_available, |element| {
                        element
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        Button::new("copy-remote-link")
                                            .outline()
                                            .small()
                                            .h(px(CONTROL_HEIGHT))
                                            .flex_1()
                                            .icon(IconName::ExternalLink)
                                            .label("复制链接")
                                            .on_click(cx.listener(Self::copy_remote_link)),
                                    )
                                    .child(
                                        Button::new("copy-remote-token")
                                            .outline()
                                            .small()
                                            .h(px(CONTROL_HEIGHT))
                                            .flex_1()
                                            .icon(IconName::Copy)
                                            .label("复制令牌")
                                            .on_click(cx.listener(Self::copy_remote_token)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .line_height(relative(1.6))
                                    .child("监听范围 · 本机回环地址 · FRP TCP"),
                            )
                    })
                    .when_some(remote_error, |element, error| {
                        element.child(
                            div()
                                .text_xs()
                                .text_color(rgb(DANGER))
                                .line_height(relative(1.6))
                                .child(format!("远程服务启动失败：{error}")),
                        )
                    }),
            )
            .when_some(model.selected_project.as_ref(), |element, project| {
                element.child(
                    div()
                        .pt_5()
                        .border_t_1()
                        .border_color(rgb(BORDER))
                        .flex()
                        .flex_col()
                        .gap_4()
                        .child(section_label("WORKSPACE · 项目空间"))
                        .child(label_value("项目", project.display_name.clone()))
                        .child(label_value("工作目录", project.canonical_path.clone()))
                        .when(model.project_dirty, |element| {
                            element.child(
                                Alert::warning("workspace-dirty", "Nexus 不会自动还原或提交。")
                                    .title("目录存在未提交修改")
                                    .small(),
                            )
                        }),
                )
            })
            .child(
                div()
                    .pt_5()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(section_label("PREFERENCES · 交互偏好"))
                    .child(
                        Switch::new("reduce-motion")
                            .small()
                            .label("减少动效")
                            .checked(self.reduced_motion)
                            .on_click(cx.listener(|app, checked, _, cx| {
                                app.reduced_motion = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .line_height(relative(1.6))
                            .child("关闭入场位移和状态呼吸效果。本次窗口内生效。"),
                    ),
            );

        div()
            .debug_selector(|| "settings-page".into())
            .size_full()
            .pt(px(if cfg!(target_os = "macos") { 36. } else { 0. }))
            .bg(rgb(CANVAS))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(HEADER_HEIGHT))
                    .flex_none()
                    .px_8()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        Button::new("back-to-workspace")
                            .debug_selector(|| "back-to-workspace".into())
                            .ghost()
                            .small()
                            .icon(IconName::ArrowLeft)
                            .label("返回工作区")
                            .on_click(
                                cx.listener(|app, _, window, cx| app.toggle_settings(window, cx)),
                            ),
                    )
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("设置"),
                    ),
            )
            .child(
                div()
                    .id("settings-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .lock_scroll_axis()
                    .track_scroll(&self.settings_scroll)
                    .p_8()
                    .child(content)
                    .vertical_scrollbar(&self.settings_scroll),
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
            .pt_5()
            .border_t_1()
            .border_color(rgb(BORDER))
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(section_label("PROVIDER · 凭据配置"))
                    .child(
                        Button::new("new-provider-profile")
                            .ghost()
                            .small()
                            .size(px(COMPACT_CONTROL_HEIGHT))
                            .icon(IconName::Plus)
                            .tooltip("新建 Provider Profile")
                            .disabled(active_run)
                            .on_click(cx.listener(Self::new_provider_profile)),
                    ),
            )
            .child(self.provider_profile_selector("settings-provider-profile", false, true, cx))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_xs()
                    .text_color(rgb(TEXT_SECONDARY))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(form_title),
            )
            .child(profile_form_field(
                "名称",
                Input::new(&self.provider_name_input)
                    .small()
                    .min_h(px(CONTROL_HEIGHT))
                    .text_sm()
                    .prefix(Icon::new(IconName::Bot).small()),
            ))
            .child(profile_form_field(
                "API Key 环境变量",
                Input::new(&self.provider_api_key_env_input)
                    .small()
                    .min_h(px(CONTROL_HEIGHT))
                    .text_sm()
                    .prefix(Icon::new(IconName::SquareTerminal).small()),
            ))
            .child(profile_form_field(
                api_key_label,
                Input::new(&self.provider_api_key_input)
                    .small()
                    .min_h(px(CONTROL_HEIGHT))
                    .text_sm()
                    .prefix(Icon::new(IconName::EyeOff).small()),
            ))
            .child(profile_form_field(
                "Base URL 环境变量",
                Input::new(&self.provider_base_url_env_input)
                    .small()
                    .min_h(px(CONTROL_HEIGHT))
                    .text_sm()
                    .prefix(Icon::new(IconName::SquareTerminal).small()),
            ))
            .child(profile_form_field(
                "Base URL",
                Input::new(&self.provider_base_url_input)
                    .small()
                    .min_h(px(CONTROL_HEIGHT))
                    .text_sm()
                    .prefix(Icon::new(IconName::Globe).small()),
            ))
            .child(profile_form_field(
                "默认模型",
                Input::new(&self.provider_model_input)
                    .small()
                    .min_h(px(CONTROL_HEIGHT))
                    .text_sm()
                    .prefix(Icon::new(IconName::Cpu).small()),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("save-provider-profile")
                            .primary()
                            .small()
                            .h(px(CONTROL_HEIGHT))
                            .flex_1()
                            .icon(IconName::Check)
                            .label("保存并启用")
                            .disabled(active_run)
                            .on_click(cx.listener(Self::save_provider_profile)),
                    )
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
                    ),
            )
    }
}

fn profile_form_field(label: &'static str, input: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(section_label(label))
        .child(input)
}

fn masked_token(token: &str) -> String {
    if token.chars().count() <= 10 {
        return "••••••••".into();
    }
    let prefix = token.chars().take(6).collect::<String>();
    let suffix = token.chars().rev().take(4).collect::<String>();
    format!("{prefix}••••{}", suffix.chars().rev().collect::<String>())
}
