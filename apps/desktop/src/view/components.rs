use super::*;
use gpui::{App, DismissEvent, RenderOnce};
use gpui_kit::{
    base::{PopoverState, Popup, Presence, Transition, transition},
    component::menu::PopupMenu,
};

fn control_transition(reduced_motion: bool) -> Transition {
    Transition::new(if reduced_motion {
        Duration::ZERO
    } else {
        Duration::from_millis(180)
    })
    .ease(ease_out_quint())
}

pub(super) fn disclosure_progress(
    id: impl Into<ElementId>,
    open: bool,
    reduced_motion: bool,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    transition(
        id.into(),
        if open { 1. } else { 0. },
        control_transition(reduced_motion),
        window,
        cx,
    )
}

type MenuBuilder =
    std::rc::Rc<dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu>;

#[derive(IntoElement)]
pub(super) struct AnimatedDropdown {
    id: ElementId,
    trigger: Button,
    reduced_motion: bool,
    builder: MenuBuilder,
}

struct DropdownState {
    popover: Entity<PopoverState>,
    menu: Option<Entity<PopupMenu>>,
    subscription: Option<gpui::Subscription>,
}

impl AnimatedDropdown {
    pub(super) fn new(
        id: impl Into<ElementId>,
        trigger: Button,
        reduced_motion: bool,
        builder: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            trigger,
            reduced_motion,
            builder: std::rc::Rc::new(builder),
        }
    }
}

impl RenderOnce for AnimatedDropdown {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state((self.id.clone(), "state"), cx, |_, cx| DropdownState {
            popover: cx.new(|cx| PopoverState::new(false, cx)),
            menu: None,
            subscription: None,
        });
        let popover = state.read(cx).popover.clone();
        let open = popover.read(cx).is_open();
        let presence = Presence::new((self.id.clone(), "presence"), open)
            .transition(control_transition(self.reduced_motion))
            .sample(window, cx);
        let progress = disclosure_progress(
            (self.id.clone(), "caret"),
            open,
            self.reduced_motion,
            window,
            cx,
        );
        let trigger_size = self.trigger.style().size.clone();
        let toggle = std::rc::Rc::new({
            let state = state.clone();
            move |window: &mut Window, cx: &mut App| {
                let popover = state.read(cx).popover.clone();
                if popover.read(cx).is_open() {
                    popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                } else {
                    popover.update(cx, |popover, cx| popover.show(window, cx));
                    // Rebuild on each open so checkmarks reflect the latest selection.
                    let menu = PopupMenu::build(window, cx, |menu, window, cx| {
                        (self.builder)(menu, window, cx)
                    });
                    menu.focus_handle(cx).focus(window, cx);
                    let subscription =
                        window.subscribe(&menu, cx, move |_, _: &DismissEvent, window, cx| {
                            popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                            window.refresh();
                        });
                    state.update(cx, |state, _| {
                        state.menu = Some(menu);
                        state.subscription = Some(subscription);
                    });
                }
                window.refresh();
            }
        });
        let trigger = self
            .trigger
            .selected(open)
            .dropdown_caret(false)
            .child(
                Icon::new(IconName::ChevronDown)
                    .size(px(14.))
                    .rotate(gpui::radians(-std::f32::consts::PI * progress)),
            )
            .on_click({
                let toggle = toggle.clone();
                move |event, window, cx| {
                    if event.is_keyboard() {
                        toggle(window, cx);
                    }
                }
            });
        let mut popup = Popup::new(self.id, trigger)
            .anchor(Anchor::TopLeft)
            .on_mouse_down(gpui::MouseButton::Left, {
                let popover = popover.clone();
                move |_, window, cx| {
                    cx.stop_propagation();
                    // Outside-dismiss runs in capture before this trigger's bubble handler.
                    if popover.read(cx).is_open() == open {
                        toggle(window, cx);
                    }
                }
            });
        popup.style().size = trigger_size;
        if presence.should_render() {
            let menu = state.read(cx).menu.clone();
            let focus_handle = popover.focus_handle(cx);
            popup = popup.content(
                div()
                    .id("animated-menu-surface")
                    .debug_selector(|| "animated-menu-surface".into())
                    .track_focus(&focus_handle)
                    .tab_group()
                    .relative()
                    .top(px(4. - 6. * (1. - presence.progress)))
                    .opacity(presence.progress)
                    .children(menu)
                    // Keep the exit frame visible, but prevent a second selection.
                    .when(!open, |surface| {
                        surface.child(
                            div()
                                .id("closing-menu-shield")
                                .absolute()
                                .inset_0()
                                .occlude()
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .on_mouse_up(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                }),
                        )
                    }),
            );
        } else {
            state.update(cx, |state, _| {
                state.menu = None;
                state.subscription = None;
            });
        }
        popup
    }
}

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
                            .text_size(px(16.))
                            .font_weight(gpui::FontWeight::NORMAL)
                            .line_height(relative(1.7))
                            .style(
                                TextViewStyle::default()
                                    .paragraph_gap(gpui::rems(16. / 14.))
                                    .heading_font_size(|level, _| {
                                        px(match level {
                                            1 => 22.,
                                            2 => 20.,
                                            3 => 18.,
                                            4 => 17.,
                                            _ => 16.,
                                        })
                                    })
                                    .code_block(
                                        gpui::StyleRefinement::default()
                                            .font_family(MONO_FONT)
                                            .text_size(px(14.))
                                            .line_height(relative(1.6))
                                            .p(px(16.))
                                            .bg(rgb(SURFACE))
                                            .border_1()
                                            .border_color(rgb(BORDER))
                                            .rounded(px(8.)),
                                    )
                                    .table_head(
                                        gpui::StyleRefinement::default()
                                            .bg(rgb(SURFACE))
                                            .text_color(rgb(TEXT_SECONDARY)),
                                    )
                                    .table_cell(
                                        gpui::StyleRefinement::default().px(px(12.)).py(px(8.)),
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

pub(super) fn run_status_color(status: RunStatus) -> Option<Hsla> {
    match status {
        RunStatus::Completed => None,
        RunStatus::Failed => Some(rgb(DANGER).into()),
        RunStatus::Running | RunStatus::Starting | RunStatus::Cancelling => {
            Some(rgb(ACCENT).into())
        }
        RunStatus::Cancelled | RunStatus::Interrupted => Some(rgb(MUTED).into()),
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

    struct DropdownHarness {
        reduced_motion: bool,
        disabled: bool,
        selections: usize,
        focus: FocusHandle,
    }

    impl Render for DropdownHarness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let app = cx.entity();
            div()
                .id("dropdown-harness")
                .tab_group()
                .p_4()
                .child(AnimatedDropdown::new(
                    "test-dropdown",
                    Button::new("test-trigger")
                        .debug_selector(|| "test-trigger".into())
                        .label("Select")
                        .disabled(self.disabled),
                    self.reduced_motion,
                    move |menu, _, _| {
                        let app = app.clone();
                        menu.item(PopupMenuItem::new("Option").on_click(move |_, _, cx| {
                            app.update(cx, |app, cx| {
                                app.selections += 1;
                                cx.notify();
                            });
                        }))
                    },
                ))
        }
    }

    fn dropdown_fixture(
        cx: &mut gpui::TestAppContext,
    ) -> (Entity<DropdownHarness>, &mut gpui::VisualTestContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            DropdownHarness {
                reduced_motion: false,
                disabled: false,
                selections: 0,
                focus,
            }
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        (view, cx)
    }

    fn dropdown_frame(cx: &mut gpui::VisualTestContext, millis: u64) {
        cx.executor().advance_clock(Duration::from_millis(millis));
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
    }

    #[gpui::test]
    fn dropdown_animates_each_open_and_exit_and_preserves_selection_and_focus(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = dropdown_fixture(cx);
        let trigger = cx.debug_bounds("test-trigger").unwrap().center();
        for expected in 1..=2 {
            cx.simulate_click(trigger, Default::default());
            dropdown_frame(cx, 0);
            let opening = cx.debug_bounds("animated-menu-surface").unwrap();
            dropdown_frame(cx, 60);
            let moving = cx.debug_bounds("animated-menu-surface").unwrap();
            assert!(moving.origin.y > opening.origin.y);
            dropdown_frame(cx, 180);
            let settled = cx.debug_bounds("animated-menu-surface").unwrap();
            assert!(settled.origin.y > moving.origin.y);
            cx.simulate_keystrokes("down enter");
            dropdown_frame(cx, 0);
            assert_eq!(view.read_with(cx, |view, _| view.selections), expected);
            assert!(cx.debug_bounds("animated-menu-surface").is_some());
            dropdown_frame(cx, 60);
            assert!(cx.debug_bounds("animated-menu-surface").unwrap().origin.y < settled.origin.y);
            dropdown_frame(cx, 180);
            assert!(cx.debug_bounds("animated-menu-surface").is_none());
            cx.update(|window, cx| assert!(view.read(cx).focus.is_focused(window)));
        }
    }

    #[gpui::test]
    fn dropdown_reverses_on_repeated_click_and_respects_reduced_motion_and_disabled(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = dropdown_fixture(cx);
        let trigger = cx.debug_bounds("test-trigger").unwrap().center();
        cx.simulate_click(trigger, Default::default());
        dropdown_frame(cx, 0);
        dropdown_frame(cx, 40);
        cx.simulate_click(trigger, Default::default());
        dropdown_frame(cx, 0);
        dropdown_frame(cx, 200);
        assert!(cx.debug_bounds("animated-menu-surface").is_none());

        view.update(cx, |view, cx| {
            view.reduced_motion = true;
            cx.notify();
        });
        dropdown_frame(cx, 0);
        cx.simulate_click(trigger, Default::default());
        dropdown_frame(cx, 0);
        assert!(cx.debug_bounds("animated-menu-surface").is_some());
        cx.simulate_keystrokes("escape");
        dropdown_frame(cx, 0);
        assert!(cx.debug_bounds("animated-menu-surface").is_none());
        cx.update(|window, cx| window.focus_next(cx));
        dropdown_frame(cx, 0);
        let keystroke = gpui::Keystroke::parse("enter").unwrap();
        cx.simulate_event(gpui::KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(gpui::KeyUpEvent { keystroke });
        dropdown_frame(cx, 0);
        assert!(cx.debug_bounds("animated-menu-surface").is_some());
        cx.simulate_click(gpui::point(px(400.), px(400.)), Default::default());
        dropdown_frame(cx, 0);
        assert!(cx.debug_bounds("animated-menu-surface").is_none());
        view.update(cx, |view, cx| {
            view.disabled = true;
            cx.notify();
        });
        dropdown_frame(cx, 0);
        cx.simulate_click(trigger, Default::default());
        dropdown_frame(cx, 0);
        assert!(cx.debug_bounds("animated-menu-surface").is_none());
    }

    #[test]
    fn completed_tasks_do_not_show_a_dot_that_looks_like_an_unread_badge() {
        assert!(run_status_color(RunStatus::Completed).is_none());
        for status in [
            RunStatus::Starting,
            RunStatus::Running,
            RunStatus::Cancelling,
        ] {
            assert_eq!(run_status_color(status), Some(rgb(ACCENT).into()));
        }
        assert_eq!(
            run_status_color(RunStatus::Failed),
            Some(rgb(DANGER).into())
        );
    }

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
