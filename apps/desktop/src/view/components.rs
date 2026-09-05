use super::*;

pub(super) fn brand_mark(size: f32) -> impl IntoElement {
    div()
        .size(px(size))
        .flex_none()
        .rounded(px(size * 0.3))
        .bg(rgb(SELECTED))
        .border_1()
        .border_color(rgba(0x619cff40))
        .flex()
        .items_center()
        .justify_center()
        .child(
            Icon::new(IconName::Asterisk)
                .size(px(size * 0.55))
                .text_color(rgb(ACCENT)),
        )
}

pub(super) fn entrance(element: gpui::Div, id: impl Into<ElementId>, animated: bool) -> AnyElement {
    if !animated {
        return element.into_any_element();
    }
    element
        .relative()
        .with_animation(
            id,
            Animation::new(Duration::from_millis(240)).with_easing(ease_out_quint()),
            |element, delta| element.opacity(delta).top(px(8. * (1. - delta))),
        )
        .into_any_element()
}

pub(super) fn matches_search(text: &str, query: &str) -> bool {
    text.to_lowercase().contains(&query.trim().to_lowercase())
}

pub(super) fn can_send_prompt(model: &crate::model::AppModel, prompt: &str) -> bool {
    model.can_submit() && model.selected_codex_thread.is_none() && !prompt.trim().is_empty()
}

impl NexusView {
    pub(super) fn render_message(
        &self,
        message: &Message,
        window: &mut Window,
        cx: &mut Context<NexusView>,
    ) -> AnyElement {
        self.message_card(
            message.id,
            message.role,
            &message.content,
            message.kind,
            window,
            cx,
        )
    }

    pub(super) fn render_history_message(
        &self,
        index: usize,
        message: &HistoryMessage,
        window: &mut Window,
        cx: &mut Context<NexusView>,
    ) -> AnyElement {
        self.message_card(
            SharedString::from(format!(
                "history-{}-{index}",
                self.presenter
                    .model()
                    .selected_codex_thread
                    .as_deref()
                    .unwrap_or_default()
            )),
            message.role,
            &message.content,
            message.kind,
            window,
            cx,
        )
    }

    pub(super) fn message_card(
        &self,
        id: impl Into<ElementId>,
        role: MessageRole,
        content: &str,
        kind: MessageKind,
        _window: &mut Window,
        cx: &mut Context<NexusView>,
    ) -> AnyElement {
        let id = id.into();
        let expanded = self.expanded_messages.contains(&id);
        let animated = !self.reduced_motion;
        let collapsible = matches!(kind, MessageKind::ToolCall | MessageKind::ToolResult)
            && (content.lines().count() > 6 || content.chars().count() > 400);
        let toggle_id = id.clone();
        let label = match role {
            MessageRole::User => "You",
            MessageRole::Assistant => "Agent",
            MessageRole::Tool => "Tool",
            MessageRole::System => "System",
        };
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
                            .bg(rgb(RECESSED))
                            .border_1()
                            .border_color(rgba(0xffffff10))
                            .px_4()
                            .py_3()
                    })
                    .when(!is_user, |element| element.w_full())
                    .when(!is_user && is_panel, |element| {
                        element
                            .rounded(px(14.))
                            .bg(rgb(SURFACE))
                            .border_1()
                            .border_color(rgba(0xffffff0d))
                            .shadow(surface_border_shadow())
                            .p_4()
                    })
                    .when(!is_user && !is_panel, |element| element.px_1().py_1())
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
                    .when(collapsible, |element| {
                        element.child(
                            Button::new((id.clone(), "disclosure"))
                                .ghost()
                                .small()
                                .icon(if expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .label(if expanded {
                                    "收起工具输出"
                                } else {
                                    "展开完整工具输出"
                                })
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    if !app.expanded_messages.remove(&toggle_id) {
                                        app.expanded_messages.insert(toggle_id.clone());
                                    }
                                    cx.notify();
                                })),
                        )
                    })
                    .child(if kind == MessageKind::Text {
                        TextView::markdown(id.clone(), content.to_owned())
                            .text_size(px(15.))
                            .line_height(relative(1.45))
                            .style(
                                TextViewStyle::default()
                                    .paragraph_gap(gpui::rems(0.5))
                                    .heading_font_size(|level, _| {
                                        px(match level {
                                            1 => 24.,
                                            2 => 20.,
                                            3 => 17.,
                                            4 => 16.,
                                            _ => 15.,
                                        })
                                    })
                                    .code_block(
                                        gpui::StyleRefinement::default()
                                            .font_family(MONO_FONT)
                                            .text_size(px(14.))
                                            .line_height(relative(1.4)),
                                    )
                                    .table_head(
                                        gpui::StyleRefinement::default()
                                            .bg(rgb(SURFACE))
                                            .text_color(rgb(TEXT_SECONDARY)),
                                    ),
                            )
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
                            .when(collapsible && !expanded, |element| element.line_clamp(3))
                            .when(
                                matches!(kind, MessageKind::ToolCall | MessageKind::ToolResult),
                                |element| element.font_family(MONO_FONT),
                            )
                            .child(content.to_owned())
                            .into_any_element()
                    }),
            )
            .map(|element| entrance(element, id, animated))
    }
}

pub(super) fn message_indicator(kind: MessageKind) -> Hsla {
    match kind {
        MessageKind::Error => rgb(DANGER).into(),
        MessageKind::ToolCall | MessageKind::ToolResult => rgb(TOOL).into(),
        _ => rgb(MUTED).into(),
    }
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

pub(super) fn glass_shadow() -> Vec<gpui::BoxShadow> {
    vec![
        box_shadow(0., 0., 0., 1., rgba(0xffffff0a).into()),
        box_shadow(0., 2., 8., -3., rgba(0x00000080).into()),
        box_shadow(0., 18., 42., -18., rgba(0x000000c0).into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{infrastructure::storage::Storage, model::AppModel};
    use nexus_protocol::HarnessProbe;
    use std::path::Path;

    #[test]
    fn search_accepts_case_insensitive_titles_chinese_and_surrounding_spaces() {
        assert!(matches_search("Fix Runner output", " RUNNER "));
        assert!(matches_search("优化会话交互", "会话"));
        assert!(matches_search("any session", "  "));
        assert!(!matches_search("Fix Runner output", "storage"));
    }

    #[test]
    fn composer_rejects_blank_prompts_read_only_history_and_busy_or_unready_agents() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Storage::open(Path::new(":memory:")).unwrap();
        let mut model = AppModel {
            selected_project: Some(storage.open_project(directory.path()).unwrap()),
            ..AppModel::default()
        };
        model.harnesses.insert(
            model.selected_harness,
            HarnessProbe {
                harness: model.selected_harness,
                available: true,
                authenticated: true,
                executable: "fake-agent".into(),
                version: None,
                message: "ready".into(),
            },
        );
        assert!(can_send_prompt(&model, "检查当前项目"));
        for prompt in ["", "  ", "\n\t", "\u{3000}"] {
            assert!(!can_send_prompt(&model, prompt));
        }
        model.selected_codex_thread = Some("read-only-thread".into());
        assert!(!can_send_prompt(&model, "检查当前项目"));
        model.selected_codex_thread = None;
        model.active_run = Some(Uuid::new_v4());
        assert!(!can_send_prompt(&model, "检查当前项目"));
        model.active_run = None;
        model.harnesses.clear();
        assert!(!can_send_prompt(&model, "检查当前项目"));
    }
}
