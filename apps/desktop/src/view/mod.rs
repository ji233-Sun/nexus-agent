mod components;
mod settings;
mod sidebar;
pub(crate) mod theme;
mod timeline;

use crate::{model::history::HistoryMessage, presenter::Presenter};
use components::*;
use gpui::{
    Animation, AnimationExt as _, AnyElement, AppContext as _, Context, Corner, ElementId, Entity,
    Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Timer, Window, div, ease_out_quint,
    prelude::FluentBuilder as _, pulsating_between, px, relative, rgb, rgba,
};
use gpui_component::{
    Disableable as _, Sizable as _, box_shadow,
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    text::TextView,
};
use nexus_domain::{
    ClaudeModel, HarnessKind, Message, MessageKind, MessageRole, Project, RunStatus, ThinkingEffort,
};
use std::time::Duration;
use theme::*;
use uuid::Uuid;

pub(crate) struct NexusView {
    presenter: Presenter,
    prompt_input: Entity<InputState>,
    executable_input: Entity<InputState>,
}

impl NexusView {
    pub(crate) fn new(presenter: Presenter, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prompt_input = cx.new(|cx| {
            InputState::new(window, cx)
                .auto_grow(3, 8)
                .placeholder("描述你希望 Agent 完成的工作…")
        });
        let executable_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(presenter.model().executable.clone())
                .placeholder("命令名或完整路径")
        });
        let view = Self {
            presenter,
            prompt_input,
            executable_input,
        };
        view.start_event_pump(cx);
        view
    }

    fn start_event_pump(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(33)).await;
                let Some(this) = this.upgrade() else { break };
                if this
                    .update(cx, |app, cx| {
                        if app.presenter.drain_events() {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
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

    fn new_task(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.presenter.new_task();
        cx.notify();
    }

    fn select_project(&mut self, project: Project) {
        self.presenter.select_project(project);
    }

    fn select_task(&mut self, task_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        self.presenter.select_task(task_id);
        self.sync_executable(window, cx);
        cx.notify();
    }

    fn select_codex_thread(&mut self, thread_id: String, cx: &mut Context<Self>) {
        self.presenter.select_codex_thread(thread_id);
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
        let prompt = self.prompt_input.read(cx).value().to_string();
        let executable = self.executable_input.read(cx).value().to_string();
        if self.presenter.submit(&prompt, &executable) {
            self.prompt_input
                .update(cx, |input, cx| input.set_value("", window, cx));
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
            .label(format!("{selected}  ⌄"))
            .disabled(model.active_run.is_some())
            .when(compact, |button| button.ghost().small())
            .when(!compact, |button| button.outline().w_full())
            .dropdown_menu_with_anchor(Corner::TopLeft, move |menu, _, _| {
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
            .label(format!("{label}  ⌄"))
            .disabled(model.selected_harness == HarnessKind::Codex || model.active_run.is_some())
            .dropdown_menu_with_anchor(Corner::TopLeft, move |menu, _, _| {
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
            .label(format!("{selected}  ⌄"))
            .disabled(model.active_run.is_some())
            .dropdown_menu_with_anchor(Corner::TopLeft, move |menu, _, _| {
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
        let model = self.presenter.model();
        let probe = model.selected_probe();
        let can_submit = model.can_submit();
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
            .size_full()
            .bg(rgba(0x101211ec))
            .text_color(rgb(TEXT))
            .text_sm()
            .flex()
            .child(self.render_sidebar(cx))
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .bg(rgba(0x101211f2))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(58.))
                            .flex_none()
                            .bg(rgba(0x151716d9))
                            .border_b_1()
                            .border_color(rgba(0xffffff10))
                            .px_5()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(div().text_color(rgb(MUTED)).child("▱"))
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
                                        model.active_run.is_some(),
                                    ))
                                    .child(
                                        div()
                                            .max_w(px(260.))
                                            .truncate()
                                            .child(model.status.clone()),
                                    ),
                            ),
                    )
                    .child(self.render_timeline(window, cx))
                    .child(
                        div()
                            .flex_none()
                            .bg(rgba(0x10121100))
                            .px_6()
                            .pt_2()
                            .pb_5()
                            .child(
                                div()
                                    .relative()
                                    .w_full()
                                    .max_w(px(720.))
                                    .mx_auto()
                                    .rounded(px(18.))
                                    .bg(rgba(0x292c2add))
                                    .border_1()
                                    .border_color(rgba(0xffffff14))
                                    .shadow(glass_shadow())
                                    .p_2()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        Input::new(&self.prompt_input)
                                            .h(px(72.))
                                            .appearance(false)
                                            .focus_bordered(false),
                                    )
                                    .child(
                                        div()
                                            .h(px(38.))
                                            .px_1()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .flex()
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
                                            .child(
                                                Button::new("submit")
                                                    .primary()
                                                    .rounded(px(18.))
                                                    .size(px(36.))
                                                    .p_0()
                                                    .child(button_label(
                                                        if model.active_run.is_some() {
                                                            "…"
                                                        } else {
                                                            "↑"
                                                        },
                                                        SURFACE,
                                                    ))
                                                    .when(!can_submit, |button| {
                                                        button.opacity(0.42)
                                                    })
                                                    .disabled(!can_submit)
                                                    .on_click(cx.listener(Self::submit)),
                                            ),
                                    )
                                    .with_animation(
                                        "composer-enter",
                                        Animation::new(Duration::from_millis(280))
                                            .with_easing(ease_out_quint()),
                                        |element, delta| {
                                            element.opacity(delta).top(px(10.) - delta * px(10.))
                                        },
                                    ),
                            ),
                    ),
            )
            .child(self.render_settings(cx))
    }
}
