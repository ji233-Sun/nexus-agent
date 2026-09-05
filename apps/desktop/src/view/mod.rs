mod components;
mod settings;
mod sidebar;
pub(crate) mod theme;
mod timeline;

use crate::{model::history::HistoryMessage, presenter::Presenter};
use components::*;
use gpui::{
    Anchor, Animation, AnimationExt as _, AnyElement, AppContext as _, Context, ElementId, Entity,
    FocusHandle, Focusable as _, Hsla, InteractiveElement as _, IntoElement, KeyBinding,
    ParentElement as _, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, ease_out_quint, prelude::FluentBuilder as _, pulsating_between, px,
    relative, rgb, rgba,
};
use gpui_kit as gpui;
use gpui_kit::component::{
    Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
    alert::Alert,
    box_shadow,
    button::{Button, ButtonVariants as _},
    input::{Enter, Input, InputEvent, InputState, Textarea, TextareaState},
    menu::{DropdownMenu as _, PopupMenuItem},
    switch::Switch,
    text::{TextView, TextViewStyle},
};
use nexus_domain::{
    ClaudeModel, HarnessKind, Message, MessageKind, MessageRole, Project, RunStatus, ThinkingEffort,
};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
use theme::*;
use uuid::Uuid;

gpui::actions!(nexus_view, [SearchSessions, NewTask, ToggleEnvironment]);

pub(crate) struct NexusView {
    presenter: Presenter,
    prompt_input: Entity<TextareaState>,
    executable_input: Entity<InputState>,
    search_input: Entity<InputState>,
    focus_handle: FocusHandle,
    timeline_scroll: ScrollHandle,
    expanded_messages: HashSet<ElementId>,
    collapsed_projects: HashSet<Uuid>,
    codex_history_open: bool,
    settings_open: bool,
    settings_from: f32,
    settings_changed: Instant,
    reduced_motion: bool,
}

impl NexusView {
    pub(crate) fn new(presenter: Presenter, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prompt_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .placeholder("描述一个目标，让 Agent 开始工作…")
        });
        let executable_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(presenter.model().executable.clone())
                .placeholder("命令名或完整路径")
        });
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索任务与历史…"));
        cx.subscribe(&prompt_input, |_, _, event: &InputEvent, cx| {
            if matches!(
                event,
                InputEvent::Change | InputEvent::Focus | InputEvent::Blur
            ) {
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(&search_input, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        cx.bind_keys([
            KeyBinding::new("secondary-k", SearchSessions, Some("Nexus")),
            KeyBinding::new("secondary-n", NewTask, Some("Nexus")),
            KeyBinding::new("secondary-,", ToggleEnvironment, Some("Nexus")),
        ]);
        let view = Self {
            presenter,
            prompt_input,
            executable_input,
            search_input,
            focus_handle: cx.focus_handle(),
            timeline_scroll: ScrollHandle::new(),
            expanded_messages: HashSet::new(),
            collapsed_projects: HashSet::new(),
            codex_history_open: false,
            settings_open: false,
            settings_from: 0.,
            settings_changed: Instant::now(),
            reduced_motion: false,
        };
        view.focus_handle.focus(window, cx);
        view.start_event_pump(cx);
        view
    }

    fn start_event_pump(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                smol::Timer::after(Duration::from_millis(33)).await;
                let Some(this) = this.upgrade() else { break };
                this.update(cx, |app, cx| {
                    let follow_latest = app.timeline_scroll.max_offset().y
                        + app.timeline_scroll.offset().y
                        <= px(48.);
                    if app.presenter.drain_events() {
                        if follow_latest {
                            app.timeline_scroll.scroll_to_bottom();
                        }
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    fn choose_project(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let Some(folder) = rfd::AsyncFileDialog::new().pick_folder().await else {
                return;
            };
            let _ = this.update(cx, |view, cx| {
                view.presenter.open_project(folder.path());
                cx.notify();
            });
        })
        .detach();
    }

    fn new_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.presenter.model().active_run.is_some()
            || self.presenter.model().selected_project.is_none()
        {
            return;
        }
        self.presenter.new_task();
        self.expanded_messages.clear();
        self.focus_prompt(window, cx);
        cx.notify();
    }

    fn select_project(&mut self, project: Project) {
        self.presenter.select_project(project);
        self.expanded_messages.clear();
        self.timeline_scroll.scroll_to_bottom();
    }

    fn select_task(&mut self, task_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        self.presenter.select_task(task_id);
        self.expanded_messages.clear();
        self.timeline_scroll.scroll_to_bottom();
        self.sync_executable(window, cx);
        cx.notify();
    }

    fn select_codex_thread(&mut self, thread_id: String, cx: &mut Context<Self>) {
        self.presenter.select_codex_thread(thread_id);
        self.expanded_messages.clear();
        self.timeline_scroll.scroll_to_bottom();
        cx.notify();
    }

    fn refresh_codex_history(
        &mut self,
        _: &gpui::ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.presenter.request_codex_history_refresh();
        cx.notify();
    }

    fn submit(&mut self, _: &gpui::ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.submit_prompt(window, cx);
    }

    fn submit_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = self.prompt_input.read(cx).value().to_string();
        if !can_send_prompt(self.presenter.model(), &prompt) {
            return;
        }
        let executable = self.executable_input.read(cx).value().to_string();
        if self.presenter.submit(&prompt, &executable) {
            self.prompt_input
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.expanded_messages.clear();
            self.timeline_scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn focus_prompt(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.presenter.model().selected_codex_thread.is_some() {
            self.focus_handle.focus(window, cx);
            return;
        }
        self.prompt_input
            .update(cx, |input, cx| input.focus(window, cx));
    }

    fn settings_progress(&self) -> f32 {
        let target = if self.settings_open { 1. } else { 0. };
        if self.reduced_motion {
            return target;
        }
        let elapsed = (self.settings_changed.elapsed().as_secs_f32() / 0.24).min(1.);
        self.settings_from + (target - self.settings_from) * (1. - (1. - elapsed).powi(5))
    }

    fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_from = self.settings_progress();
        self.settings_changed = Instant::now();
        self.settings_open = !self.settings_open;
        if !self.settings_open {
            self.focus_prompt(window, cx);
        }
        cx.notify();
    }

    fn cancel(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.presenter.cancel();
        cx.notify();
    }

    fn probe(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let executable = self.executable_input.read(cx).value().to_string();
        self.presenter.probe(&executable);
        cx.notify();
    }

    fn select_harness(
        &mut self,
        harness: HarnessKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let executable = self.executable_input.read(cx).value().to_string();
        if self.presenter.select_harness(harness, &executable) {
            self.sync_executable(window, cx);
        }
        cx.notify();
    }

    fn select_model(&mut self, model: ClaudeModel, cx: &mut Context<Self>) {
        self.presenter.select_model(model);
        cx.notify();
    }

    fn select_effort(&mut self, effort: ThinkingEffort, cx: &mut Context<Self>) {
        self.presenter.select_effort(effort);
        cx.notify();
    }

    fn sync_executable(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.executable_input.update(cx, |input, cx| {
            input.set_value(&self.presenter.model().executable, window, cx)
        });
    }

    fn harness_selector(
        &self,
        id: &'static str,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let model = self.presenter.model();
        let selected = model.selected_harness;
        let app = cx.entity().clone();
        Button::new(id)
            .icon(IconName::Bot)
            .label(selected.to_string())
            .dropdown_caret(true)
            .disabled(model.active_run.is_some())
            .small()
            .when(compact, |button| {
                button.ghost().h(px(COMPACT_CONTROL_HEIGHT))
            })
            .when(!compact, |button| {
                button.outline().w_full().h(px(CONTROL_HEIGHT))
            })
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                HarnessKind::ALL.into_iter().fold(
                    menu.min_w(if compact { px(160.) } else { px(220.) }),
                    |menu, harness| {
                        let app = app.clone();
                        menu.item(
                            PopupMenuItem::new(harness.to_string())
                                .checked(harness == selected)
                                .on_click(move |_, window, cx| {
                                    app.update(cx, |app, cx| {
                                        app.select_harness(harness, window, cx)
                                    });
                                }),
                        )
                    },
                )
            })
    }

    fn model_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let selected = model.claude_model;
        let app = cx.entity().clone();
        let label = if model.selected_harness == HarnessKind::Claude {
            selected.to_string()
        } else {
            "CLI 默认模型".into()
        };
        Button::new("composer-model")
            .ghost()
            .small()
            .h(px(COMPACT_CONTROL_HEIGHT))
            .label(label)
            .dropdown_caret(true)
            .disabled(model.selected_harness == HarnessKind::Codex || model.active_run.is_some())
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                ClaudeModel::ALL
                    .into_iter()
                    .fold(menu.min_w(px(160.)), |menu, model| {
                        let app = app.clone();
                        menu.item(
                            PopupMenuItem::new(model.to_string())
                                .checked(model == selected)
                                .on_click(move |_, _, cx| {
                                    app.update(cx, |app, cx| app.select_model(model, cx));
                                }),
                        )
                    })
            })
    }

    fn effort_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let selected = model.effort;
        let app = cx.entity().clone();
        Button::new("composer-effort")
            .ghost()
            .small()
            .h(px(COMPACT_CONTROL_HEIGHT))
            .icon(IconName::Cpu)
            .label(selected.to_string())
            .dropdown_caret(true)
            .disabled(model.active_run.is_some())
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                ThinkingEffort::ALL
                    .into_iter()
                    .fold(menu.min_w(px(140.)), |menu, effort| {
                        let app = app.clone();
                        menu.item(
                            PopupMenuItem::new(effort.to_string())
                                .checked(effort == selected)
                                .on_click(move |_, _, cx| {
                                    app.update(cx, |app, cx| app.select_effort(effort, cx));
                                }),
                        )
                    })
            })
    }
}

impl Render for NexusView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings_progress = self.settings_progress();
        if !self.reduced_motion
            && self.settings_changed.elapsed() < Duration::from_millis(240)
            && (settings_progress - if self.settings_open { 1. } else { 0. }).abs() > f32::EPSILON
        {
            window.request_animation_frame();
        }
        let model = self.presenter.model();
        let probe = model.selected_probe();
        let history = model.selected_codex_thread.is_some();
        let can_submit = can_send_prompt(model, &self.prompt_input.read(cx).value());
        let prompt_focused = self
            .prompt_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let composer_hint = if history {
            "这是只读历史。选择项目并新建任务后即可开始。"
        } else if model.selected_project.is_none() {
            "先选择本地项目，再描述你希望完成的工作。"
        } else if model.active_run.is_some() {
            "Agent 正在执行 · 可以提前起草下一项任务"
        } else if !model.can_submit() {
            "Agent 尚未就绪 · 打开环境面板检查探测和登录状态"
        } else if cfg!(target_os = "macos") {
            "⌘ Enter 发送新任务 · Enter 换行"
        } else {
            "Ctrl Enter 发送新任务 · Enter 换行"
        };
        let header_status_color = if model.active_run.is_some() {
            rgb(ACCENT).into()
        } else {
            probe
                .map(|probe| {
                    if probe.available && probe.authenticated {
                        rgb(SUCCESS).into()
                    } else {
                        rgb(DANGER).into()
                    }
                })
                .unwrap_or_else(|| rgb(MUTED).into())
        };
        let header_title = model
            .selected_codex_thread
            .as_ref()
            .and_then(|thread_id| {
                model
                    .codex_threads
                    .iter()
                    .find(|thread| &thread.id == thread_id)
            })
            .map(|thread| format!("Codex 历史 · {}", thread.title))
            .or_else(|| {
                model
                    .selected_project
                    .as_ref()
                    .map(|project| project.display_name.clone())
            })
            .unwrap_or_else(|| "未选择项目".into());
        let header_context = if model.selected_codex_thread.is_some() {
            Some("Codex 原有会话")
        } else if model.selected_task.is_some() {
            Some("任务时间线")
        } else {
            None
        };
        div()
            .key_context("Nexus")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|app, _: &SearchSessions, window, cx| {
                app.search_input
                    .update(cx, |input, cx| input.focus(window, cx));
            }))
            .on_action(cx.listener(|app, _: &NewTask, window, cx| {
                app.new_task(window, cx);
            }))
            .on_action(cx.listener(|app, _: &ToggleEnvironment, window, cx| {
                app.toggle_settings(window, cx);
            }))
            .capture_action(cx.listener(|app, action: &Enter, window, cx| {
                if action.secondary
                    && app
                        .prompt_input
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                {
                    app.submit_prompt(window, cx);
                    cx.stop_propagation();
                }
            }))
            .size_full()
            .relative()
            .bg(rgb(CANVAS))
            .text_color(rgb(TEXT))
            .text_sm()
            .flex()
            .child(self.render_sidebar(cx))
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .bg(rgb(CANVAS))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(HEADER_HEIGHT))
                            .flex_none()
                            .bg(rgb(CANVAS))
                            .border_b_1()
                            .border_color(rgba(0xffffff10))
                            .px_6()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        Icon::new(if history {
                                            IconName::FileText
                                        } else {
                                            IconName::Folder
                                        })
                                        .text_color(rgb(MUTED)),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .child(header_title),
                                    )
                                    .when_some(header_context, |element, context| {
                                        element.child(
                                            div()
                                                .flex_none()
                                                .rounded_full()
                                                .bg(rgba(0xffffff0a))
                                                .border_1()
                                                .border_color(rgba(0xffffff0d))
                                                .px_2()
                                                .py(px(3.))
                                                .text_xs()
                                                .text_color(rgb(MUTED))
                                                .child(context),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .child(live_status_dot(
                                        header_status_color,
                                        model.active_run.is_some() && !self.reduced_motion,
                                    ))
                                    .child(
                                        div()
                                            .max_w(px(180.))
                                            .truncate()
                                            .child(model.status.clone()),
                                    )
                                    .child(
                                        Button::new("toggle-environment")
                                            .ghost()
                                            .small()
                                            .h(px(COMPACT_CONTROL_HEIGHT))
                                            .icon(IconName::PanelRight)
                                            .selected(self.settings_open)
                                            .label("环境")
                                            .tooltip(if cfg!(target_os = "macos") {
                                                "显示 / 隐藏环境 · ⌘ ,"
                                            } else {
                                                "显示 / 隐藏环境 · Ctrl ,"
                                            })
                                            .on_click(cx.listener(|app, _, window, cx| {
                                                app.toggle_settings(window, cx)
                                            })),
                                    ),
                            ),
                    )
                    .child(self.render_timeline(window, cx))
                    .child(
                        div()
                            .flex_none()
                            .bg(rgba(0x10121100))
                            .px_8()
                            .pt_3()
                            .pb_6()
                            .child(
                                div()
                                    .relative()
                                    .w_full()
                                    .max_w(px(816.))
                                    .mx_auto()
                                    .rounded(px(16.))
                                    .bg(rgb(SURFACE))
                                    .border_1()
                                    .border_color(rgba(0xffffff14))
                                    .when(prompt_focused, |element| {
                                        element.border_color(rgb(ACCENT))
                                    })
                                    .shadow(glass_shadow())
                                    .p_3()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        Textarea::new(&self.prompt_input)
                                            .disabled(history)
                                            .appearance(false)
                                            .bordered(false)
                                            .aria_label("任务描述"),
                                    )
                                    .child(
                                        div()
                                            .min_h(px(CONTROL_HEIGHT))
                                            .mt_2()
                                            .pt_2()
                                            .border_t_1()
                                            .border_color(rgb(BORDER))
                                            .flex()
                                            .flex_wrap()
                                            .gap_2()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_wrap()
                                                    .items_center()
                                                    .gap_1()
                                                    .child(self.harness_selector(
                                                        "composer-harness",
                                                        true,
                                                        cx,
                                                    ))
                                                    .child(self.model_selector(cx))
                                                    .child(self.effort_selector(cx)),
                                            )
                                            .when(model.active_run.is_some(), |element| {
                                                element.child(
                                                    Button::new("composer-cancel")
                                                        .danger()
                                                        .outline()
                                                        .small()
                                                        .h(px(COMPACT_CONTROL_HEIGHT))
                                                        .icon(IconName::Pause)
                                                        .label("停止")
                                                        .tooltip("停止当前运行，保留已有输出")
                                                        .on_click(cx.listener(Self::cancel)),
                                                )
                                            })
                                            .when(model.active_run.is_none(), |element| {
                                                element.child(
                                                    Button::new("submit")
                                                        .primary()
                                                        .small()
                                                        .size(px(COMPACT_CONTROL_HEIGHT))
                                                        .p_0()
                                                        .icon(IconName::ArrowUp)
                                                        .accessibility_label("发送任务")
                                                        .tooltip(composer_hint)
                                                        .when(!can_submit, |button| {
                                                            button.opacity(0.42)
                                                        })
                                                        .disabled(!can_submit)
                                                        .on_click(cx.listener(Self::submit)),
                                                )
                                            }),
                                    )
                                    .map(|element| {
                                        entrance(element, "composer-enter", !self.reduced_motion)
                                    }),
                            )
                            .child(
                                div()
                                    .max_w(px(816.))
                                    .mx_auto()
                                    .mt_3()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div().text_xs().text_color(rgb(MUTED)).child(composer_hint),
                                    )
                                    .when(
                                        !history
                                            && model.selected_project.is_some()
                                            && !model.can_submit()
                                            && model.active_run.is_none(),
                                        |element| {
                                            element.child(
                                                Button::new("setup-agent")
                                                    .ghost()
                                                    .small()
                                                    .h(px(COMPACT_CONTROL_HEIGHT))
                                                    .label("检查环境")
                                                    .on_click(cx.listener(|app, _, window, cx| {
                                                        if !app.settings_open {
                                                            app.toggle_settings(window, cx);
                                                        }
                                                    })),
                                            )
                                        },
                                    ),
                            ),
                    ),
            )
            .when(settings_progress > 0., |element| {
                element.child(
                    div()
                        .absolute()
                        .top(px(HEADER_HEIGHT))
                        .bottom_0()
                        .right(px(-320. * (1. - settings_progress)))
                        .w(px(320.))
                        .flex_none()
                        .overflow_hidden()
                        .opacity(settings_progress)
                        .child(self.render_settings(cx)),
                )
            })
    }
}
