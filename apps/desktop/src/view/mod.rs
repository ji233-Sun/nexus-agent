mod components;
mod pane;
mod settings;
mod sidebar;
pub(crate) mod theme;
mod timeline;
mod tools;

use crate::{
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
    menu::PopupMenuItem,
    searchable_list::{SearchableGroup, SearchableListItem, SearchableVec},
    select::{Select, SelectEvent, SelectState},
    switch::Switch,
    text::{TextView, TextViewStyle},
};
use nexus_domain::{
    ClaudeModel, HarnessKind, Message, MessageKind, MessageRole, ModelDescriptor, Project,
    ProviderProfile, RunStatus, ThinkingEffort,
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum CatalogModelChoice {
    FollowDefault,
    Model(String),
    Status(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CatalogModelItem {
    choice: CatalogModelChoice,
    title: String,
    trigger_title: String,
    search_text: String,
    disabled: bool,
}

impl CatalogModelItem {
    fn follow_default(title: String, trigger_title: String, search_text: String) -> Self {
        Self {
            choice: CatalogModelChoice::FollowDefault,
            title,
            trigger_title,
            search_text,
            disabled: false,
        }
    }

    fn model(model: &ModelDescriptor) -> Self {
        let title = catalog_model_row_title(model);
        let trigger_title = catalog_model_trigger_title(model);
        let provider = model.provider.as_deref().unwrap_or_default();
        Self {
            choice: CatalogModelChoice::Model(model.id.clone()),
            search_text: format!("{provider} {} {}", model.display_name, model.id),
            title,
            trigger_title,
            disabled: false,
        }
    }

    fn unavailable(model_id: &str, state: &ModelCatalogState) -> Self {
        let availability = match state {
            ModelCatalogState::Loading { .. } => "验证中",
            ModelCatalogState::Ready(_) | ModelCatalogState::Empty => "不可用",
            ModelCatalogState::Idle | ModelCatalogState::Failed(_) => "未验证",
        };
        let title = format!("{model_id} · {availability}");
        Self {
            choice: CatalogModelChoice::Model(model_id.to_owned()),
            trigger_title: title.clone(),
            search_text: format!("{model_id} {availability}"),
            title,
            disabled: true,
        }
    }

    fn status(title: String) -> Self {
        Self {
            choice: CatalogModelChoice::Status(title.clone()),
            trigger_title: title.clone(),
            search_text: title.clone(),
            title,
            disabled: true,
        }
    }
}

impl SearchableListItem for CatalogModelItem {
    type Value = CatalogModelChoice;

    fn title(&self) -> SharedString {
        self.title.clone().into()
    }

    fn display_title(&self) -> Option<AnyElement> {
        Some(self.trigger_title.clone().into_any_element())
    }

    fn value(&self) -> &Self::Value {
        &self.choice
    }

    fn matches(&self, query: &str) -> bool {
        self.search_text
            .to_lowercase()
            .contains(&query.trim().to_lowercase())
    }

    fn disabled(&self) -> bool {
        self.disabled
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CatalogModelGroup {
    title: String,
    items: Vec<CatalogModelItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CatalogModelSelectContent {
    groups: Vec<CatalogModelGroup>,
    selected: CatalogModelChoice,
}

type CatalogModelSelect = SearchableVec<SearchableGroup<CatalogModelItem>>;

impl CatalogModelSelectContent {
    fn from_model(model: &AppModel) -> Self {
        let selected = model
            .model_override
            .as_ref()
            .map(|model_id| CatalogModelChoice::Model(model_id.clone()))
            .unwrap_or(CatalogModelChoice::FollowDefault);
        let mut groups = vec![CatalogModelGroup {
            title: "默认".into(),
            items: vec![catalog_follow_default_item(model)],
        }];
        let catalog_models = model.model_catalog.models().unwrap_or_default();

        if let Some(model_id) = model.model_override.as_deref()
            && !catalog_models.iter().any(|entry| entry.id == model_id)
        {
            groups.push(CatalogModelGroup {
                title: "当前选择".into(),
                items: vec![CatalogModelItem::unavailable(
                    model_id,
                    &model.model_catalog,
                )],
            });
        }

        let mut provider_groups = BTreeMap::<String, Vec<CatalogModelItem>>::new();
        for descriptor in catalog_models {
            let provider = descriptor
                .provider
                .clone()
                .unwrap_or_else(|| model.selected_harness.to_string());
            provider_groups
                .entry(provider)
                .or_default()
                .push(CatalogModelItem::model(descriptor));
        }
        groups.extend(
            provider_groups
                .into_iter()
                .map(|(title, items)| CatalogModelGroup { title, items }),
        );

        let status = match &model.model_catalog {
            ModelCatalogState::Idle if model.selected_project.is_none() => {
                Some("选择项目后加载模型目录".into())
            }
            ModelCatalogState::Idle => Some("模型目录尚未加载".into()),
            ModelCatalogState::Loading { .. } => Some("正在加载模型目录…".into()),
            ModelCatalogState::Empty => Some("当前模型目录为空".into()),
            ModelCatalogState::Failed(message) => Some(format!("模型目录加载失败：{message}")),
            ModelCatalogState::Ready(_) => None,
        };
        if let Some(status) = status {
            groups.push(CatalogModelGroup {
                title: "状态".into(),
                items: vec![CatalogModelItem::status(status)],
            });
        }

        Self { groups, selected }
    }

    fn delegate(&self) -> CatalogModelSelect {
        SearchableVec::new(
            self.groups
                .iter()
                .map(|group| {
                    SearchableGroup::new(group.title.clone()).items(group.items.iter().cloned())
                })
                .collect::<Vec<_>>(),
        )
    }

    fn selected_index(&self) -> Option<IndexPath> {
        self.groups.iter().enumerate().find_map(|(section, group)| {
            group
                .items
                .iter()
                .position(|item| item.choice == self.selected)
                .map(|row| IndexPath::new(row).section(section))
        })
    }
}

fn catalog_model_row_title(model: &ModelDescriptor) -> String {
    if model.display_name == model.id {
        model.id.clone()
    } else {
        format!("{} · {}", model.display_name, model.id)
    }
}

fn catalog_model_trigger_title(model: &ModelDescriptor) -> String {
    model
        .provider
        .as_deref()
        .map(|provider| format!("{provider} · {}", model.display_name))
        .unwrap_or_else(|| model.display_name.clone())
}

fn catalog_follow_default_item(model: &AppModel) -> CatalogModelItem {
    let profile_model = model
        .selected_provider_profile()
        .and_then(|profile| profile.model.as_deref());
    if let Some(model_id) = profile_model {
        if let Some(descriptor) = model
            .model_catalog
            .models()
            .and_then(|models| models.iter().find(|entry| entry.id == model_id))
        {
            return CatalogModelItem::follow_default(
                format!(
                    "跟随 Profile 默认 · {}",
                    catalog_model_row_title(descriptor)
                ),
                format!("默认 · {}", catalog_model_trigger_title(descriptor)),
                format!(
                    "default 默认 profile {} {}",
                    descriptor.display_name, descriptor.id
                ),
            );
        }
        let verification = match &model.model_catalog {
            ModelCatalogState::Loading { .. } => "验证中",
            ModelCatalogState::Failed(_) => "目录加载失败",
            ModelCatalogState::Ready(_) | ModelCatalogState::Empty => "目录未验证",
            ModelCatalogState::Idle => "未验证",
        };
        return CatalogModelItem::follow_default(
            format!("跟随 Profile 默认 · {model_id}（{verification}）"),
            format!("默认 · {model_id} · {verification}"),
            format!("default 默认 profile {model_id} {verification}"),
        );
    }

    if let Some(descriptor) = model.selected_catalog_model() {
        return CatalogModelItem::follow_default(
            format!("跟随 CLI 默认 · {}", catalog_model_row_title(descriptor)),
            format!("CLI 默认 · {}", catalog_model_trigger_title(descriptor)),
            format!(
                "default 默认 cli {} {}",
                descriptor.display_name, descriptor.id
            ),
        );
    }

    let suffix = match &model.model_catalog {
        ModelCatalogState::Loading { .. } => " · 目录加载中",
        ModelCatalogState::Failed(_) => " · 目录加载失败",
        ModelCatalogState::Empty => " · 目录为空",
        ModelCatalogState::Idle | ModelCatalogState::Ready(_) => "",
    };
    CatalogModelItem::follow_default(
        "跟随 CLI 默认模型".into(),
        format!("CLI 默认模型{suffix}"),
        "default 默认 cli".into(),
    )
}

pub(crate) struct NexusView {
    presenter: Presenter,
    prompt_input: Entity<TextareaState>,
    catalog_model_select: Entity<SelectState<CatalogModelSelect>>,
    catalog_model_select_content: CatalogModelSelectContent,
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
        let catalog_model_select_content = CatalogModelSelectContent::from_model(presenter.model());
        let catalog_model_select = cx.new(|cx| {
            SelectState::new(
                catalog_model_select_content.delegate(),
                catalog_model_select_content.selected_index(),
                window,
                cx,
            )
            .searchable(true)
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
        cx.subscribe(
            &catalog_model_select,
            |app, _, event: &SelectEvent<CatalogModelSelect>, cx| {
                let SelectEvent::Confirm(choice) = event;
                match choice {
                    Some(CatalogModelChoice::FollowDefault) => {
                        app.select_catalog_model(None, cx);
                    }
                    Some(CatalogModelChoice::Model(model_id)) => {
                        app.select_catalog_model(Some(model_id.clone()), cx);
                    }
                    Some(CatalogModelChoice::Status(_)) | None => {}
                }
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
        let message = format!("永久删除“{title}”？");
        let answer = window.prompt(
            PromptLevel::Critical,
            &message,
            Some("此操作会删除该对话的全部消息和运行记录，且无法撤销。"),
            &[PromptButton::ok("永久删除"), PromptButton::cancel("取消")],
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
        let count = self.presenter.model().archived_tasks.len();
        if count == 0 || self.presenter.model().active_run.is_some() {
            return;
        }
        let message = format!("永久删除 {count} 个归档对话？");
        let detail = format!("此操作会删除这 {count} 个对话的全部消息和运行记录，且无法撤销。");
        let answer = window.prompt(
            PromptLevel::Critical,
            &message,
            Some(&detail),
            &[PromptButton::ok("全部删除"), PromptButton::cancel("取消")],
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

    fn select_model(&mut self, model: ClaudeModel, cx: &mut Context<Self>) {
        self.presenter.select_model(model);
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
        let selected = content.selected.clone();
        self.catalog_model_select.update(cx, |state, cx| {
            state.set_items(content.delegate(), window, cx);
            state.set_selected_value(&selected, window, cx);
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

    fn model_selector(&self, cx: &mut Context<Self>) -> AnyElement {
        let model = self.presenter.model();
        if model.uses_model_catalog() {
            return self.catalog_model_selector(cx);
        }
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
            .into_any_element()
    }

    fn catalog_model_selector(&self, cx: &mut Context<Self>) -> AnyElement {
        let model = self.presenter.model();
        let active = model.active_run.is_some();
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div().w(px(220.)).h(px(COMPACT_CONTROL_HEIGHT)).child(
                    Select::new(&self.catalog_model_select)
                        .small()
                        .appearance(false)
                        .accessibility_label("模型")
                        .search_placeholder("按 Provider、名称或模型 ID 搜索")
                        .menu_width(px(360.))
                        .menu_max_h(px(360.))
                        .disabled(active)
                        .size_full(),
                ),
            )
            .child(
                Button::new("composer-model-refresh")
                    .ghost()
                    .small()
                    .size(px(COMPACT_CONTROL_HEIGHT))
                    .p_0()
                    .icon(IconName::RotateCw)
                    .accessibility_label("刷新模型目录")
                    .tooltip(format!("刷新 {} 模型目录", model.selected_harness))
                    .disabled(active || model.selected_project.is_none())
                    .on_click(cx.listener(Self::refresh_model_catalog)),
            )
            .into_any_element()
    }

    fn effort_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let selected = model.effort;
        let efforts = if model.uses_model_catalog() {
            let mut efforts = vec![ThinkingEffort::Default];
            if let Some(catalog_model) = model.selected_catalog_model() {
                efforts.extend(
                    catalog_model
                        .supported_reasoning_efforts
                        .iter()
                        .map(|option| option.effort),
                );
            }
            if !efforts.contains(&selected) {
                efforts.push(selected);
            }
            efforts
        } else {
            ThinkingEffort::ALL.to_vec()
        };
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
                    efforts
                        .iter()
                        .copied()
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
            "这是只读历史。选择项目并新建任务后即可开始。"
        } else if model.selected_project.is_none() {
            "先选择本地项目，再描述你希望完成的工作。"
        } else if model.active_run.is_some() {
            "Agent 正在执行 · 可以提前起草下一条消息"
        } else if !model.can_submit() {
            "Agent 尚未就绪 · 打开设置检查探测和登录状态"
        } else if cfg!(target_os = "macos") {
            "⌘ Enter 发送消息 · Enter 换行"
        } else {
            "Ctrl Enter 发送消息 · Enter 换行"
        };
        let header_status_color = if model.active_run.is_some() {
            rgb(colors.accent).into()
        } else {
            probe
                .map(|probe| {
                    if probe.available && probe.authenticated {
                        rgb(colors.success).into()
                    } else {
                        rgb(colors.danger).into()
                    }
                })
                .unwrap_or_else(|| rgb(colors.muted).into())
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
                                        .text_color(rgb(colors.muted)),
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
                                                .text_size(px(12.))
                                                .text_color(rgb(colors.muted))
                                                .child(context),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors.muted))
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
                                    .child(
                                        Textarea::new(&self.prompt_input)
                                            .disabled(history)
                                            .appearance(false)
                                            .bordered(false)
                                            .aria_label("任务描述"),
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

    #[gpui::test]
    fn catalog_select_syncs_after_a_catalog_response(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, runner, _directory) = fixture();
        assert!(presenter.select_harness(HarnessKind::Omp, "claude"));
        let ModelCatalogState::Loading { request_id } = presenter.model().model_catalog else {
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
                view.catalog_model_select.read(cx).selected_value().cloned()
            }),
            Some(CatalogModelChoice::Model("bigmodel/shared-model".into()))
        );
    }
}
