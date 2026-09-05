mod components;
mod pane;
mod settings;
mod sidebar;
pub(crate) mod theme;
mod timeline;

use crate::{
    model::history::HistoryMessage,
    presenter::{Presenter, ProviderProfileDraft},
};
use components::*;
use gpui::{
    Anchor, Animation, AnimationExt as _, AnyElement, AppContext as _, ClipboardItem, Context,
    ElementId, Entity, FocusHandle, Focusable as _, Hsla, InteractiveElement as _, IntoElement,
    KeyBinding, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, ease_out_quint,
    prelude::FluentBuilder as _, pulsating_between, px, relative, rgb, rgba,
};
use gpui_kit as gpui;
use gpui_kit::component::{
    Disableable as _, Icon, IconName, InteractiveElementExt as _, Selectable as _, Sizable as _,
    alert::Alert,
    box_shadow,
    button::{Button, ButtonVariants as _},
    input::{Enter, Input, InputEvent, InputState, Textarea, TextareaState},
    menu::PopupMenuItem,
    switch::Switch,
    text::{TextView, TextViewStyle},
};
use nexus_domain::{
    ClaudeModel, HarnessKind, Message, MessageKind, MessageRole, Project, ProviderProfile,
    RunStatus, ThinkingEffort,
};
use pane::{PaneKind, WorkspacePane};
use settings::SettingsSection;
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
use theme::*;
use uuid::Uuid;

gpui::actions!(nexus_view, [SearchSessions, NewTask, ToggleSettings]);

pub(crate) struct NexusView {
    presenter: Presenter,
    prompt_input: Entity<TextareaState>,
    executable_input: Entity<InputState>,
    provider_name_input: Entity<InputState>,
    provider_api_key_env_input: Entity<InputState>,
    provider_api_key_input: Entity<InputState>,
    provider_base_url_env_input: Entity<InputState>,
    provider_base_url_input: Entity<InputState>,
    provider_model_input: Entity<InputState>,
    search_input: Entity<InputState>,
    focus_handle: FocusHandle,
    timeline_scroll: ScrollHandle,
    sidebar_scroll: ScrollHandle,
    settings_scroll: ScrollHandle,
    sidebar_pane: Entity<WorkspacePane>,
    timeline_pane: Entity<WorkspacePane>,
    settings_pane: Entity<WorkspacePane>,
    expanded_messages: HashSet<ElementId>,
    collapsed_projects: HashSet<Uuid>,
    codex_history_open: bool,
    codex_history_visible_count: usize,
    settings_open: bool,
    settings_section: SettingsSection,
    reduced_motion: bool,
    editing_provider_profile: Option<Uuid>,
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
        let ProviderProfileDraft {
            id: editing_provider_profile,
            name,
            api_key_env,
            api_key: _,
            base_url_env,
            base_url,
            model,
        } = profile_form_draft(
            presenter.model().selected_provider_profile(),
            presenter.model().selected_harness,
        );
        let provider_name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(name)
                .placeholder("例如 DeepSeek Production")
        });
        let provider_api_key_env_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(api_key_env)
                .placeholder("例如 DEEPSEEK_API_KEY")
        });
        let provider_api_key_input = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("新建时必填；编辑时留空保留")
        });
        let provider_base_url_env_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(base_url_env)
                .placeholder("可选，例如 OPENAI_BASE_URL")
        });
        let provider_base_url_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(base_url)
                .placeholder("可选，例如 https://api.example.com/v1")
        });
        let provider_model_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(model)
                .placeholder("可选，例如 deepseek/deepseek-v4-pro")
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
        cx.subscribe(&search_input, |app, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                app.codex_history_visible_count = sidebar::HISTORY_PAGE_SIZE;
            }
            cx.notify();
        })
        .detach();
        cx.bind_keys([
            KeyBinding::new("secondary-k", SearchSessions, Some("Nexus")),
            KeyBinding::new("secondary-n", NewTask, Some("Nexus")),
            KeyBinding::new("secondary-,", ToggleSettings, Some("Nexus")),
        ]);
        let owner = cx.weak_entity();
        let sidebar_pane = cx.new(|cx| WorkspacePane::new(owner.clone(), PaneKind::Sidebar, cx));
        let timeline_pane = cx.new(|cx| WorkspacePane::new(owner.clone(), PaneKind::Timeline, cx));
        let settings_pane = cx.new(|cx| WorkspacePane::new(owner, PaneKind::Settings, cx));
        let view = Self {
            presenter,
            prompt_input,
            executable_input,
            provider_name_input,
            provider_api_key_env_input,
            provider_api_key_input,
            provider_base_url_env_input,
            provider_base_url_input,
            provider_model_input,
            search_input,
            focus_handle: cx.focus_handle(),
            timeline_scroll: ScrollHandle::new(),
            sidebar_scroll: ScrollHandle::new(),
            settings_scroll: ScrollHandle::new(),
            sidebar_pane,
            timeline_pane,
            settings_pane,
            expanded_messages: HashSet::new(),
            collapsed_projects: HashSet::new(),
            codex_history_open: false,
            codex_history_visible_count: sidebar::HISTORY_PAGE_SIZE,
            settings_open: false,
            settings_section: SettingsSection::General,
            reduced_motion: false,
            editing_provider_profile,
        };
        view.focus_handle.focus(window, cx);
        view.start_event_pump(cx);
        view
    }

    fn start_event_pump(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
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
                view.presenter.notify_remote_changed();
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
        self.settings_open = false;
        self.expanded_messages.clear();
        self.focus_prompt(window, cx);
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    fn select_project(&mut self, project: Project) {
        self.presenter.select_project(project);
        self.expanded_messages.clear();
        self.timeline_scroll.scroll_to_bottom();
        self.presenter.notify_remote_changed();
    }

    fn select_task(&mut self, task_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        self.presenter.select_task(task_id);
        self.expanded_messages.clear();
        self.timeline_scroll.scroll_to_bottom();
        self.sync_executable(window, cx);
        self.sync_provider_profile_form(
            self.presenter
                .model()
                .selected_provider_profile()
                .map(|profile| profile.id),
            window,
            cx,
        );
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    fn select_codex_thread(&mut self, thread_id: String, cx: &mut Context<Self>) {
        self.presenter.select_codex_thread(thread_id);
        self.expanded_messages.clear();
        self.timeline_scroll.scroll_to_bottom();
        self.presenter.notify_remote_changed();
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
        self.presenter.notify_remote_changed();
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

    fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_open = !self.settings_open;
        if self.settings_open {
            self.focus_handle.focus(window, cx);
        } else {
            self.focus_prompt(window, cx);
        }
        cx.notify();
    }

    fn cancel(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.presenter.cancel();
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    fn copy_remote_link(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(link) = self.presenter.copyable_remote_link() {
            cx.write_to_clipboard(ClipboardItem::new_string(link));
            cx.notify();
        }
    }

    fn copy_remote_token(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(token) = self.presenter.copyable_remote_token() {
            cx.write_to_clipboard(ClipboardItem::new_string(token));
            cx.notify();
        }
    }

    fn probe(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let executable = self.executable_input.read(cx).value().to_string();
        self.presenter.probe(&executable);
        self.presenter.notify_remote_changed();
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
            self.sync_provider_profile_form(
                self.presenter
                    .model()
                    .selected_provider_profile()
                    .map(|profile| profile.id),
                window,
                cx,
            );
        }
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    fn select_model(&mut self, model: ClaudeModel, cx: &mut Context<Self>) {
        self.presenter.select_model(model);
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    fn select_effort(&mut self, effort: ThinkingEffort, cx: &mut Context<Self>) {
        self.presenter.select_effort(effort);
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    fn select_provider_profile(
        &mut self,
        profile_id: Option<Uuid>,
        edit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.presenter.select_provider_profile(profile_id) {
            if edit {
                self.sync_provider_profile_form(profile_id, window, cx);
            }
            self.presenter.notify_remote_changed();
        }
        cx.notify();
    }

    fn new_provider_profile(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_provider_profile_form(None, window, cx);
        self.provider_name_input
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn save_provider_profile(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let draft = ProviderProfileDraft {
            id: self.editing_provider_profile,
            name: self.provider_name_input.read(cx).value().to_string(),
            api_key_env: self.provider_api_key_env_input.read(cx).value().to_string(),
            api_key: self.provider_api_key_input.read(cx).value().to_string(),
            base_url_env: self
                .provider_base_url_env_input
                .read(cx)
                .value()
                .to_string(),
            base_url: self.provider_base_url_input.read(cx).value().to_string(),
            model: self.provider_model_input.read(cx).value().to_string(),
        };
        if let Some(profile_id) = self.presenter.save_provider_profile(draft) {
            self.sync_provider_profile_form(Some(profile_id), window, cx);
            self.presenter.notify_remote_changed();
        }
        cx.notify();
    }

    fn delete_provider_profile(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile_id) = self.editing_provider_profile else {
            return;
        };
        if self.presenter.delete_provider_profile(profile_id) {
            self.sync_provider_profile_form(None, window, cx);
            self.presenter.notify_remote_changed();
        }
        cx.notify();
    }

    fn sync_provider_profile_form(
        &mut self,
        profile_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let profile = profile_id.and_then(|profile_id| {
            self.presenter
                .model()
                .provider_profiles
                .iter()
                .find(|profile| profile.id == profile_id)
                .cloned()
        });
        let draft = profile_form_draft(profile.as_ref(), self.presenter.model().selected_harness);
        self.editing_provider_profile = draft.id;
        for (input, value) in [
            (&self.provider_name_input, draft.name),
            (&self.provider_api_key_env_input, draft.api_key_env),
            (&self.provider_api_key_input, draft.api_key),
            (&self.provider_base_url_env_input, draft.base_url_env),
            (&self.provider_base_url_input, draft.base_url),
            (&self.provider_model_input, draft.model),
        ] {
            input.update(cx, |input, cx| input.set_value(&value, window, cx));
        }
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
        let button_id = id;
        Button::new(id)
            .icon(IconName::Bot)
            .label(selected.to_string())
            .disabled(model.active_run.is_some())
            .small()
            .when(compact, |button| {
                button.ghost().h(px(COMPACT_CONTROL_HEIGHT)).max_w(px(180.))
            })
            .when(!compact, |button| {
                button.outline().w_full().h(px(CONTROL_HEIGHT))
            })
            .map(|button| {
                AnimatedDropdown::new(button_id, button, self.reduced_motion, move |menu, _, _| {
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
            })
    }

    fn provider_profile_selector(
        &self,
        id: &'static str,
        compact: bool,
        edit_on_select: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let model = self.presenter.model();
        let selected = model.selected_provider_profile().map(|profile| profile.id);
        let selected_name = model
            .selected_provider_profile()
            .map(|profile| profile.name.clone())
            .unwrap_or_else(|| "CLI 凭据".into());
        let profiles = model
            .provider_profiles
            .iter()
            .filter(|profile| profile.harness == model.selected_harness)
            .cloned()
            .collect::<Vec<_>>();
        let app = cx.entity().clone();
        let button_id = id;
        Button::new(button_id)
            .icon(IconName::Globe)
            .label(selected_name)
            .disabled(model.active_run.is_some())
            .small()
            .when(compact, |button| {
                button.ghost().h(px(COMPACT_CONTROL_HEIGHT)).max_w(px(180.))
            })
            .when(!compact, |button| {
                button.outline().w_full().h(px(CONTROL_HEIGHT))
            })
            .map(|button| {
                AnimatedDropdown::new(button_id, button, self.reduced_motion, move |menu, _, _| {
                    let app_for_default = app.clone();
                    profiles.iter().cloned().fold(
                        menu.min_w(if compact { px(180.) } else { px(220.) }).item(
                            PopupMenuItem::new("使用 CLI 当前凭据")
                                .checked(selected.is_none())
                                .on_click(move |_, window, cx| {
                                    app_for_default.update(cx, |app, cx| {
                                        app.select_provider_profile(
                                            None,
                                            edit_on_select,
                                            window,
                                            cx,
                                        )
                                    });
                                }),
                        ),
                        |menu, profile| {
                            let app = app.clone();
                            let profile_id = profile.id;
                            menu.item(
                                PopupMenuItem::new(profile.name)
                                    .checked(selected == Some(profile_id))
                                    .on_click(move |_, window, cx| {
                                        app.update(cx, |app, cx| {
                                            app.select_provider_profile(
                                                Some(profile_id),
                                                edit_on_select,
                                                window,
                                                cx,
                                            )
                                        });
                                    }),
                            )
                        },
                    )
                })
            })
    }

    fn model_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let selected = model.claude_model;
        let app = cx.entity().clone();
        let profile_model = model
            .selected_provider_profile()
            .and_then(|profile| profile.model.clone());
        let label = profile_model.clone().unwrap_or_else(|| {
            if model.selected_harness == HarnessKind::Claude {
                selected.to_string()
            } else {
                "CLI 默认模型".into()
            }
        });
        let button_id = "composer-model";
        Button::new(button_id)
            .ghost()
            .small()
            .h(px(COMPACT_CONTROL_HEIGHT))
            .max_w(px(200.))
            .label(label)
            .disabled(
                model.selected_harness != HarnessKind::Claude
                    || profile_model.is_some()
                    || model.active_run.is_some(),
            )
            .map(|button| {
                AnimatedDropdown::new(button_id, button, self.reduced_motion, move |menu, _, _| {
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
            })
    }

    fn effort_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let selected = model.effort;
        let app = cx.entity().clone();
        let button_id = "composer-effort";
        Button::new(button_id)
            .ghost()
            .small()
            .h(px(COMPACT_CONTROL_HEIGHT))
            .icon(IconName::Cpu)
            .label(selected.to_string())
            .disabled(model.active_run.is_some())
            .map(|button| {
                AnimatedDropdown::new(button_id, button, self.reduced_motion, move |menu, _, _| {
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
            })
    }
}

impl NexusView {
    fn render_workspace(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            "Agent 尚未就绪 · 打开设置检查探测和登录状态"
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
            .debug_selector(|| "workspace-page".into())
            .size_full()
            .flex()
            .child(
                self.sidebar_pane.clone().cached(
                    gpui::StyleRefinement::default()
                        .w(px(300.))
                        .h_full()
                        .flex_none(),
                ),
            )
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
                            .border_color(rgb(BORDER))
                            .px_4()
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
                                                .rounded(px(6.))
                                                .bg(rgb(SURFACE))
                                                .border_1()
                                                .border_color(rgb(BORDER))
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
                                        Button::new("open-settings")
                                            .debug_selector(|| "open-settings".into())
                                            .ghost()
                                            .small()
                                            .h(px(COMPACT_CONTROL_HEIGHT))
                                            .icon(IconName::Settings2)
                                            .label("设置")
                                            .tooltip(if cfg!(target_os = "macos") {
                                                "打开设置 · ⌘ ,"
                                            } else {
                                                "打开设置 · Ctrl ,"
                                            })
                                            .on_click(cx.listener(|app, _, window, cx| {
                                                app.toggle_settings(window, cx)
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        self.timeline_pane
                            .clone()
                            .cached(gpui::StyleRefinement::default().flex_1().min_h_0()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px_8()
                            .pt_3()
                            .pb_4()
                            .child(
                                div()
                                    .relative()
                                    .w_full()
                                    .max_w(px(816.))
                                    .mx_auto()
                                    .rounded(px(CARD_RADIUS))
                                    .bg(rgb(SURFACE))
                                    .border_1()
                                    .border_color(rgb(BORDER))
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
                                                    .child(self.provider_profile_selector(
                                                        "composer-provider-profile",
                                                        true,
                                                        false,
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
                                                        app.toggle_settings(window, cx);
                                                    })),
                                            )
                                        },
                                    ),
                            ),
                    ),
            )
    }
}

impl Render for NexusView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("Nexus")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|app, _: &SearchSessions, window, cx| {
                app.settings_open = false;
                app.search_input
                    .update(cx, |input, cx| input.focus(window, cx));
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &NewTask, window, cx| {
                app.new_task(window, cx);
            }))
            .on_action(cx.listener(|app, _: &ToggleSettings, window, cx| {
                app.toggle_settings(window, cx);
            }))
            .capture_action(cx.listener(|app, action: &Enter, window, cx| {
                if !app.settings_open
                    && action.secondary
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
            // Keep workspace layout state while settings is visible so measured
            // disclosures do not collapse and clamp scroll offsets on return.
            .child(
                div()
                    .size_full()
                    .when(self.settings_open, |page| page.invisible().absolute())
                    .child(self.render_workspace(window, cx)),
            )
            .when(self.settings_open, |element| {
                element.child(
                    self.settings_pane
                        .clone()
                        .cached(gpui::StyleRefinement::default().size_full()),
                )
            })
    }
}

fn provider_environment_defaults(harness: HarnessKind) -> (&'static str, &'static str) {
    match harness {
        HarnessKind::Claude => ("ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL"),
        HarnessKind::Codex => ("CODEX_API_KEY", "OPENAI_BASE_URL"),
        HarnessKind::Omp => ("DEEPSEEK_API_KEY", ""),
    }
}

fn profile_form_draft(
    profile: Option<&ProviderProfile>,
    harness: HarnessKind,
) -> ProviderProfileDraft {
    let (default_api_key_env, default_base_url_env) = provider_environment_defaults(harness);
    profile
        .map(|profile| ProviderProfileDraft {
            id: Some(profile.id),
            name: profile.name.clone(),
            api_key_env: profile.api_key_env.clone(),
            api_key: String::new(),
            base_url_env: profile.base_url_env.clone().unwrap_or_default(),
            base_url: profile.base_url.clone().unwrap_or_default(),
            model: profile.model.clone().unwrap_or_default(),
        })
        .unwrap_or_else(|| ProviderProfileDraft {
            id: None,
            name: String::new(),
            api_key_env: default_api_key_env.into(),
            api_key: String::new(),
            base_url_env: default_base_url_env.into(),
            base_url: String::new(),
            model: String::new(),
        })
}
