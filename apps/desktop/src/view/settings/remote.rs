use super::*;

impl NexusView {
    pub(super) fn render_remote_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

fn masked_token(token: &str) -> String {
    if token.chars().count() <= 10 {
        return "••••••••".into();
    }
    let prefix = token.chars().take(6).collect::<String>();
    let suffix = token.chars().rev().take(4).collect::<String>();
    format!("{prefix}••••{}", suffix.chars().rev().collect::<String>())
}
