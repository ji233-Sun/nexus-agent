mod attachments;
mod cnb_media;
mod components;
mod composer;
mod conversation;
mod events;
mod fonts;
mod issues;
mod lifecycle;
mod model_picker;
mod navigation;
mod pane;
mod pdf;
mod provider_profiles;
mod review;
mod settings;
mod sidebar;
pub(crate) mod theme;
mod timeline;
mod tools;
mod user_ask;
mod voice;
mod workspace;

use crate::{
    i18n::Language,
    model::{
        AppModel, AppearanceSettings, GenerationKind, ModelCatalogState, PendingUserAsk,
        ThemePreference, UserAskSubmissionState, issues::IssueProvider,
    },
    presenter::{Presenter, ProviderProfileDraft},
};
use components::*;
use gpui::{
    Anchor, Animation, AnimationExt as _, AnyElement, AppContext as _, ClipboardItem, Context,
    ElementId, Entity, FocusHandle, Focusable as _, Hsla, InteractiveElement as _, IntoElement,
    KeyBinding, ParentElement as _, PromptButton, PromptLevel, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, ease_out_quint,
    prelude::FluentBuilder as _, px, relative, rgb, rgba,
};
use gpui_kit as gpui;
use gpui_kit::component::{
    Disableable as _, Icon, IconName, IndexPath, InteractiveElementExt as _, Selectable as _,
    Sizable as _, WindowExt as _,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    input::{Enter, Input, InputEvent, InputState, Textarea, TextareaState},
    list::{List, ListDelegate, ListEvent, ListItem, ListState},
    menu::PopupMenuItem,
    popover::Popover,
    radio::Radio,
    searchable_list::SearchableListItem,
    switch::Switch,
    text::{TextView, TextViewStyle},
    tooltip::Tooltip,
};
use model_picker::{CatalogModelChoice, CatalogModelSelectContent};
use nexus_domain::{
    HarnessKind, Message, MessageKind, MessageRole, ModelDescriptor, PermissionMode, Project,
    ProviderProfile, RunStatus, ThinkingEffort, UserAskAnswerMode, UserAskAnswerValue,
};
use pane::{PaneKind, WorkspacePane};
use settings::SettingsSection;
use sidebar::navigation_row;
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    time::{Duration, Instant},
};
use theme::*;
use uuid::Uuid;

gpui::actions!(
    nexus_view,
    [
        SearchSessions,
        NewTask,
        ToggleSettings,
        CloseReview,
        HideWindow,
        ToggleWindow,
        QuitApp
    ]
);

// Render outside NexusView's update so dialog builders can read its current model.
struct DialogLayer;

impl Render for DialogLayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .inset_0()
            .children(gpui_kit::component::Root::render_dialog_layer(window, cx))
    }
}

pub(crate) struct NexusView {
    pdf_events: (
        std::sync::mpsc::Sender<crate::infrastructure::pdf::PdfEvent>,
        std::sync::mpsc::Receiver<crate::infrastructure::pdf::PdfEvent>,
    ),
    presenter: Presenter,
    prompt_input: Entity<TextareaState>,
    voice_key_input: Entity<InputState>,
    font_controls: fonts::FontControls,
    user_ask_inputs: BTreeMap<(Uuid, String), Entity<TextareaState>>,
    model_picker: model_picker::ModelPickerControl,
    generation_pickers: BTreeMap<GenerationKind, model_picker::ModelPickerControl>,
    commit_inputs: BTreeMap<Uuid, Entity<TextareaState>>,
    review_pages: BTreeMap<Uuid, review::ReviewPage>,
    executable_input: Entity<InputState>,
    provider_form: provider_profiles::ProviderProfileForm,
    search_input: Entity<InputState>,
    project_search_input: Entity<InputState>,
    project_picker_open: bool,
    issue_launch: Option<issues::IssueLaunch>,
    issue_launch_input: Entity<TextareaState>,
    focus_handle: FocusHandle,
    timeline_scroll: ScrollHandle,
    sidebar_scroll: ScrollHandle,
    settings_scroll: ScrollHandle,
    issues_scroll: ScrollHandle,
    issue_detail_scroll: ScrollHandle,
    sidebar_pane: Entity<WorkspacePane>,
    timeline_pane: Entity<WorkspacePane>,
    settings_pane: Entity<WorkspacePane>,
    expanded_messages: HashSet<ElementId>,
    collapsed_projects: HashSet<Uuid>,
    settings_open: bool,
    settings_section: SettingsSection,
    reduced_motion: bool,
    approval_dialog: Option<(Uuid, Uuid)>,
    dialog_layer: Entity<DialogLayer>,
}

impl NexusView {
    pub(crate) fn new(presenter: Presenter, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let locale = presenter.model().language;
        gpui_kit::component::set_locale(locale.as_str());
        configure_fonts(&presenter.model().fonts, cx);
        let font_controls = Self::new_font_controls(&presenter.model().fonts, locale, window, cx);
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
                .default_value(presenter.model().conversation.executable.clone())
                .placeholder(locale.text("命令名或完整路径"))
        });
        let provider_form = provider_profiles::ProviderProfileForm::new(
            presenter.model().selected_provider_profile(),
            presenter.model().conversation.selected_harness,
            locale,
            window,
            cx,
        );
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(locale.text("搜索任务…")));
        let project_search_input = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe(&project_search_input, |_, _, _: &InputEvent, cx| {
            cx.notify()
        })
        .detach();
        let issue_launch_input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(4, 10));
        cx.subscribe(&issue_launch_input, |_, _, event: &InputEvent, cx| {
            if matches!(
                event,
                InputEvent::Change | InputEvent::Focus | InputEvent::Blur
            ) {
                cx.notify();
            }
        })
        .detach();
        let model_picker = model_picker::ModelPickerControl::new(
            CatalogModelSelectContent::from_model(presenter.model()),
            window,
            cx,
        );
        let generation_pickers = GenerationKind::ALL
            .into_iter()
            .map(|kind| {
                let content =
                    CatalogModelSelectContent::from_generation_settings(presenter.model(), kind);
                let picker = model_picker::ModelPickerControl::new(content, window, cx);
                cx.subscribe(&picker.list, move |app, list, event: &ListEvent, cx| {
                    match event {
                        ListEvent::Confirm(index) => {
                            let choice = list
                                .read(cx)
                                .delegate()
                                .item(*index)
                                .filter(|item| !item.disabled)
                                .map(|item| item.choice.clone());
                            let selected = match choice {
                                Some(CatalogModelChoice::FollowDefault) => None,
                                Some(CatalogModelChoice::Model(id)) => Some(id),
                                _ => return,
                            };
                            if app.presenter.select_generation_model(kind, selected) {
                                app.generation_pickers.get_mut(&kind).unwrap().open = false;
                            }
                        }
                        ListEvent::Cancel => {
                            app.generation_pickers.get_mut(&kind).unwrap().open = false
                        }
                        _ => return,
                    }
                    cx.notify();
                })
                .detach();
                (kind, picker)
            })
            .collect();
        cx.subscribe(&prompt_input, |_, _, event: &InputEvent, cx| {
            if matches!(
                event,
                InputEvent::Change | InputEvent::Focus | InputEvent::Blur
            ) {
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(&search_input, |_, _, _: &InputEvent, cx| {
            cx.notify();
        })
        .detach();
        cx.subscribe_in(
            &model_picker.list,
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
                        app.model_picker.open = false;
                    }
                    ListEvent::Cancel => app.model_picker.open = false,
                    _ => return,
                }
                cx.notify();
            },
        )
        .detach();
        let keys = vec![
            KeyBinding::new("secondary-k", SearchSessions, Some("Nexus")),
            KeyBinding::new("secondary-n", NewTask, Some("Nexus")),
            KeyBinding::new("secondary-,", ToggleSettings, Some("Nexus")),
            KeyBinding::new("escape", CloseReview, Some("Nexus")),
        ];
        // ⌘W hides the window and ⌘Q quits the application; both are macOS
        // conventions the platform expects to find in the menu bar.
        #[cfg(target_os = "macos")]
        let keys = keys.into_iter().chain([
            KeyBinding::new("secondary-w", HideWindow, Some("Nexus")),
            KeyBinding::new("secondary-q", QuitApp, Some("Nexus")),
        ]);
        cx.bind_keys(keys);
        // Must follow `bind_keys`: the platform derives each menu item's key
        // equivalent from the binding registered for its action.
        #[cfg(target_os = "macos")]
        cx.set_menus(lifecycle::menu_bar(locale));
        let owner = cx.weak_entity();
        let sidebar_pane = cx.new(|cx| WorkspacePane::new(owner.clone(), PaneKind::Sidebar, cx));
        let timeline_pane = cx.new(|cx| WorkspacePane::new(owner.clone(), PaneKind::Timeline, cx));
        let dialog_layer = cx.new(|cx| {
            if let Some(owner) = owner.upgrade() {
                cx.observe(&owner, |_, _, cx| cx.notify()).detach();
            }
            DialogLayer
        });
        let settings_pane = cx.new(|cx| WorkspacePane::new(owner, PaneKind::Settings, cx));
        let settings_open = matches!(
            presenter.model().updates.state,
            crate::model::updates::UpdateState::Failed(_)
        );
        let mut view = Self {
            pdf_events: std::sync::mpsc::channel(),
            voice_key_input: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("MiMo API Key")
            }),
            presenter,
            font_controls,
            prompt_input,
            user_ask_inputs: BTreeMap::new(),
            model_picker,
            generation_pickers,
            commit_inputs: BTreeMap::new(),
            review_pages: BTreeMap::new(),
            executable_input,
            provider_form,
            search_input,
            project_search_input,
            project_picker_open: false,
            issue_launch: None,
            issue_launch_input,
            focus_handle: cx.focus_handle(),
            timeline_scroll: ScrollHandle::new(),
            sidebar_scroll: ScrollHandle::new(),
            settings_scroll: ScrollHandle::new(),
            issues_scroll: ScrollHandle::new(),
            issue_detail_scroll: ScrollHandle::new(),
            sidebar_pane,
            timeline_pane,
            settings_pane,
            expanded_messages: HashSet::new(),
            collapsed_projects: HashSet::new(),
            settings_open,
            settings_section: SettingsSection::General,
            reduced_motion: false,
            approval_dialog: None,
            dialog_layer,
        };
        view.refresh_appearance(window, cx);
        view.focus_handle.focus(window, cx);
        view.start_event_pump(window, cx);
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
            self.sync_font_controls(window, cx);
            self.provider_form.set_language(language, window, cx);
            self.prompt_input.update(cx, |input, cx| {
                input.set_placeholder(
                    language.text("描述一个目标，让 Agent 开始工作…"),
                    window,
                    cx,
                );
            });
            for (input, placeholder) in [
                (&self.executable_input, "命令名或完整路径"),
                (&self.search_input, "搜索任务…"),
            ] {
                input.update(cx, |input, cx| {
                    input.set_placeholder(language.text(placeholder), window, cx);
                });
            }
            for input in self.user_ask_inputs.values() {
                input.update(cx, |input, cx| {
                    input.set_placeholder(language.text("输入回答…"), window, cx);
                });
            }
            self.sync_catalog_model_select(window, cx);
            // The menu bar and the status item are native and keep their own
            // copies of every label.
            #[cfg(target_os = "macos")]
            {
                cx.set_menus(lifecycle::menu_bar(language));
                crate::infrastructure::status_item::sync_titles(language);
            }
            window.refresh();
        }
        cx.notify();
    }

    fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_open = !self.settings_open;
        if self.settings_open {
            if matches!(
                self.presenter.model().conversation.title_model_catalog,
                ModelCatalogState::Idle
            ) {
                self.presenter
                    .refresh_generation_model_catalog(GenerationKind::Title);
            }
            self.focus_handle.focus(window, cx);
        } else {
            self.focus_prompt(window, cx);
        }
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
        self.presenter.scan_harness_installations();
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    fn sync_executable(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.executable_input.update(cx, |input, cx| {
            input.set_value(&self.presenter.model().conversation.executable, window, cx)
        });
    }
}

impl Render for NexusView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_user_ask_inputs(window, cx);
        self.sync_commit_input(window, cx);
        if self.model_picker.open && self.presenter.model().conversation.active_run.is_some() {
            self.model_picker.open = false;
            self.prompt_input
                .update(cx, |input, cx| input.focus(window, cx));
        }
        self.sync_catalog_model_select(window, cx);
        let colors = palette(cx);
        let issue_launch = self
            .issue_launch
            .map(|_| self.render_issue_launch(window, cx));
        let element = div()
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
            .on_action(cx.listener(|app, _: &CloseReview, window, cx| {
                if !app.settings_open
                    && app.presenter.model().opened_issues().is_none()
                    && app
                        .review_pages
                        .contains_key(&app.presenter.model().conversation.id)
                {
                    app.close_workspace_review(window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &QuitApp, window, cx| {
                app.quit_application(window, cx);
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
            .child(self.dialog_layer.clone())
            .when_some(issue_launch, |element, overlay| element.child(overlay));
        // The status item and the menu bar reach the window here, so ⌘W and the
        // toggle share one hidden/restore implementation.
        #[cfg(target_os = "macos")]
        let element = element
            .on_action(cx.listener(|app, _: &HideWindow, _, cx| app.hide_window(cx)))
            .on_action(
                cx.listener(|app, _: &ToggleWindow, window, cx| app.toggle_window(window, cx)),
            );
        element
    }
}

fn working_directory_label(model: &AppModel) -> &str {
    if model.conversation.selected_project.is_none() {
        return model.language.text("未关联项目");
    }
    match model.working_directory() {
        Some(path) => Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(path),
        None => model.language.text("未选择目录"),
    }
}

#[cfg(test)]
mod tests;
