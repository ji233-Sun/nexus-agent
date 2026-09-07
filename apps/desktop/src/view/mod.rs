mod components;
mod model_picker;
mod pane;
mod settings;
mod sidebar;
pub(crate) mod theme;
mod timeline;
mod tools;

use crate::{
    i18n::Language,
    model::{
        AppModel, AppearanceSettings, ModelCatalogState, ThemePreference, history::HistoryMessage,
    },
    presenter::{Presenter, ProviderProfileDraft},
};
use components::*;
use gpui::{
    Anchor, Animation, AnimationExt as _, AnyElement, AppContext as _, ClipboardItem, Context,
    ElementId, Entity, FocusHandle, Focusable as _, Hsla, InteractiveElement as _, IntoElement,
    KeyBinding, ParentElement as _, PromptButton, PromptLevel, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, ease_out_quint,
    prelude::FluentBuilder as _, pulsating_between, px, relative, rgb, rgba,
};
use gpui_kit as gpui;
use gpui_kit::component::{
    Disableable as _, Icon, IconName, IndexPath, InteractiveElementExt as _, Selectable as _,
    Sizable as _,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    input::{Enter, Input, InputEvent, InputState, Textarea, TextareaState},
    list::{List, ListDelegate, ListEvent, ListItem, ListState},
    menu::PopupMenuItem,
    popover::Popover,
    searchable_list::SearchableListItem,
    switch::Switch,
    text::{TextView, TextViewStyle},
    tooltip::Tooltip,
};
use model_picker::{CatalogModelChoice, CatalogModelSelectContent, ModelPickerList};
use nexus_domain::{
    HarnessKind, Message, MessageKind, MessageRole, ModelDescriptor, Project, ProviderProfile,
    RunStatus, ThinkingEffort,
};
use pane::{PaneKind, WorkspacePane};
use settings::SettingsSection;
use std::{
    collections::{BTreeMap, HashSet},
    time::{Duration, Instant},
};
use theme::*;
use uuid::Uuid;

gpui::actions!(nexus_view, [SearchSessions, NewTask, ToggleSettings]);

pub(crate) struct NexusView {
    presenter: Presenter,
    prompt_input: Entity<TextareaState>,
    catalog_model_select: Entity<ListState<ModelPickerList>>,
    catalog_model_select_content: CatalogModelSelectContent,
    model_picker_open: bool,
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
        let locale = presenter.model().language;
        gpui_kit::component::set_locale(locale.as_str());
        cx.set_window_appearance(match presenter.model().appearance.theme {
            ThemePreference::System => None,
            ThemePreference::Light => Some(gpui::WindowAppearance::Light),
            ThemePreference::Dark => Some(gpui::WindowAppearance::Dark),
        });
        cx.observe_window_appearance(window, Self::refresh_appearance)
            .detach();
        cx.observe_window_activation(window, Self::refresh_appearance)
            .detach();
        let prompt_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .placeholder(locale.text("描述一个目标，让 Agent 开始工作…"))
        });
        let executable_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(presenter.model().executable.clone())
                .placeholder(locale.text("命令名或完整路径"))
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
                .placeholder(locale.text("例如 DeepSeek Production"))
        });
        let provider_api_key_env_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(api_key_env)
                .placeholder(locale.text("例如 DEEPSEEK_API_KEY"))
        });
        let provider_api_key_input = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(locale.text("新建时必填；编辑时留空保留"))
        });
        let provider_base_url_env_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(base_url_env)
                .placeholder(locale.text("可选，例如 OPENAI_BASE_URL"))
        });
        let provider_base_url_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(base_url)
                .placeholder(locale.text("可选，例如 https://api.example.com/v1"))
        });
        let provider_model_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(model)
                .placeholder(locale.text("可选，例如 deepseek/deepseek-v4-pro"))
        });
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(locale.text("搜索任务与历史…")));
        let catalog_model_select_content = CatalogModelSelectContent::from_model(presenter.model());
        let catalog_model_select = cx.new(|cx| {
            let mut state = ListState::new(
                ModelPickerList::new(catalog_model_select_content.clone()),
                window,
                cx,
            )
            .searchable(true);
            state.set_selected_index(catalog_model_select_content.selected_index(), window, cx);
            state
        });
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
        cx.subscribe_in(
            &catalog_model_select,
            window,
            |app, list, event: &ListEvent, _, cx| {
                match event {
                    ListEvent::Confirm(index) => {
                        let choice = list
                            .read(cx)
                            .delegate()
                            .item(*index)
                            .filter(|item| !item.disabled)
                            .map(|item| item.choice.clone());
                        match choice {
                            Some(CatalogModelChoice::FollowDefault) => {
                                app.select_catalog_model(None, cx)
                            }
                            Some(CatalogModelChoice::Model(id)) => {
                                app.select_catalog_model(Some(id), cx)
                            }
                            _ => return,
                        }
                        app.model_picker_open = false;
                    }
                    ListEvent::Cancel => app.model_picker_open = false,
                    _ => return,
                }
                cx.notify();
            },
        )
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
        let mut view = Self {
            presenter,
            prompt_input,
            catalog_model_select,
            catalog_model_select_content,
            model_picker_open: false,
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
        view.refresh_appearance(window, cx);
        view.focus_handle.focus(window, cx);
        view.start_event_pump(cx);
        view
    }

    fn refresh_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let appearance = ResolvedAppearance::resolve(
            self.presenter.model().appearance,
            window.appearance(),
            system_accessibility(),
            window.is_window_active(),
            cfg!(target_os = "macos"),
        );
        self.reduced_motion = appearance.reduced_motion;
        if apply_theme(appearance, cx) {
            window.set_background_appearance(appearance.window_background());
            window.refresh();
            cx.notify();
        }
    }

    fn set_appearance(
        &mut self,
        settings: AppearanceSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.presenter.set_appearance(settings) {
            cx.set_window_appearance(match settings.theme {
                ThemePreference::System => None,
                ThemePreference::Light => Some(gpui::WindowAppearance::Light),
                ThemePreference::Dark => Some(gpui::WindowAppearance::Dark),
            });
            self.refresh_appearance(window, cx);
        }
        cx.notify();
    }

    fn set_language(&mut self, language: Language, window: &mut Window, cx: &mut Context<Self>) {
        if self.presenter.set_language(language) {
            gpui_kit::component::set_locale(language.as_str());
            self.prompt_input.update(cx, |input, cx| {
                input.set_placeholder(
                    language.text("描述一个目标，让 Agent 开始工作…"),
                    window,
                    cx,
                );
            });
            for (input, placeholder) in [
                (&self.executable_input, "命令名或完整路径"),
                (&self.provider_name_input, "例如 DeepSeek Production"),
                (&self.provider_api_key_env_input, "例如 DEEPSEEK_API_KEY"),
                (&self.provider_api_key_input, "新建时必填；编辑时留空保留"),
                (
                    &self.provider_base_url_env_input,
                    "可选，例如 OPENAI_BASE_URL",
                ),
                (
                    &self.provider_base_url_input,
                    "可选，例如 https://api.example.com/v1",
                ),
                (
                    &self.provider_model_input,
                    "可选，例如 deepseek/deepseek-v4-pro",
                ),
                (&self.search_input, "搜索任务与历史…"),
            ] {
                input.update(cx, |input, cx| {
                    input.set_placeholder(language.text(placeholder), window, cx);
                });
            }
            self.sync_catalog_model_select(window, cx);
            window.refresh();
        }
        cx.notify();
    }

    fn start_event_pump(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                let Some(this) = this.upgrade() else { break };
                this.update(cx, |app, cx| {
                    app.poll_events(Instant::now(), cx);
                });
            }
        })
        .detach();
    }

    fn poll_events(&mut self, now: Instant, cx: &mut Context<Self>) {
        let follow_latest =
            self.timeline_scroll.max_offset().y + self.timeline_scroll.offset().y <= px(48.);
        if self.presenter.drain_events() {
            if follow_latest {
                self.timeline_scroll.scroll_to_bottom();
            }
            cx.notify();
        }
        if self.presenter.refresh_run_elapsed(now) {
            // Clock ticks are not new output: preserve scroll and skip remote broadcasts.
            self.timeline_pane.update(cx, |_, cx| cx.notify());
        }
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

    fn archive_task(&mut self, task_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.presenter.model().selected_task == Some(task_id);
        if self.presenter.archive_task(task_id) && selected {
            self.expanded_messages.clear();
            self.timeline_scroll.scroll_to_bottom();
            self.focus_prompt(window, cx);
        }
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    fn restore_task(&mut self, task_id: Uuid, cx: &mut Context<Self>) {
        self.presenter.restore_task(task_id);
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    fn confirm_delete_task(&mut self, task_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let locale = self.presenter.model().language;
        let Some(title) = self
            .presenter
            .model()
            .tasks
            .iter()
            .chain(&self.presenter.model().archived_tasks)
            .find(|task| task.id == task_id)
            .map(|task| task.title.clone())
        else {
            return;
        };
        let message = locale.format("永久删除“{title}”？", &[("title", (title).to_string())]);
        let answer = window.prompt(
            PromptLevel::Critical,
            &message,
            Some(locale.text("此操作会删除该对话的全部消息和运行记录，且无法撤销。")),
            &[
                PromptButton::ok(locale.text("永久删除")),
                PromptButton::cancel(locale.text("取消")),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() == Some(0) {
                let _ = this.update_in(cx, |app, window, cx| app.delete_task(task_id, window, cx));
            }
        })
        .detach();
    }

    fn delete_task(&mut self, task_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.presenter.model().selected_task == Some(task_id);
        if self.presenter.delete_task(task_id) && selected {
            self.expanded_messages.clear();
            self.timeline_scroll.scroll_to_bottom();
            if self.settings_open {
                self.focus_handle.focus(window, cx);
            } else {
                self.focus_prompt(window, cx);
            }
        }
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    fn confirm_delete_archived_tasks(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let locale = self.presenter.model().language;
        let count = self.presenter.model().archived_tasks.len();
        if count == 0 || self.presenter.model().active_run.is_some() {
            return;
        }
        let message = locale.format(
            "永久删除 {count} 个归档对话？",
            &[("count", (count).to_string())],
        );
        let detail = locale.format(
            "此操作会删除这 {count} 个对话的全部消息和运行记录，且无法撤销。",
            &[("count", (count).to_string())],
        );
        let answer = window.prompt(
            PromptLevel::Critical,
            &message,
            Some(&detail),
            &[
                PromptButton::ok(locale.text("全部删除")),
                PromptButton::cancel(locale.text("取消")),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() == Some(0) {
                let _ = this.update(cx, |app, cx| app.delete_archived_tasks(cx));
            }
        })
        .detach();
    }

    fn delete_archived_tasks(&mut self, cx: &mut Context<Self>) {
        self.presenter.delete_archived_tasks();
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

    fn select_catalog_model(&mut self, model_id: Option<String>, cx: &mut Context<Self>) {
        self.presenter.select_catalog_model(model_id);
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    fn refresh_model_catalog(
        &mut self,
        _: &gpui::ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.presenter.refresh_model_catalog();
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

    fn sync_catalog_model_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let content = CatalogModelSelectContent::from_model(self.presenter.model());
        if content == self.catalog_model_select_content {
            return;
        }
        self.catalog_model_select_content = content.clone();
        self.catalog_model_select.update(cx, |state, cx| {
            state.delegate_mut().replace_content(content);
            let selected = state.delegate().selected_index();
            state.set_selected_index(selected, window, cx);
            cx.notify();
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
        let locale = self.presenter.model().language;
        let model = self.presenter.model();
        let selected = model.selected_provider_profile().map(|profile| profile.id);
        let selected_name = model
            .selected_provider_profile()
            .map(|profile| profile.name.clone())
            .unwrap_or_else(|| locale.text("CLI 凭据").into());
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
                            PopupMenuItem::new(locale.text("使用 CLI 当前凭据"))
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

    fn effort_selector(&self, cx: &mut Context<Self>) -> AnyElement {
        let model = self.presenter.model();
        let locale = model.language;
        let selected = model.effort;
        let mut efforts = vec![ThinkingEffort::Default];
        if let Some(descriptor) = model.selected_catalog_model() {
            efforts.extend(
                descriptor
                    .supported_reasoning_efforts
                    .iter()
                    .map(|option| option.effort),
            );
        }
        let resolved = model.resolved_model_selection().effort;
        let label = if selected.is_default() && !resolved.is_default() {
            format!("{} · {}", locale.effort(selected), locale.effort(resolved))
        } else {
            locale.effort(resolved).to_owned()
        };
        let supported = efforts.len() > 1;
        let app = cx.entity().clone();
        let button_id = "composer-effort";
        let button = Button::new(button_id)
            .ghost()
            .small()
            .h(px(COMPACT_CONTROL_HEIGHT))
            .icon(IconName::Cpu)
            .label(label)
            .tooltip(if supported {
                locale.text("思考档位")
            } else {
                locale.text("当前模型未确认支持独立思考设置，将使用默认行为。")
            })
            .disabled(model.active_run.is_some() || !supported);
        if model.active_run.is_some() || !supported {
            return button.into_any_element();
        }
        AnimatedDropdown::new(button_id, button, self.reduced_motion, move |menu, _, _| {
            efforts
                .iter()
                .copied()
                .fold(menu.min_w(px(140.)), |menu, effort| {
                    let app = app.clone();
                    menu.item(
                        PopupMenuItem::new(locale.effort(effort))
                            .checked(effort == selected)
                            .on_click(move |_, _, cx| {
                                app.update(cx, |app, cx| app.select_effort(effort, cx));
                            }),
                    )
                })
        })
        .into_any_element()
    }
}

impl NexusView {
    fn render_message_queue(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let model = self.presenter.model();
        let colors = palette(cx);
        let queued = model
            .queued_messages
            .iter()
            .filter(|message| Some(message.task_id) == model.selected_task)
            .collect::<Vec<_>>();
        div().when(!queued.is_empty(), |element| {
            element
                .pb_2()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(colors.muted))
                        .child(
                            locale.format("排队消息 · {0}", &[("0", (queued.len()).to_string())]),
                        ),
                )
                .child(
                    div()
                        .id("message-queue")
                        .max_h(px(160.))
                        .overflow_y_scroll()
                        .children(queued.into_iter().map(|message| {
                            let id = message.id;
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .py_1()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(13.))
                                        .child(message.prompt.clone()),
                                )
                                .when(model.active_run.is_none(), |element| {
                                    element.child(
                                        Button::new((ElementId::from(id), "send-queued"))
                                            .ghost()
                                            .small()
                                            .label(locale.text("发送"))
                                            .on_click(cx.listener(move |app, _, _, cx| {
                                                app.presenter.send_queued_message(id);
                                                app.presenter.notify_remote_changed();
                                                cx.notify();
                                            })),
                                    )
                                })
                                .when(model.active_run.is_some(), |element| {
                                    element.child(
                                        Button::new((ElementId::from(id), "steer-queued"))
                                            .debug_selector(move || format!("steer-queued-{id}"))
                                            .ghost()
                                            .small()
                                            .label(if model.steering_message == Some(id) {
                                                locale.text("等待工具完成…")
                                            } else {
                                                locale.text("介入")
                                            })
                                            .tooltip(locale.text("等待工具执行结束后介入当前对话"))
                                            .disabled(
                                                !model.can_queue()
                                                    || model.steering_message.is_some(),
                                            )
                                            .on_click(cx.listener(move |app, _, _, cx| {
                                                app.presenter.steer_queued_message(id);
                                                app.presenter.notify_remote_changed();
                                                cx.notify();
                                            })),
                                    )
                                })
                                .child(
                                    Button::new((ElementId::from(id), "remove-queued"))
                                        .ghost()
                                        .small()
                                        .icon(IconName::Close)
                                        .accessibility_label(locale.text("移除排队消息"))
                                        .disabled(model.steering_message == Some(id))
                                        .on_click(cx.listener(move |app, _, _, cx| {
                                            app.presenter.remove_queued_message(id);
                                            cx.notify();
                                        })),
                                )
                        })),
                )
        })
    }

    fn render_workspace(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let material = materials(cx);
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
            locale.text("这是只读历史。选择项目并新建任务后即可开始。")
        } else if model.selected_project.is_none() {
            locale.text("先选择本地项目，再描述你希望完成的工作。")
        } else if model.active_run.is_some() {
            locale.text("Agent 正在执行 · 发送后排队，每轮结束后发送一条")
        } else if !model.can_submit() {
            locale.text("Agent 尚未就绪 · 打开设置检查探测和登录状态")
        } else if cfg!(target_os = "macos") {
            locale.text("⌘ Enter 发送消息 · Enter 换行")
        } else {
            locale.text("Ctrl Enter 发送消息 · Enter 换行")
        };
        let selected_thread = model.selected_codex_thread.as_ref().and_then(|thread_id| {
            model
                .codex_threads
                .iter()
                .find(|thread| &thread.id == thread_id)
        });
        let selected_task = model
            .selected_task
            .and_then(|task_id| model.tasks.iter().find(|task| task.id == task_id));
        let selected_profile_ready = model
            .selected_provider_profile()
            .is_some_and(|profile| profile.credential_configured);
        let header_status_pending = model.active_run.is_some()
            || model.codex_thread_loading
            || (!history
                && (matches!(model.model_catalog, ModelCatalogState::Loading { .. })
                    || model.selected_probe().is_none()));
        let header_status_color = if header_status_pending {
            rgb(colors.accent).into()
        } else if !history
            && (matches!(
                model.model_catalog,
                ModelCatalogState::Failed { .. } | ModelCatalogState::NotReady(_)
            ) || selected_task.is_some_and(|task| task.status == RunStatus::Failed))
        {
            rgb(colors.danger).into()
        } else if selected_task.is_some_and(|task| {
            matches!(task.status, RunStatus::Cancelled | RunStatus::Interrupted)
        }) {
            rgb(colors.warning).into()
        } else {
            probe
                .map(|probe| {
                    if probe.available && (probe.authenticated || selected_profile_ready) {
                        rgb(colors.success).into()
                    } else {
                        rgb(colors.danger).into()
                    }
                })
                .unwrap_or_else(|| rgb(colors.muted).into())
        };
        let header_project = if history {
            locale.text("Codex 历史").to_owned()
        } else {
            model
                .selected_project
                .as_ref()
                .map(|project| project.display_name.clone())
                .unwrap_or_else(|| locale.text("未选择项目").into())
        };
        let header_project_tooltip = if history {
            header_project.clone()
        } else {
            model.selected_project.as_ref().map_or_else(
                || header_project.clone(),
                |project| format!("{}\n{}", project.display_name, project.canonical_path),
            )
        };
        let header_task = selected_thread
            .map(|thread| thread.title.clone())
            .or_else(|| {
                model.selected_project.as_ref().map(|_| {
                    selected_task
                        .map(|task| task.title.clone())
                        .unwrap_or_else(|| locale.text("新建任务").into())
                })
            });
        let header_status = model.status_text().to_owned();
        div()
            .debug_selector(|| "workspace-page".into())
            .size_full()
            .flex()
            .child(
                self.sidebar_pane.clone().cached(
                    gpui::StyleRefinement::default()
                        .w(px(SIDEBAR_WIDTH))
                        .h_full()
                        .flex_none(),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .debug_selector(|| "workspace-header".into())
                            .h(px(HEADER_HEIGHT))
                            .flex_none()
                            .bg(material.chrome)
                            .border_b(px(0.5))
                            .border_color(material.edge)
                            .px_4()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Icon::new(if history {
                                            IconName::FileText
                                        } else {
                                            IconName::Folder
                                        })
                                        .text_color(rgb(colors.muted)),
                                    )
                                    .child(
                                        div()
                                            .id("workspace-header-project")
                                            .debug_selector(|| "workspace-header-project".into())
                                            .max_w(px(200.))
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(13.))
                                            .text_color(rgb(colors.text_secondary))
                                            .tooltip(move |window, cx| {
                                                Tooltip::new(header_project_tooltip.clone())
                                                    .build(window, cx)
                                            })
                                            .child(header_project),
                                    )
                                    .when_some(header_task, |element, task| {
                                        let tooltip = task.clone();
                                        element
                                            .child(
                                                Icon::new(IconName::ChevronRight)
                                                    .size(px(14.))
                                                    .text_color(rgb(colors.muted)),
                                            )
                                            .child(
                                                div()
                                                    .id("workspace-header-task")
                                                    .debug_selector(|| {
                                                        "workspace-header-task".into()
                                                    })
                                                    .flex_1()
                                                    .min_w_0()
                                                    .truncate()
                                                    .font_weight(gpui::FontWeight::MEDIUM)
                                                    .tooltip(move |window, cx| {
                                                        Tooltip::new(tooltip.clone())
                                                            .build(window, cx)
                                                    })
                                                    .child(task),
                                            )
                                    }),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors.muted))
                                    .child(live_status_dot(
                                        header_status_color,
                                        header_status_pending && !self.reduced_motion,
                                    ))
                                    .child(
                                        div()
                                            .id("workspace-header-status")
                                            .debug_selector(|| "workspace-header-status".into())
                                            .max_w(px(180.))
                                            .min_w_0()
                                            .truncate()
                                            .tooltip({
                                                let header_status = header_status.clone();
                                                move |window, cx| {
                                                    Tooltip::new(header_status.clone())
                                                        .build(window, cx)
                                                }
                                            })
                                            .child(header_status),
                                    )
                                    .child(
                                        Button::new("open-settings")
                                            .debug_selector(|| "open-settings".into())
                                            .ghost()
                                            .small()
                                            .size(px(COMPACT_CONTROL_HEIGHT))
                                            .p_0()
                                            .icon(IconName::Settings2)
                                            .accessibility_label(locale.text("设置"))
                                            .tooltip(if cfg!(target_os = "macos") {
                                                locale.text("打开设置 · ⌘ ,")
                                            } else {
                                                locale.text("打开设置 · Ctrl ,")
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
                            .bg(rgb(colors.canvas))
                            .px(px(24.))
                            .pt_3()
                            .pb_4()
                            .child(
                                div()
                                    .debug_selector(|| "composer-surface".into())
                                    .relative()
                                    .w_full()
                                    .max_w(px(CONTENT_WIDTH))
                                    .mx_auto()
                                    .rounded(px(20.))
                                    .bg(material.floating)
                                    .border_1()
                                    .border_color(material.edge)
                                    .when(prompt_focused, |element| {
                                        element.border_color(rgb(colors.accent))
                                    })
                                    .shadow(material.shadow())
                                    .p_3()
                                    .flex()
                                    .flex_col()
                                    .child(self.render_message_queue(cx))
                                    .child(
                                        Textarea::new(&self.prompt_input)
                                            .disabled(history)
                                            .appearance(false)
                                            .bordered(false)
                                            .aria_label(locale.text("任务描述")),
                                    )
                                    .child(
                                        div()
                                            .min_h(px(COMPACT_CONTROL_HEIGHT))
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
                                                    .child(self.model_selector(window, cx))
                                                    .child(self.effort_selector(cx)),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .when(model.active_run.is_some(), |element| {
                                                        element.child(
                                                            Button::new("composer-cancel")
                                                                .danger()
                                                                .outline()
                                                                .small()
                                                                .h(px(COMPACT_CONTROL_HEIGHT))
                                                                .icon(IconName::Pause)
                                                                .label(locale.text("停止"))
                                                                .disabled(model.run_cancelling)
                                                                .tooltip(locale.text(
                                                                    "停止当前运行，保留已有输出",
                                                                ))
                                                                .on_click(
                                                                    cx.listener(Self::cancel),
                                                                ),
                                                        )
                                                    })
                                                    .child(
                                                        Button::new("submit")
                                                            .primary()
                                                            .small()
                                                            .size(px(COMPACT_CONTROL_HEIGHT))
                                                            .p_0()
                                                            .icon(IconName::ArrowUp)
                                                            .accessibility_label(
                                                                if model.active_run.is_some() {
                                                                    locale.text("加入消息队列")
                                                                } else {
                                                                    locale.text("发送任务")
                                                                },
                                                            )
                                                            .tooltip(composer_hint)
                                                            .when(!can_submit, |button| {
                                                                button.opacity(0.42)
                                                            })
                                                            .disabled(!can_submit)
                                                            .on_click(cx.listener(Self::submit)),
                                                    ),
                                            ),
                                    )
                                    .map(|element| {
                                        entrance(element, "composer-enter", !self.reduced_motion)
                                    }),
                            )
                            .child(
                                div()
                                    .max_w(px(CONTENT_WIDTH))
                                    .mx_auto()
                                    .mt_3()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(rgb(colors.muted))
                                            .child(composer_hint),
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
                                                    .label(locale.text("检查环境"))
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
        if self.model_picker_open && self.presenter.model().active_run.is_some() {
            self.model_picker_open = false;
            self.prompt_input
                .update(cx, |input, cx| input.focus(window, cx));
        }
        self.sync_catalog_model_select(window, cx);
        let colors = palette(cx);
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
            .bg(rgba(0x00000000))
            .text_color(rgb(colors.text))
            .text_size(px(14.))
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

#[cfg(test)]
mod catalog_model_tests {
    use super::*;
    use crate::presenter::tests::fixture;
    use nexus_domain::ModelReasoningEffort;
    use nexus_protocol::Event;

    fn omp_model(provider: &str, id: &str) -> ModelDescriptor {
        ModelDescriptor {
            source: nexus_domain::ModelSource::OmpCli,
            availability: nexus_domain::ModelAvailability::Available,
            id: id.into(),
            display_name: "Shared Model".into(),
            provider: Some(provider.into()),
            is_default: false,
            supported_reasoning_efforts: vec![ModelReasoningEffort {
                effort: ThinkingEffort::XHigh,
                description: String::new(),
            }],
            default_reasoning_effort: None,
        }
    }

    #[test]
    fn catalog_content_groups_providers_and_searches_provider_name_and_full_id() {
        let mut model = AppModel {
            selected_harness: HarnessKind::Omp,
            model_catalog: ModelCatalogState::Ready(vec![
                omp_model("openai", "openai/shared-model"),
                omp_model("bigmodel", "bigmodel/shared-model"),
            ]),
            ..AppModel::default()
        };
        model.model_override = Some("bigmodel/shared-model".into());

        let content = CatalogModelSelectContent::from_model(&model);
        assert_eq!(
            content
                .groups
                .iter()
                .map(|group| group.title.as_str())
                .collect::<Vec<_>>(),
            vec!["默认", "bigmodel", "openai"]
        );
        let bigmodel = &content.groups[1].items[0];
        assert!(bigmodel.matches("BIGMODEL"));
        assert!(bigmodel.matches("shared model"));
        assert!(bigmodel.matches("bigmodel/shared-model"));
        assert_eq!(
            bigmodel.choice,
            CatalogModelChoice::Model("bigmodel/shared-model".into())
        );
        assert_eq!(
            content.groups[2].items[0].choice,
            CatalogModelChoice::Model("openai/shared-model".into())
        );
        assert!(content.selected_index().is_some());
    }

    #[test]
    fn catalog_content_keeps_an_unavailable_full_selector_visible() {
        let model = AppModel {
            selected_harness: HarnessKind::Omp,
            model_override: Some("private-provider/custom-model".into()),
            model_catalog: ModelCatalogState::Ready(vec![omp_model(
                "public-provider",
                "public-provider/custom-model",
            )]),
            ..AppModel::default()
        };

        let content = CatalogModelSelectContent::from_model(&model);
        let current = &content.groups[1];
        assert_eq!(current.title, "当前选择");
        assert!(current.items[0].disabled);
        assert_eq!(
            current.items[0].choice,
            CatalogModelChoice::Model("private-provider/custom-model".into())
        );
        assert!(current.items[0].title.contains("不可用"));
        assert!(content.selected_index().is_some());
    }

    #[test]
    fn picker_distinguishes_catalog_states_and_does_not_confuse_default_with_override() {
        let mut default = omp_model("provider", "default-model");
        default.is_default = true;
        let mut explicit = omp_model("provider", "explicit-model");
        explicit.display_name = "Chosen display name".into();
        let mut model = AppModel {
            selected_harness: HarnessKind::Omp,
            model_override: Some("explicit-model".into()),
            model_override_name: Some(explicit.display_name.clone()),
            model_catalog: ModelCatalogState::Ready(vec![default, explicit]),
            ..Default::default()
        };
        let content = CatalogModelSelectContent::from_model(&model);
        assert!(content.groups[0].items[0].title.contains("default-model"));
        assert!(!content.groups[0].items[0].title.contains("explicit-model"));
        for (state, status) in [
            (ModelCatalogState::Idle, "选择项目"),
            (
                ModelCatalogState::Loading {
                    request_id: Uuid::new_v4(),
                    models: vec![],
                },
                "正在加载",
            ),
            (ModelCatalogState::Empty, "目录为空"),
            (
                ModelCatalogState::Failed {
                    message: "test failure".into(),
                    models: vec![],
                },
                "test failure",
            ),
            (
                ModelCatalogState::NotReady("CLI not ready".into()),
                "CLI not ready",
            ),
        ] {
            model.model_catalog = state;
            let content = CatalogModelSelectContent::from_model(&model);
            let items = content
                .groups
                .iter()
                .flat_map(|group| &group.items)
                .collect::<Vec<_>>();
            assert!(items.iter().any(|item| item.title.contains(status)));
            assert!(
                items
                    .iter()
                    .any(|item| item.disabled && item.title.contains("Chosen display name"))
            );
        }
        let mut default = omp_model("provider", "default-model");
        default.is_default = true;
        default.availability = nexus_domain::ModelAvailability::Unavailable {
            reason: "disabled by provider".into(),
        };
        model.model_catalog = ModelCatalogState::Ready(vec![default]);
        model.model_override = None;
        let content = CatalogModelSelectContent::from_model(&model);
        let default = &content.groups[0].items[0];
        assert!(default.title.contains("disabled by provider"));
        assert!(default.trigger_title.contains("不可用"));
    }

    #[gpui::test]
    fn unified_picker_supports_keyboard_focus_and_minimum_window_bounds(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, runner, _directory) = fixture();
        presenter.select_harness(HarnessKind::Omp, "claude");
        let ModelCatalogState::Loading { request_id, .. } = presenter.model().model_catalog else {
            panic!("loading")
        };
        let mut long = omp_model("provider", "provider/needle-target");
        long.display_name = "Long model name · 很长的模型名称 ".repeat(40);
        runner.emit(Event::ModelCatalogLoaded {
            request_id,
            harness: HarnessKind::Omp,
            models: vec![long, omp_model("provider", "provider/other-model")],
        });
        presenter.drain_events();
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
        for (width, height, theme, glass) in [
            (1040., 680., ThemePreference::Light, true),
            (1280., 800., ThemePreference::Dark, false),
        ] {
            cx.simulate_resize(gpui::size(px(width), px(height)));
            view.update_in(cx, |view, window, cx| {
                view.set_appearance(
                    AppearanceSettings {
                        theme,
                        glass,
                        reduced_motion: true,
                    },
                    window,
                    cx,
                );
                view.prompt_input
                    .update(cx, |input, cx| input.focus(window, cx));
            });
            cx.run_until_parked();
            let trigger = cx.debug_bounds("composer-model").unwrap();
            assert!(trigger.right() <= px(width));
            assert!(trigger.size.width <= px(300.));
            cx.simulate_click(trigger.center(), Default::default());
            cx.run_until_parked();
            let bounds = cx.debug_bounds("model-picker-surface").unwrap();
            assert!(
                bounds.left() >= px(0.) && bounds.right() <= px(width),
                "{bounds:?}"
            );
            assert!(
                bounds.top() >= px(0.) && bounds.bottom() <= px(height),
                "{bounds:?}"
            );
            assert!(cx.debug_bounds("model-config-claude-cli").is_some());
            assert!(cx.debug_bounds("model-config-codex-cli").is_some());
            assert!(cx.debug_bounds("model-config-omp-cli").is_some());
            view.update_in(cx, |view, window, cx| {
                assert!(
                    view.catalog_model_select
                        .focus_handle(cx)
                        .is_focused(window)
                );
            });
            cx.simulate_keystrokes("down up escape");
            cx.run_until_parked();
            assert!(cx.debug_bounds("model-picker-surface").is_none());
            view.update_in(cx, |view, window, cx| {
                assert!(view.prompt_input.focus_handle(cx).is_focused(window));
            });
            cx.simulate_click(trigger.center(), Default::default());
            cx.run_until_parked();
            cx.simulate_input("NEEDLE-target");
            cx.run_until_parked();
            cx.simulate_keystrokes("enter");
            cx.run_until_parked();
            assert!(cx.debug_bounds("model-picker-surface").is_none());
            assert_eq!(
                view.read_with(cx, |view, _| view.presenter.model().model_override.clone()),
                Some("provider/needle-target".into())
            );
        }
        let trigger = cx.debug_bounds("composer-model").unwrap();
        cx.simulate_click(trigger.center(), Default::default());
        cx.run_until_parked();
        let claude = cx.debug_bounds("model-config-claude-cli").unwrap();
        cx.simulate_click(claude.center(), Default::default());
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.presenter.model().selected_harness),
            HarnessKind::Claude
        );
        assert!(cx.debug_bounds("model-picker-surface").is_some());
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(cx.debug_bounds("model-picker-surface").is_none());
    }

    #[gpui::test]
    fn queued_message_steer_button_targets_the_message_and_waits_for_receipt(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, runner, _directory) = fixture();
        assert!(presenter.submit("first", "claude"));
        assert!(presenter.submit("correction", "claude"));
        let run_id = presenter.model().active_run.unwrap();
        let message_id = presenter.model().queued_messages[0].id;
        let selector = format!("steer-queued-{message_id}").leak();
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        let button = cx
            .debug_bounds(selector)
            .expect("queued message must expose Steer")
            .center();
        cx.simulate_click(button, Default::default());
        view.read_with(cx, |view, _| {
            assert_eq!(view.presenter.model().steering_message, Some(message_id));
            assert_eq!(view.presenter.model().queued_messages.len(), 1);
        });
        runner.emit(Event::RunInputAccepted { run_id, message_id });
        view.update_in(cx, |view, _, cx| {
            view.presenter.drain_events();
            cx.notify();
        });
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds(selector).is_none());
        assert!(view.read_with(cx, |view, _| {
            view.presenter.model().queued_messages.is_empty()
        }));
    }

    #[gpui::test]
    fn catalog_select_syncs_after_a_catalog_response(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, runner, _directory) = fixture();
        assert!(presenter.select_harness(HarnessKind::Omp, "claude"));
        let ModelCatalogState::Loading { request_id, .. } = presenter.model().model_catalog else {
            panic!("expected loading catalog")
        };
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });

        runner.emit(Event::ModelCatalogLoaded {
            request_id,
            harness: HarnessKind::Omp,
            models: vec![omp_model("bigmodel", "bigmodel/shared-model")],
        });
        view.update_in(cx, |view, _, cx| {
            assert!(view.presenter.drain_events());
            cx.notify();
        });
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        view.update_in(cx, |view, _, cx| {
            view.presenter
                .select_catalog_model(Some("bigmodel/shared-model".into()));
            cx.notify();
        });
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });

        assert_eq!(
            view.read_with(cx, |view, cx| {
                view.catalog_model_select
                    .read(cx)
                    .delegate()
                    .selected_index()
                    .and_then(|index| {
                        view.catalog_model_select
                            .read(cx)
                            .delegate()
                            .item(index)
                            .map(|item| item.choice.clone())
                    })
            }),
            Some(CatalogModelChoice::Model("bigmodel/shared-model".into()))
        );
    }
}
