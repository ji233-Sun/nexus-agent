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
            .relative()
            .w(px(292.))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .bg(rgba(0x101211f2))
            .pt(px(66.))
            .px_3()
            .pb_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .rounded(px(18.))
                    .bg(rgba(0x2a2d2be3))
                    .border_1()
                    .border_color(rgba(0xffffff12))
                    .shadow(glass_shadow())
                    .p_4()
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Environment"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("当前项目的本地执行环境与 Agent"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .rounded(px(18.))
                    .bg(rgba(0x252826dc))
                    .border_1()
                    .border_color(rgba(0xffffff10))
                    .shadow(glass_shadow())
                    .p_4()
                    .child(section_label("AGENT"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("类型 · 点击选择"),
                    )
                    .child(self.harness_selector("settings-harness", false, cx))
                    .child(div().text_xs().text_color(rgb(MUTED)).child("可执行文件"))
                    .child(Input::new(&self.executable_input))
                    .child(
                        Button::new("probe")
                            .outline()
                            .small()
                            .w_full()
                            .child(button_label("重新探测", TEXT))
                            .disabled(model.active_run.is_some())
                            .on_click(cx.listener(Self::probe)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_2()
                            .text_xs()
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(status_dot(harness_color))
                            .child(
                                probe
                                    .map(|probe| probe.message.clone())
                                    .unwrap_or_else(|| "尚未探测".into()),
                            ),
                    )
                    .when_some(
                        probe.and_then(|probe| probe.version.clone()),
                        |element, version| element.child(label_value("版本", version)),
                    ),
            )
            .when_some(model.selected_project.as_ref(), |element, project| {
                element
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .rounded(px(18.))
                            .bg(rgba(0x252826dc))
                            .border_1()
                            .border_color(rgba(0xffffff10))
                            .shadow(glass_shadow())
                            .p_4()
                            .child(section_label("WORKSPACE"))
                            .child(label_value("工作目录", project.canonical_path.clone())),
                    )
                    .when(model.project_dirty, |element| {
                        element.child(
                            div()
                                .flex()
                                .items_start()
                                .gap_2()
                                .text_xs()
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(status_dot(rgb(WARNING).into()))
                                .child("目录存在未提交修改；Nexus 不会自动还原或提交。"),
                        )
                    })
            })
            .child(div().flex_1())
            .when(model.active_run.is_some(), |element| {
                element.child(
                    Button::new("cancel")
                        .danger()
                        .outline()
                        .w_full()
                        .child(button_label("取消运行", DANGER))
                        .on_click(cx.listener(Self::cancel)),
                )
            })
            .with_animation(
                "settings-panel-enter",
                Animation::new(Duration::from_millis(320)).with_easing(ease_out_quint()),
                |element, delta| element.opacity(delta).right(px(10.) - delta * px(10.)),
            )
    }
}
