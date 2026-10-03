use super::*;

impl NexusView {
    pub(super) fn render_appearance_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                                            .child(
                                                theme_preview(theme)
                                                    .debug_selector(move || {
                                                        format!("appearance-theme-preview-{id}")
                                                    })
                                                    .border_color(rgb(if selected {
                                                        colors.accent
                                                    } else {
                                                        colors.border
                                                    })),
                                            )
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
}

fn theme_preview(theme: ThemePreference) -> gpui::Div {
    let sidebar = Palette::for_dark(theme == ThemePreference::Dark);
    let content = Palette::for_dark(theme != ThemePreference::Light);
    let radius = px(10.);
    let border_width = px(2.);
    // GPUI clips children to rectangles, so the backgrounds need their own inner radii.
    let inner_radius = radius - border_width;
    div()
        .w_full()
        .h(px(124.))
        .rounded(radius)
        .border(border_width)
        .overflow_hidden()
        .flex()
        .child(
            div()
                .w(px(46.))
                .h_full()
                .flex_none()
                .rounded_l(inner_radius)
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
                .rounded_r(inner_radius)
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
