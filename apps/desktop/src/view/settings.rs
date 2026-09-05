use super::*;

impl NexusView {
    pub(super) fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let probe = model.selected_probe();
        let harness_color: Hsla = probe
            .map(|probe| {
                if probe.available && probe.authenticated {
                    rgb(SUCCESS).into()
                } else {
                    rgb(DANGER).into()
                }
            })
            .unwrap_or_else(|| rgb(MUTED).into());
        div()
            .id("settings-panel")
            .w_full()
            .h_full()
            .overflow_y_scroll()
            .bg(rgb(SURFACE))
            .border_l_1()
            .border_color(rgb(BORDER))
            .shadow(glass_shadow())
            .p_4()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_lg()
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
                        Button::new("close-environment")
                            .ghost()
                            .small()
                            .size(px(COMPACT_CONTROL_HEIGHT))
                            .icon(IconName::Close)
                            .tooltip("关闭环境面板")
                            .on_click(
                                cx.listener(|app, _, window, cx| app.toggle_settings(window, cx)),
                            ),
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
                            .rounded(px(10.))
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
                                        div().flex_1().min_w_0().whitespace_normal().child(
                                            probe
                                                .map(|probe| probe.message.clone())
                                                .unwrap_or_else(|| "尚未探测".into()),
                                        ),
                                    ),
                            )
                            .when_some(
                                probe.and_then(|probe| probe.version.clone()),
                                |element, version| element.child(label_value("版本", version)),
                            ),
                    ),
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
                                app.settings_from = if app.settings_open { 1. } else { 0. };
                                app.settings_changed = Instant::now();
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
            )
    }
}
