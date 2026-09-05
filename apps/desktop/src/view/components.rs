use super::*;

pub(super) fn render_message(
    message: &Message,
    window: &mut Window,
    cx: &mut Context<NexusView>,
) -> AnyElement {
    let label = match message.role {
        MessageRole::User => "You",
        MessageRole::Assistant => "Agent",
        MessageRole::Tool => "Tool",
        MessageRole::System => "System",
    };
    message_card(
        message.id,
        message.role,
        label,
        &message.content,
        message.kind,
        window,
        cx,
    )
}

pub(super) fn render_history_message(
    index: usize,
    message: &HistoryMessage,
    window: &mut Window,
    cx: &mut Context<NexusView>,
) -> AnyElement {
    let label = match message.role {
        MessageRole::User => "You · Codex 历史",
        MessageRole::Assistant => "Codex · 历史",
        MessageRole::Tool => "Tool · Codex 历史",
        MessageRole::System => "System · Codex 历史",
    };
    message_card(
        ("history-message", index),
        message.role,
        label,
        &message.content,
        message.kind,
        window,
        cx,
    )
}

pub(super) fn message_indicator(kind: MessageKind) -> Hsla {
    match kind {
        MessageKind::Error => rgb(DANGER).into(),
        MessageKind::ToolCall | MessageKind::ToolResult => rgb(TOOL).into(),
        _ => rgb(MUTED).into(),
    }
}

pub(super) fn message_card(
    id: impl Into<ElementId>,
    role: MessageRole,
    label: &str,
    content: &str,
    kind: MessageKind,
    window: &mut Window,
    cx: &mut Context<NexusView>,
) -> AnyElement {
    let is_user = role == MessageRole::User;
    let is_panel = matches!(
        kind,
        MessageKind::ToolCall | MessageKind::ToolResult | MessageKind::Error
    );
    let show_label = !is_user && (role != MessageRole::Assistant || is_panel);
    div()
        .w_full()
        .flex()
        .when(is_user, |element| element.justify_end())
        .child(
            div()
                .when(is_user, |element| {
                    element
                        .max_w(px(600.))
                        .rounded(px(18.))
                        .bg(rgba(0xffffff11))
                        .border_1()
                        .border_color(rgba(0xffffff10))
                        .px_4()
                        .py_3()
                })
                .when(!is_user, |element| element.w_full())
                .when(!is_user && is_panel, |element| {
                    element
                        .rounded(px(14.))
                        .bg(rgba(0x252826b8))
                        .border_1()
                        .border_color(rgba(0xffffff0d))
                        .shadow(surface_border_shadow())
                        .p_4()
                })
                .when(!is_user && !is_panel, |element| element.px_1().py_2())
                .when(show_label, |element| {
                    element.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .mb_2()
                            .child(status_dot(message_indicator(kind)))
                            .child(label.to_owned()),
                    )
                })
                .child(if kind == MessageKind::Text {
                    TextView::markdown(id, content.to_owned(), window, cx)
                        .selectable(true)
                        .into_any_element()
                } else {
                    div()
                        .text_sm()
                        .text_color(if kind == MessageKind::Status {
                            rgb(TEXT_SECONDARY)
                        } else {
                            rgb(TEXT)
                        })
                        .whitespace_normal()
                        .line_height(relative(1.55))
                        .when(
                            matches!(kind, MessageKind::ToolCall | MessageKind::ToolResult),
                            |element| element.font_family(MONO_FONT),
                        )
                        .child(content.to_owned())
                        .into_any_element()
                }),
        )
        .into_any_element()
}

pub(super) fn label_value(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let value: SharedString = value.into();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(rgb(MUTED)).child(label))
        .child(
            div()
                .text_sm()
                .text_color(rgb(TEXT_SECONDARY))
                .whitespace_normal()
                .child(value),
        )
}

pub(super) fn section_label(label: impl Into<SharedString>) -> impl IntoElement {
    div()
        .text_xs()
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(rgb(MUTED))
        .child(label.into())
}

pub(super) fn button_label(label: impl Into<SharedString>, color: u32) -> impl IntoElement {
    div().text_color(rgb(color)).child(label.into())
}

pub(super) fn status_dot(color: Hsla) -> gpui::Div {
    div()
        .size(px(8.))
        .mt(px(3.))
        .flex_none()
        .rounded_full()
        .bg(color)
}

pub(super) fn live_status_dot(color: Hsla, animated: bool) -> gpui::AnyElement {
    let dot = status_dot(color);
    if animated {
        dot.with_animation(
            "active-run-pulse",
            Animation::new(Duration::from_millis(1_800))
                .repeat()
                .with_easing(pulsating_between(0.45, 1.)),
            |element, opacity| element.opacity(opacity),
        )
        .into_any_element()
    } else {
        dot.into_any_element()
    }
}

pub(super) fn selection_indicator(id: SharedString) -> impl IntoElement {
    div()
        .absolute()
        .left(px(3.))
        .top(px(9.))
        .bottom(px(9.))
        .w(px(2.))
        .rounded_full()
        .bg(rgb(ACCENT))
        .shadow(vec![box_shadow(0., 0., 8., 0., rgba(0x9b7cff80).into())])
        .with_animation(
            id,
            Animation::new(Duration::from_millis(180)).with_easing(ease_out_quint()),
            |element, delta| element.opacity(delta).left(px(1.) + delta * px(2.)),
        )
}

pub(super) fn run_status_color(status: RunStatus) -> Hsla {
    match status {
        RunStatus::Completed => rgb(SUCCESS).into(),
        RunStatus::Failed => rgb(DANGER).into(),
        RunStatus::Running | RunStatus::Starting | RunStatus::Cancelling => rgb(ACCENT).into(),
        _ => rgb(MUTED).into(),
    }
}

pub(super) fn surface_border_shadow() -> Vec<gpui::BoxShadow> {
    vec![box_shadow(0., 0., 0., 1., rgba(0xffffff0d).into())]
}

pub(super) fn selection_shadow() -> Vec<gpui::BoxShadow> {
    vec![
        box_shadow(0., 0., 0., 1., rgba(0xffffff09).into()),
        box_shadow(0., 5., 14., -10., rgba(0x000000a0).into()),
    ]
}

pub(super) fn glass_shadow() -> Vec<gpui::BoxShadow> {
    vec![
        box_shadow(0., 0., 0., 1., rgba(0xffffff0a).into()),
        box_shadow(0., 2., 8., -3., rgba(0x00000080).into()),
        box_shadow(0., 18., 42., -18., rgba(0x000000c0).into()),
    ]
}
