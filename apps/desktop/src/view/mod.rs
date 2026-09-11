mod cnb;
mod cnb_media;
mod components;
mod fonts;
mod model_picker;
mod pane;
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
        ThemePreference, UserAskSubmissionState,
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
use model_picker::{CatalogModelChoice, CatalogModelSelectContent, ModelPickerList};
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
    [SearchSessions, NewTask, ToggleSettings, CloseReview]
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

struct GenerationPicker {
    list: Entity<ListState<ModelPickerList>>,
    content: CatalogModelSelectContent,
    open: bool,
}

pub(crate) struct NexusView {
    presenter: Presenter,
    prompt_input: Entity<TextareaState>,
    voice_key_input: Entity<InputState>,
    font_controls: fonts::FontControls,
    user_ask_inputs: BTreeMap<(Uuid, String), Entity<TextareaState>>,
    catalog_model_select: Entity<ListState<ModelPickerList>>,
    catalog_model_select_content: CatalogModelSelectContent,
    model_picker_open: bool,
    generation_pickers: BTreeMap<GenerationKind, GenerationPicker>,
    commit_inputs: BTreeMap<Uuid, Entity<TextareaState>>,
    review_pages: BTreeMap<Uuid, review::ReviewPage>,
    executable_input: Entity<InputState>,
    provider_name_input: Entity<InputState>,
    provider_api_key_env_input: Entity<InputState>,
    provider_api_key_input: Entity<InputState>,
    provider_base_url_env_input: Entity<InputState>,
    provider_base_url_input: Entity<InputState>,
    provider_model_input: Entity<InputState>,
    search_input: Entity<InputState>,
    project_search_input: Entity<InputState>,
    project_picker_open: bool,
    focus_handle: FocusHandle,
    timeline_scroll: ScrollHandle,
    sidebar_scroll: ScrollHandle,
    settings_scroll: ScrollHandle,
    cnb_scroll: ScrollHandle,
    cnb_detail_scroll: ScrollHandle,
    sidebar_pane: Entity<WorkspacePane>,
    timeline_pane: Entity<WorkspacePane>,
    settings_pane: Entity<WorkspacePane>,
    expanded_messages: HashSet<ElementId>,
    collapsed_projects: HashSet<Uuid>,
    settings_open: bool,
    settings_section: SettingsSection,
    reduced_motion: bool,
    editing_provider_profile: Option<Uuid>,
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
            cx.new(|cx| InputState::new(window, cx).placeholder(locale.text("搜索任务…")));
        let project_search_input = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe(&project_search_input, |_, _, _: &InputEvent, cx| {
            cx.notify()
        })
        .detach();
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
        let generation_pickers = GenerationKind::ALL
            .into_iter()
            .map(|kind| {
                let content =
                    CatalogModelSelectContent::from_generation_settings(presenter.model(), kind);
                let list = cx.new(|cx| {
                    ListState::new(ModelPickerList::new(content.clone()), window, cx)
                        .searchable(true)
                });
                cx.subscribe(&list, move |app, list, event: &ListEvent, cx| {
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
                (
                    kind,
                    GenerationPicker {
                        list,
                        content,
                        open: false,
                    },
                )
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
            KeyBinding::new("escape", CloseReview, Some("Nexus")),
        ]);
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
            voice_key_input: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("MiMo API Key")
            }),
            presenter,
            font_controls,
            prompt_input,
            user_ask_inputs: BTreeMap::new(),
            catalog_model_select,
            catalog_model_select_content,
            model_picker_open: false,
            generation_pickers,
            commit_inputs: BTreeMap::new(),
            review_pages: BTreeMap::new(),
            executable_input,
            provider_name_input,
            provider_api_key_env_input,
            provider_api_key_input,
            provider_base_url_env_input,
            provider_base_url_input,
            provider_model_input,
            search_input,
            project_search_input,
            project_picker_open: false,
            focus_handle: cx.focus_handle(),
            timeline_scroll: ScrollHandle::new(),
            sidebar_scroll: ScrollHandle::new(),
            settings_scroll: ScrollHandle::new(),
            cnb_scroll: ScrollHandle::new(),
            cnb_detail_scroll: ScrollHandle::new(),
            sidebar_pane,
            timeline_pane,
            settings_pane,
            expanded_messages: HashSet::new(),
            collapsed_projects: HashSet::new(),
            settings_open,
            settings_section: SettingsSection::General,
            reduced_motion: false,
            editing_provider_profile,
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
            window.refresh();
        }
        cx.notify();
    }

    fn sync_user_ask_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let desired = self
            .presenter
            .model()
            .pending_user_asks
            .iter()
            .flat_map(|request| {
                request.questions.iter().filter_map(move |question| {
                    let accepts_text = matches!(
                        question.answer_mode,
                        UserAskAnswerMode::Text
                            | UserAskAnswerMode::Choice {
                                allow_custom: true,
                                ..
                            }
                    );
                    accepts_text.then(|| {
                        let value = match request.drafts.get(&question.id) {
                            Some(UserAskAnswerValue::Text(value)) => value.clone(),
                            _ => String::new(),
                        };
                        ((request.request_id, question.id.clone()), value)
                    })
                })
            })
            .collect::<BTreeMap<_, _>>();
        self.user_ask_inputs
            .retain(|key, _| desired.contains_key(key));
        let placeholder = self.presenter.model().language.text("输入回答…");
        for (key, value) in desired {
            if self.user_ask_inputs.contains_key(&key) {
                continue;
            }
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(2, 4)
                    .default_value(value)
                    .placeholder(placeholder)
            });
            let request_id = key.0;
            let question_id = key.1.clone();
            cx.subscribe(&input, move |app, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = input.read(cx).value().to_string();
                    app.presenter
                        .set_user_ask_text(request_id, &question_id, value);
                }
                cx.notify();
            })
            .detach();
            self.user_ask_inputs.insert(key, input);
        }
    }

    fn start_event_pump(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                if this
                    .update_in(cx, |app, window, cx| {
                        let executable = app.presenter.model().executable.clone();
                        let untouched = app.executable_input.read(cx).value() == executable;
                        app.poll_events(Instant::now(), cx);
                        app.poll_voice_input(window, cx);
                        if untouched && app.presenter.model().executable != executable {
                            app.sync_executable(window, cx);
                        }
                        app.sync_approval_dialog(window, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn sync_approval_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.presenter.model();
        let next = model.active_run.zip(
            model
                .pending_approvals
                .front()
                .map(|request| request.request_id),
        );
        if next == self.approval_dialog {
            return;
        }
        if self.approval_dialog.take().is_some() {
            window.close_dialog(cx);
        }
        let Some((run_id, request_id)) = next else {
            return;
        };
        self.approval_dialog = next;
        self.model_picker_open = false;
        let app = cx.entity().clone();
        window.open_dialog(cx, move |dialog, window, cx| {
            let model = app.read(cx).presenter.model();
            let locale = model.language;
            let Some(request) = model
                .pending_approvals
                .front()
                .filter(|request| request.request_id == request_id)
            else {
                return dialog;
            };
            let responding = model.responding_approval == Some(request_id);
            let title = model
                .tasks
                .iter()
                .find(|task| Some(task.id) == model.active_task)
                .map(|task| task.title.as_str())
                .unwrap_or_default();
            let mut buttons = div().flex().flex_wrap().gap_2();
            for (index, label) in request.options.iter().enumerate() {
                let app = app.clone();
                let label = match label.as_str() {
                    "Approve" => locale.text("允许本次").to_owned(),
                    "Deny" => locale.text("拒绝").to_owned(),
                    _ => label.clone(),
                };
                buttons = buttons.child(
                    Button::new(("approval-option", index))
                        .debug_selector(move || format!("approval-option-{index}"))
                        .label(label)
                        .disabled(responding)
                        .on_click(move |_, window, cx| {
                            app.update(cx, |app, cx| {
                                app.presenter
                                    .respond_approval(run_id, request_id, Some(index));
                                app.presenter.notify_remote_changed();
                                cx.notify();
                            });
                            window.refresh();
                        }),
                );
            }
            let stop_app = app.clone();
            buttons = buttons.child(
                Button::new("approval-stop")
                    .label(locale.text("停止任务"))
                    .ghost()
                    .on_click(move |_, window, cx| {
                        stop_app.update(cx, |app, cx| {
                            app.presenter.cancel();
                            app.presenter.notify_remote_changed();
                            cx.notify();
                        });
                        window.refresh();
                    }),
            );
            dialog
                .title(locale.text("需要授权"))
                .width(px(640.).min(window.viewport_size().width - px(48.)))
                .close_button(false)
                .overlay_closable(false)
                .keyboard(false)
                .on_ok(|_, _, _| false)
                .child(div().text_size(px(13.)).child(title.to_owned()))
                .child(div().child(request.title.clone()))
                .child(
                    div()
                        .id("approval-details")
                        .debug_selector(|| "approval-details".into())
                        .max_h(window.viewport_size().height * 0.45)
                        .overflow_y_scroll()
                        .text_size(px(13.))
                        .child(request.details.clone()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .child(model.run_status.render(model.language).to_owned()),
                )
                .footer(buttons)
        });
    }

    fn poll_events(&mut self, now: Instant, cx: &mut Context<Self>) {
        if self.presenter.drain_cnb_events() {
            cx.notify();
        }
        if self.presenter.drain_installation_events() {
            cx.notify();
        }
        if self.presenter.drain_update_events() {
            cx.notify();
        }
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
        if self.presenter.install_update_when_idle() {
            cx.notify();
        }
        if matches!(
            self.presenter.model().updates.state,
            crate::model::updates::UpdateState::Restarting(_)
        ) {
            self.presenter.shutdown_for_update();
            cx.quit();
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
        self.presenter.new_task();
        self.settings_open = false;
        self.expanded_messages.clear();
        self.focus_prompt(window, cx);
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    fn new_projectless_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.presenter.new_projectless_task();
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

    fn confirm_delete_project(
        &mut self,
        project_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.presenter.can_delete_project(project_id) {
            return;
        }
        let model = self.presenter.model();
        let Some(project) = model
            .projects
            .iter()
            .find(|project| project.id == project_id)
        else {
            return;
        };
        let locale = model.language;
        let message = locale.format(
            "删除项目“{title}”？",
            &[("title", project.display_name.clone())],
        );
        let answer = window.prompt(
            PromptLevel::Critical,
            &message,
            Some(locale.text(
                "此操作会永久删除 Nexus 中该项目的全部对话（含归档）、消息、运行和工作区记录，且无法撤销。磁盘上的项目目录、Worktree、文件和 Git 分支不会删除。",
            )),
            &[
                PromptButton::ok(locale.text("删除项目")),
                PromptButton::cancel(locale.text("取消")),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() == Some(0) {
                let _ = this.update_in(cx, |app, window, cx| {
                    app.delete_project(project_id, window, cx)
                });
            }
        })
        .detach();
    }

    fn delete_project(&mut self, project_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self
            .presenter
            .model()
            .selected_project
            .as_ref()
            .is_some_and(|project| project.id == project_id);
        if self.presenter.delete_project(project_id) {
            self.collapsed_projects.remove(&project_id);
            if selected {
                self.expanded_messages.clear();
                self.timeline_scroll.scroll_to_bottom();
                self.focus_handle.focus(window, cx);
            }
        }
        self.presenter.notify_remote_changed();
        cx.notify();
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
            self.presenter.cancel_voice();
            self.prompt_input
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.expanded_messages.clear();
            self.timeline_scroll.scroll_to_bottom();
        }
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    fn focus_prompt(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt_input
            .update(cx, |input, cx| input.focus(window, cx));
    }

    fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_open = !self.settings_open;
        if self.settings_open {
            if matches!(
                self.presenter.model().title_model_catalog,
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

    fn cancel(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.presenter.cancel();
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    fn select_user_ask_question(&mut self, request_id: Uuid, index: usize, cx: &mut Context<Self>) {
        if self.presenter.set_user_ask_question(request_id, index) {
            cx.notify();
        }
    }

    fn toggle_user_ask(&mut self, request_id: Uuid, cx: &mut Context<Self>) {
        if self.presenter.toggle_user_ask_collapsed(request_id) {
            cx.notify();
        }
    }

    fn select_user_ask_option(
        &mut self,
        request_id: Uuid,
        question_id: &str,
        option_id: &str,
        checked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .presenter
            .set_user_ask_option(request_id, question_id, option_id, checked)
        {
            return;
        }
        if checked
            && let Some(input) = self
                .user_ask_inputs
                .get(&(request_id, question_id.to_owned()))
        {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        cx.notify();
    }

    fn submit_user_ask(&mut self, request_id: Uuid, cx: &mut Context<Self>) {
        if self.presenter.submit_user_ask(request_id) {
            self.presenter.notify_remote_changed();
            cx.notify();
        }
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
        for kind in GenerationKind::ALL {
            let content =
                CatalogModelSelectContent::from_generation_settings(self.presenter.model(), kind);
            let picker = self.generation_pickers.get_mut(&kind).unwrap();
            if content != picker.content {
                picker.content = content.clone();
                picker.list.update(cx, |state, cx| {
                    state.delegate_mut().replace_content(content);
                    let selected = state.delegate().selected_index();
                    state.set_selected_index(selected, window, cx);
                    cx.notify();
                });
            }
        }
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
            .disabled(model.active_run.is_some())
            .small()
            .when(compact, |button| {
                button
                    .ghost()
                    .h(px(COMPACT_CONTROL_HEIGHT))
                    .max_w(px(180.))
                    .label(selected.to_string())
            })
            .when(!compact, |button| {
                button
                    .debug_selector(move || id.into())
                    .outline()
                    .w_full()
                    .h(px(CONTROL_HEIGHT))
                    .accessibility_label(selected.to_string())
                    .child(settings::control_label(selected.to_string()))
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
            .disabled(model.active_run.is_some())
            .small()
            .when(compact, |button| {
                button
                    .ghost()
                    .h(px(COMPACT_CONTROL_HEIGHT))
                    .max_w(px(180.))
                    .label(selected_name.clone())
            })
            .when(!compact, |button| {
                button
                    .debug_selector(move || id.into())
                    .outline()
                    .w_full()
                    .h(px(CONTROL_HEIGHT))
                    .accessibility_label(selected_name.clone())
                    .child(settings::control_label(selected_name))
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

    fn permission_selector(&self, cx: &mut Context<Self>) -> AnyElement {
        let model = self.presenter.model();
        let locale = model.language;
        let selected = model.permission_mode;
        let app = cx.entity().clone();
        let button_id = "composer-permissions";
        let button = Button::new(button_id)
            .debug_selector(|| "composer-permissions".into())
            .ghost()
            .small()
            .h(px(COMPACT_CONTROL_HEIGHT))
            .label(locale.permission_mode(selected))
            .tooltip(locale.text("设置下一条消息的权限，已开始的轮次保持原权限。"))
            .disabled(model.active_run.is_some() && !model.can_queue());
        AnimatedDropdown::new(button_id, button, self.reduced_motion, move |menu, _, _| {
            PermissionMode::ALL
                .into_iter()
                .fold(menu.min_w(px(160.)), |menu, mode| {
                    let app = app.clone();
                    menu.item(
                        PopupMenuItem::new(locale.permission_mode(mode))
                            .checked(mode == selected)
                            .on_click(move |_, _, cx| {
                                app.update(cx, |app, cx| {
                                    app.presenter.select_permission_mode(mode);
                                    cx.notify();
                                });
                            }),
                    )
                })
        })
        .into_any_element()
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
                        .debug_selector(|| "message-queue".into())
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
                                        .child(format!(
                                            "{} · {}",
                                            locale.permission_mode(message.permission_mode),
                                            message.prompt
                                        )),
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
                                                    || model.steering_message.is_some()
                                                    || model.active_permission_mode
                                                        != Some(message.permission_mode),
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

    fn render_working_directory(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let colors = palette(cx);
        let path = model.working_directory().map(str::to_owned);
        let app = cx.entity();
        div()
            .debug_selector(|| "composer-context".into())
            .w_full()
            .max_w(px(CONTENT_WIDTH))
            .mx_auto()
            .mb_2()
            .px_3()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                // Recreate tooltip state when switching to a different execution directory.
                div()
                    .id(SharedString::from(format!(
                        "composer-directory-{}",
                        path.as_deref().unwrap_or_default()
                    )))
                    .debug_selector(|| "composer-directory".into())
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(rgb(colors.muted))
                    .cursor_pointer()
                    .hover(|style| style.text_color(rgb(colors.text)))
                    .when_some(path, |element, path| {
                        element.tooltip(move |window, cx| {
                            let path = path.clone();
                            Tooltip::element(move |_, _| {
                                div()
                                    .debug_selector(|| "composer-directory-tooltip".into())
                                    .max_w(px(CONTENT_WIDTH))
                                    .whitespace_normal()
                                    .child(path.clone())
                            })
                            .build(window, cx)
                        })
                    })
                    .child(Icon::new(IconName::Folder).size(px(14.)).flex_none())
                    .child(
                        div()
                            .debug_selector(|| "composer-directory-name".into())
                            .min_w_0()
                            .truncate()
                            .child(working_directory_label(model).to_owned()),
                    )
                    .map(|trigger| {
                        Popover::new("project-picker")
                            .anchor(Anchor::BottomLeft)
                            .bottom_2()
                            .p_4()
                            .open(self.project_picker_open)
                            .track_focus(&self.project_search_input.focus_handle(cx))
                            .trigger(
                                Button::new("project-picker-trigger")
                                    .debug_selector(|| "project-picker-trigger".into())
                                    .ghost()
                                    .small()
                                    .h(px(COMPACT_CONTROL_HEIGHT))
                                    .min_w_0()
                                    .max_w_full()
                                    .accessibility_label(model.language.text("选择项目"))
                                    .child(trigger),
                            )
                            .on_open_change(move |open, window, cx| {
                                app.update(cx, |app, cx| {
                                    app.project_picker_open = *open;
                                    if *open {
                                        let locale = app.presenter.model().language;
                                        app.project_search_input.update(cx, |input, cx| {
                                            input.set_placeholder(
                                                locale.text("搜索项目"),
                                                window,
                                                cx,
                                            );
                                            input.set_value("", window, cx);
                                        });
                                    }
                                    cx.notify();
                                });
                            })
                            .when(self.project_picker_open, |popover| {
                                popover.child(self.render_project_picker(cx))
                            })
                            .map(|popover| div().min_w_0().child(popover))
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_none()
                    .gap_2()
                    .child(
                        self.working_directory_opener(
                            "composer-open-directory",
                            Button::new("composer-open-directory")
                                .debug_selector(|| "composer-open-directory".into())
                                .ghost()
                                .small()
                                .h(px(COMPACT_CONTROL_HEIGHT))
                                .label(model.language.text("打开方式")),
                            cx,
                        ),
                    )
                    .child(self.render_workspace_controls(cx)),
            )
    }

    fn render_project_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let locale = model.language;
        let colors = palette(cx);
        let query = self.project_search_input.read(cx).value();
        let selected = model.selected_project.as_ref().map(|project| project.id);
        let projects: Vec<_> = model
            .projects
            .iter()
            .filter(|project| matches_search(&project.display_name, &query))
            .collect();
        div()
            .debug_selector(|| "project-picker-surface".into())
            .w(px(280.))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .debug_selector(|| "project-picker-search".into())
                    .flex_none()
                    .child(
                        Input::new(&self.project_search_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.)),
                    ),
            )
            .child(
                div()
                    .id("project-picker-list")
                    .max_h(px(240.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        navigation_row(
                            colors,
                            "project-picker-none",
                            locale.text("未关联项目"),
                            None,
                        )
                        .debug_selector(|| "project-picker-none".into())
                        .selected(selected.is_none())
                        .on_click(cx.listener(
                            move |app, _, window, cx| {
                                app.project_picker_open = false;
                                if selected.is_some() {
                                    app.new_projectless_task(window, cx);
                                }
                                app.focus_prompt(window, cx);
                                cx.notify();
                            },
                        )),
                    )
                    .when(projects.is_empty(), |list| {
                        list.child(
                            div()
                                .p_2()
                                .text_color(rgb(colors.muted))
                                .child(locale.text("没有匹配的项目")),
                        )
                    })
                    .children(projects.into_iter().map(|project| {
                        let project = project.clone();
                        let id = project.id;
                        navigation_row(
                            colors,
                            ElementId::from(id),
                            project.display_name.clone(),
                            None,
                        )
                        .debug_selector(move || format!("project-picker-{id}"))
                        .flex_none()
                        .selected(selected == Some(id))
                        .when(selected == Some(id), |row| {
                            row.suffix(|_, _| Icon::new(IconName::Check).size(px(14.)))
                        })
                        .on_click(cx.listener(
                            move |app, _, window, cx| {
                                app.project_picker_open = false;
                                if selected != Some(id) {
                                    app.select_project(project.clone());
                                }
                                app.focus_prompt(window, cx);
                                cx.notify();
                            },
                        ))
                    })),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .pt_3()
                    .child(
                        navigation_row(
                            colors,
                            "project-picker-new",
                            locale.text("新建项目"),
                            Some(IconName::Plus),
                        )
                        .debug_selector(|| "project-picker-new".into())
                        .on_click(cx.listener(|app, event, window, cx| {
                            app.project_picker_open = false;
                            app.choose_project(event, window, cx);
                            cx.notify();
                        })),
                    ),
            )
    }

    fn render_workspace(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let page = if self.presenter.model().cnb.opened {
            Some(self.render_cnb(cx).into_any_element())
        } else {
            self.review_pages
                .get(&self.presenter.model().conversation.id)
                .map(|page| self.render_workspace_review(page, cx))
        };
        if let Some(page) = page {
            return div()
                .size_full()
                .bg(materials(cx).chrome)
                .flex()
                .child(
                    self.sidebar_pane.clone().cached(
                        gpui::StyleRefinement::default()
                            .w(px(SIDEBAR_WIDTH))
                            .h_full()
                            .flex_none(),
                    ),
                )
                .child(page)
                .into_any_element();
        }
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let material = materials(cx);
        let model = self.presenter.model();
        let voice_status = model.voice.status.render(locale);
        let can_submit = can_send_prompt(model, &self.prompt_input.read(cx).value());
        let prompt_focused = self
            .prompt_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let composer_hint = if model.active_run.is_some() {
            locale.text("Agent 正在执行 · 发送后排队，每轮结束后发送一条")
        } else if !model.can_submit() {
            locale.text("Agent 尚未就绪 · 打开设置检查探测和登录状态")
        } else if cfg!(target_os = "macos") {
            locale.text("⌘ Enter 发送消息 · Enter 换行")
        } else {
            locale.text("Ctrl Enter 发送消息 · Enter 换行")
        };
        let selected_task = model
            .selected_task
            .and_then(|task_id| model.tasks.iter().find(|task| task.id == task_id));
        let header_project = model
            .selected_project
            .as_ref()
            .map(|project| project.display_name.clone())
            .unwrap_or_else(|| locale.text("未关联项目").into());
        let header_project_tooltip = model.selected_project.as_ref().map_or_else(
            || header_project.clone(),
            |project| format!("{}\n{}", project.display_name, project.canonical_path),
        );
        let header_task = selected_task
            .map(|task| task.title.clone())
            .unwrap_or_else(|| locale.text("新建任务").into());
        div()
            .debug_selector(|| "workspace-page".into())
            .size_full()
            .bg(material.chrome)
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
                    .min_w_0()
                    .my_2()
                    .mr_2()
                    .rounded(px(16.))
                    .border_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.canvas))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .debug_selector(|| "workspace-header".into())
                            .h(px(HEADER_HEIGHT))
                            .flex_none()
                            .border_b(px(0.5))
                            .border_color(material.edge)
                            .px_5()
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
                                        Icon::new(IconName::Folder).text_color(rgb(colors.muted)),
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
                                    .map(|element| {
                                        let tooltip = header_task.clone();
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
                                                    .child(header_task),
                                            )
                                    }),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .pl_3()
                                    .when(model.selected_project.is_some(), |element| {
                                        element.child(
                                            Button::new("toggle-changes-sidebar")
                                                .debug_selector(|| "toggle-changes-sidebar".into())
                                                .ghost()
                                                .small()
                                                .icon(IconName::PanelRight)
                                                .tooltip(locale.text("环境"))
                                                .accessibility_label(locale.text("环境"))
                                                .selected(model.changes_sidebar_open)
                                                .on_click(cx.listener(|app, _, _, cx| {
                                                    app.presenter.toggle_changes_sidebar();
                                                    cx.notify();
                                                })),
                                        )
                                    })
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
                            .px(px(24.))
                            .pt_3()
                            .pb_4()
                            .child(self.render_user_asks(window, cx))
                            .child(self.render_working_directory(cx))
                            .child(self.render_workspace_hints(cx))
                            .child(
                                div()
                                    .debug_selector(|| "composer-surface".into())
                                    .relative()
                                    .w_full()
                                    .max_w(px(CONTENT_WIDTH))
                                    .mx_auto()
                                    .rounded(px(16.))
                                    .bg(material.floating)
                                    .border_1()
                                    .border_color(rgb(colors.input_border).opacity(0.45))
                                    .when(prompt_focused, |element| {
                                        element.border_color(rgb(colors.accent))
                                    })
                                    .shadow(material.shadow())
                                    .p_4()
                                    .flex()
                                    .flex_col()
                                    .child(self.render_message_queue(cx))
                                    .when(!voice_status.is_empty(), |element| {
                                        element.child(
                                            div()
                                                .mb_2()
                                                .text_size(px(12.))
                                                .text_color(rgb(colors.text_secondary))
                                                .child(voice_status.to_owned()),
                                        )
                                    })
                                    .child(
                                        Textarea::new(&self.prompt_input)
                                            .appearance(false)
                                            .bordered(false)
                                            .text_size(px(15.))
                                            .line_height(relative(1.65))
                                            .aria_label(locale.text("任务描述")),
                                    )
                                    .child(
                                        div()
                                            .min_h(px(COMPACT_CONTROL_HEIGHT))
                                            .mt_3()
                                            .pt_3()
                                            .border_t(px(0.5))
                                            .border_color(rgb(colors.border))
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
                                                    .gap_2()
                                                    .child(self.model_selector(window, cx))
                                                    .child(self.effort_selector(cx))
                                                    .child(self.permission_selector(cx)),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(self.render_voice_controls(cx))
                                                    .when(model.active_run.is_some(), |element| {
                                                        element.child(
                                                            Button::new("composer-cancel")
                                                                .debug_selector(|| {
                                                                    "composer-cancel".into()
                                                                })
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
                                                            .debug_selector(|| {
                                                                "composer-submit".into()
                                                            })
                                                            .primary()
                                                            .small()
                                                            .size(px(CONTROL_HEIGHT))
                                                            .rounded(px(10.))
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
                                    .mt_2()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors.muted))
                                            .child(composer_hint),
                                    )
                                    .when(
                                        model.selected_project.is_some()
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
            .when(model.changes_sidebar_open, |element| {
                element.child(self.render_changes_sidebar(cx))
            })
            .into_any_element()
    }
}

impl Render for NexusView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_user_ask_inputs(window, cx);
        self.sync_commit_input(window, cx);
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
            .on_action(cx.listener(|app, _: &CloseReview, window, cx| {
                if !app.settings_open
                    && !app.presenter.model().cnb.opened
                    && app
                        .review_pages
                        .contains_key(&app.presenter.model().conversation.id)
                {
                    app.close_workspace_review(window, cx);
                    cx.stop_propagation();
                }
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
    }
}

fn working_directory_label(model: &AppModel) -> &str {
    if model.selected_project.is_none() {
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

fn provider_environment_defaults(harness: HarnessKind) -> (&'static str, &'static str) {
    match harness {
        HarnessKind::Claude => ("ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL"),
        HarnessKind::Codex => ("CODEX_API_KEY", "OPENAI_BASE_URL"),
        HarnessKind::Omp => ("DEEPSEEK_API_KEY", ""),
        HarnessKind::Pi => ("ANTHROPIC_API_KEY", ""),
        HarnessKind::Kimi => ("KIMI_API_KEY", "KIMI_BASE_URL"),
        HarnessKind::Qoder => ("QODER_PERSONAL_ACCESS_TOKEN", ""),
        HarnessKind::QoderCn => ("QODERCN_PERSONAL_ACCESS_TOKEN", ""),
        HarnessKind::Codebuddy => ("CODEBUDDY_API_KEY", "CODEBUDDY_BASE_URL"),
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
    use nexus_domain::{ModelReasoningEffort, UserAskOption, UserAskQuestion};
    use nexus_protocol::Event;

    #[gpui::test]
    fn cnb_issue_actions_import_complete_context_and_offer_harness_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::{
            infrastructure::cnb::{ActionResult, Response},
            model::cnb::{IssueAction, IssueFilter},
            presenter::tests::{cnb_comment, cnb_issue, finish_cnb_request, seed_cnb_issues},
        };
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, _, _directory) = fixture();
        seed_cnb_issues(&mut presenter);
        presenter.open_cnb();
        presenter.select_cnb_issue("1".into());
        let mut issue = cnb_issue("1");
        issue.body = "需要实现的功能描述。".into();
        finish_cnb_request(&mut presenter, Response::Detail(Ok(issue.clone())));
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
        cx.simulate_resize(gpui::size(px(1120.), px(900.)));
        view.update_in(cx, |view, window, cx| {
            view.prompt_input
                .update(cx, |input, cx| input.set_value("保留已有草稿", window, cx));
        });
        cx.run_until_parked();
        click_debug(cx, "cnb-chat");
        view.update(cx, |view, _| assert!(view.presenter.model().cnb.opened));
        view.update(cx, |view, cx| {
            let mut comment = cnb_comment("1");
            comment.body = "完整验收条件，包含 **格式**。".into();
            finish_cnb_request(&mut view.presenter, Response::Comments(Ok(vec![comment])));
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("cnb-comment-1").is_some());
        click_debug(cx, "cnb-assign-self");
        view.update(cx, |view, cx| {
            assert_eq!(
                view.presenter.model().cnb.action_request.unwrap().1,
                IssueAction::AssignSelf
            );
            finish_cnb_request(
                &mut view.presenter,
                Response::Action(Ok(ActionResult::Updated(Box::new(issue.clone())))),
            );
            cx.notify();
        });
        cx.run_until_parked();
        for state in [IssueFilter::Closed, IssueFilter::Open] {
            click_debug(cx, "cnb-change-state");
            view.update(cx, |view, cx| {
                assert_eq!(
                    view.presenter.model().cnb.action_request.unwrap().1,
                    IssueAction::SetState(state)
                );
                issue.state = state.state().into();
                finish_cnb_request(
                    &mut view.presenter,
                    Response::Action(Ok(ActionResult::Updated(Box::new(issue.clone())))),
                );
                cx.notify();
            });
            cx.run_until_parked();
        }
        click_debug(cx, "cnb-npc");
        view.update(cx, |view, cx| {
            assert_eq!(view.presenter.model().cnb.action_request.unwrap().1, IssueAction::StartNpc);
            let comment = serde_json::from_value(serde_json::json!({"id":"987", "body":"@CodeBuddy 请处理", "statuses":{
                "npc":[{"statuses":[{"target_url":"https://cnb.cool/team/project/-/build/logs/cnb-1"}]}]
            }})).unwrap();
            finish_cnb_request(&mut view.presenter, Response::Action(Ok(ActionResult::Npc(comment))));
            let mut comment = cnb_comment("1");
            comment.body = "完整验收条件，包含 **格式**。".into();
            finish_cnb_request(&mut view.presenter, Response::Comments(Ok(vec![comment, cnb_comment("987")])));
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("cnb-npc-action").is_some());
        click_debug(cx, "cnb-chat");
        cx.run_until_parked();
        assert!(cx.debug_bounds("cnb-page").is_none());
        assert!(cx.debug_bounds("composer-surface").is_some());
        assert!(cx.debug_bounds("model-picker-surface").is_some());
        view.update(cx, |view, cx| {
            let draft = view.prompt_input.read(cx).value();
            assert!(draft.starts_with("保留已有草稿\n\n"));
            assert!(draft.contains("https://cnb.cool/team/project/-/issues/1"));
            assert!(draft.contains("需要实现的功能描述。"));
            assert!(draft.contains("完整验收条件，包含 **格式**。"));
            assert!(view.presenter.model().selected_task.is_none());
            assert!(view.presenter.model().active_run.is_none());
        });
    }

    #[gpui::test]
    fn cnb_navigation_renders_issues_details_and_pagination_without_a_composer(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::{
            infrastructure::cnb::Response,
            model::cnb::{IssueFilter, IssuePage},
            presenter::tests::{cnb_issue, finish_cnb_request, seed_cnb_issues},
        };
        // Media loading uses Tokio workers outside GPUI's deterministic test scheduler.
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, _, _directory) = fixture();
        let project = presenter.model().selected_project.as_ref().unwrap().id;
        seed_cnb_issues(&mut presenter);
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
        cx.simulate_resize(gpui::size(px(1040.), px(720.)));
        view.update_in(cx, |view, window, cx| {
            view.set_appearance(
                AppearanceSettings {
                    theme: ThemePreference::Light,
                    reduced_motion: true,
                    ..Default::default()
                },
                window,
                cx,
            );
            view.prompt_input
                .update(cx, |input, cx| input.set_value("保留会话草稿", window, cx));
        });
        cx.run_until_parked();
        let project_selector: &'static str = format!("sidebar-project-{project}").leak();
        let project_row = cx.debug_bounds(project_selector).unwrap();
        let cnb_row = cx.debug_bounds("sidebar-cnb").unwrap();
        assert!(cnb_row.top() >= project_row.bottom());
        assert_eq!(cnb_row.size.height, px(40.));
        click_debug(cx, "sidebar-cnb");
        cx.run_until_parked();
        assert!(cx.debug_bounds("cnb-page").is_some());
        assert!(cx.debug_bounds("cnb-issue-tab").is_some());
        let assert_breadcrumb = |cx: &mut gpui::VisualTestContext, segment_count| {
            let breadcrumb = cx.debug_bounds("cnb-breadcrumb").unwrap();
            let mut previous = cx.debug_bounds("cnb-breadcrumb-root").unwrap();
            let first_separator = cx.debug_bounds("cnb-breadcrumb-separator-0").unwrap();
            let gap = first_separator.left() - previous.right();
            assert!(gap > px(0.));
            for index in 0..segment_count {
                let separator = cx
                    .debug_bounds(format!("cnb-breadcrumb-separator-{index}").leak())
                    .unwrap();
                let segment = cx
                    .debug_bounds(format!("cnb-breadcrumb-segment-{index}").leak())
                    .unwrap();
                assert_eq!(separator.size, first_separator.size);
                assert_eq!(separator.left() - previous.right(), gap);
                assert_eq!(segment.left() - separator.right(), gap);
                assert_eq!(segment.center().y, previous.center().y);
                assert_eq!(segment.center().y, separator.center().y);
                assert!(segment.size.width > px(0.));
                assert!(segment.right() <= breadcrumb.right());
                previous = segment;
            }
            let repository_button = cx.debug_bounds("cnb-repository").unwrap();
            assert!(repository_button.left() >= breadcrumb.right());
            assert!(repository_button.right() < cx.debug_bounds("cnb-page").unwrap().right());
        };
        assert_breadcrumb(cx, 2);
        click_debug(cx, "cnb-repository");
        assert_eq!(
            cx.opened_url().as_deref(),
            Some("https://cnb.cool/team/project")
        );
        let icon = cx.debug_bounds("sidebar-cnb-icon").unwrap();
        assert!(icon.left() >= cnb_row.left() && icon.right() < cnb_row.left() + px(34.));
        assert!(cx.debug_bounds("composer-surface").is_none());
        let issue = cx.debug_bounds("cnb-issue-1").unwrap();
        let page = cx.debug_bounds("cnb-page").unwrap();
        assert!(issue.left() > page.left() && issue.right() < page.right());
        click_debug(cx, "cnb-issue-1");
        view.update(cx, |view, cx| {
            let mut issue = cnb_issue("1");
            issue.body = "截图说明\n\n![截图](https://example.test/screenshot.png)\n\n[录屏.mp4](undefined/team/repo/-/files/issues/1/clip.mp4)\n\nhttps://example.test/sound.mp3\n\n[普通链接](https://example.test/page)\n\n```text\nhttps://example.test/code.mp4\n```".into();
            finish_cnb_request(&mut view.presenter, Response::Detail(Ok(issue)));
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("cnb-detail").is_some());
        assert!(cx.debug_bounds("cnb-issue-body").is_some());
        assert!(cx.debug_bounds("cnb-media-image").is_some());
        assert!(cx.debug_bounds("cnb-media-video").is_some());
        assert!(cx.debug_bounds("cnb-media-audio").is_some());
        click_debug(cx, "cnb-back");
        cx.run_until_parked();
        assert!(cx.debug_bounds("cnb-issue-1").is_some());
        assert!(cx.debug_bounds("cnb-media-video").is_none());
        click_debug(cx, "cnb-next");
        view.update(cx, |view, cx| {
            assert_eq!(view.presenter.model().cnb.page, 2);
            finish_cnb_request(
                &mut view.presenter,
                Response::List(Ok(IssuePage {
                    issues: vec![cnb_issue("31")],
                    total: 61,
                })),
            );
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("cnb-issue-31").is_some());
        click_debug(cx, "cnb-filter-closed");
        view.update_in(cx, |view, window, cx| {
            assert_eq!(view.presenter.model().cnb.filter, IssueFilter::Closed);
            assert_eq!(view.presenter.model().cnb.page, 1);
            finish_cnb_request(
                &mut view.presenter,
                Response::List(Ok(IssuePage {
                    issues: vec![],
                    total: 0,
                })),
            );
            view.set_appearance(
                AppearanceSettings {
                    theme: ThemePreference::Dark,
                    reduced_motion: true,
                    ..Default::default()
                },
                window,
                cx,
            );
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("cnb-empty").is_some());
        let nested_repository = "organization-with-a-long-name/subgroup/project-with-a-long-name";
        view.update(cx, |view, cx| {
            let cli = view.presenter.model().cnb.cli.clone().unwrap();
            view.presenter.inspect_cnb();
            finish_cnb_request(
                &mut view.presenter,
                Response::Inspection {
                    repository: Some(nested_repository.into()),
                    cli: Ok(cli),
                },
            );
            view.presenter.open_cnb();
            cx.notify();
        });
        cx.simulate_resize(gpui::size(px(760.), px(720.)));
        cx.run_until_parked();
        assert_breadcrumb(cx, 3);
        click_debug(cx, "cnb-repository");
        assert_eq!(
            cx.opened_url(),
            Some(format!("https://cnb.cool/{nested_repository}"))
        );
        view.update_in(cx, |view, window, cx| {
            view.new_task(window, cx);
            assert_eq!(view.prompt_input.read(cx).value(), "保留会话草稿");
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("cnb-page").is_none());
        assert!(cx.debug_bounds("composer-surface").is_some());
        view.update_in(cx, |view, window, cx| {
            view.settings_open = true;
            view.select_settings_section(SettingsSection::SourceControl, window, cx);
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("settings-nav-source-control").is_some());
        assert!(cx.debug_bounds("cnb-enabled").is_some());
    }

    #[gpui::test]
    fn voice_settings_select_before_configure_and_draft_insertion_is_undoable(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::infrastructure::voice::Provider;
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (presenter, _, _directory) = fixture();
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
        cx.simulate_resize(gpui::size(px(1040.), px(680.)));
        view.update_in(cx, |view, window, cx| {
            view.set_language(Language::English, window, cx);
        });
        cx.run_until_parked();
        let voice = cx.debug_bounds("voice-record").unwrap();
        let submit = cx.debug_bounds("composer-submit").unwrap();
        let composer = cx.debug_bounds("composer-surface").unwrap();
        assert_eq!(voice.center().y, submit.center().y);
        assert!(voice.left() >= composer.left() && voice.right() <= submit.left());
        assert!(voice.top() >= composer.top() && voice.bottom() <= composer.bottom());
        click_debug(cx, "voice-record");
        cx.run_until_parked();
        assert!(cx.debug_bounds("voice-settings").is_some());
        assert!(cx.debug_bounds("settings-nav-voice").is_some());
        assert_eq!(
            Language::English.text("配置语音输入"),
            "Configure voice input"
        );
        assert!(cx.debug_bounds("voice-mimo-config").is_none());
        #[cfg(target_os = "macos")]
        {
            view.update(cx, |view, cx| {
                view.presenter
                    .select_voice_provider(Provider::MacOs)
                    .unwrap();
                cx.notify();
            });
            cx.run_until_parked();
            let trigger = cx.debug_bounds("voice-locale").unwrap();
            let position = gpui::point(trigger.left() + px(16.), trigger.center().y);
            cx.simulate_click(position, Default::default());
            cx.run_until_parked();
            let settings_offset = view.read_with(cx, |view, _| view.settings_scroll.offset());
            cx.simulate_event(gpui::ScrollWheelEvent {
                position,
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-10000.))),
                ..Default::default()
            });
            cx.run_until_parked();
            cx.simulate_click(position, Default::default());
            cx.run_until_parked();
            view.read_with(cx, |view, _| {
                let locales = crate::infrastructure::voice::native_locales();
                let selected = view
                    .presenter
                    .model()
                    .voice
                    .settings
                    .locale
                    .as_ref()
                    .unwrap();
                assert!(
                    locales[locales.len() / 2..].contains(selected),
                    "scrolling the language menu should reach its lower entries: {selected}"
                );
                assert_eq!(view.settings_scroll.offset(), settings_offset);
            });
        }
        view.update(cx, |view, cx| {
            view.presenter
                .select_voice_provider(Provider::Mimo)
                .unwrap();
            cx.notify();
        });
        cx.run_until_parked();
        let bounds = cx.debug_bounds("voice-mimo-config").unwrap();
        assert!(bounds.left() >= px(0.) && bounds.right() <= px(1040.));
        view.update_in(cx, |view, window, cx| {
            assert!(!view.presenter.model().voice.ready());
            view.settings_open = false;
            view.prompt_input.update(cx, |input, cx| {
                input.set_value("已有草稿：用户刚编辑", window, cx)
            });
            view.append_voice_text("检查 src/main.rs 与 parseHTTP", window, cx);
            assert_eq!(
                view.prompt_input.read(cx).value(),
                "已有草稿：用户刚编辑\n检查 src/main.rs 与 parseHTTP"
            );
            assert!(view.presenter.model().queued_messages.is_empty());
            assert!(view.presenter.model().active_run.is_none());
            view.focus_prompt(window, cx);
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("voice-record").is_some());
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-z"
        } else {
            "ctrl-z"
        });
        view.read_with(cx, |view, cx| {
            assert_eq!(view.prompt_input.read(cx).value(), "已有草稿：用户刚编辑")
        });
    }

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

    fn user_ask_ui_questions() -> Vec<UserAskQuestion> {
        vec![
            UserAskQuestion {
                id: "target".into(),
                prompt: "选择修改范围".into(),
                answer_mode: UserAskAnswerMode::Choice {
                    multiple: false,
                    allow_custom: false,
                },
                options: vec![
                    UserAskOption {
                        id: "library".into(),
                        label: "核心库".into(),
                        description: Some("只修改共享 crate".into()),
                    },
                    UserAskOption {
                        id: "workspace".into(),
                        label: "整个工作区".into(),
                        description: None,
                    },
                ],
            },
            UserAskQuestion {
                id: "checks".into(),
                prompt: "选择需要执行的检查".into(),
                answer_mode: UserAskAnswerMode::Choice {
                    multiple: true,
                    allow_custom: false,
                },
                options: vec![
                    UserAskOption {
                        id: "tests".into(),
                        label: "测试".into(),
                        description: None,
                    },
                    UserAskOption {
                        id: "clippy".into(),
                        label: "Clippy".into(),
                        description: None,
                    },
                ],
            },
            UserAskQuestion {
                id: "scope".into(),
                prompt: "选择预设或填写其他范围".into(),
                answer_mode: UserAskAnswerMode::Choice {
                    multiple: false,
                    allow_custom: true,
                },
                options: vec![UserAskOption {
                    id: "focused".into(),
                    label: "当前模块".into(),
                    description: None,
                }],
            },
            UserAskQuestion {
                id: "note".into(),
                prompt: "补充说明".into(),
                answer_mode: UserAskAnswerMode::Text,
                options: Vec::new(),
            },
        ]
    }

    fn click_debug(cx: &mut gpui::VisualTestContext, selector: &'static str) {
        let point = cx.debug_bounds(selector).unwrap().center();
        cx.simulate_click(point, Default::default());
    }

    #[gpui::test]
    fn worktree_selectors_share_the_directory_row_and_select_existing_branches(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::{infrastructure::git, model::workspace::WorkspaceKind};
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (_repository, project) = git::tests::repository_fixture();
        let (mut presenter, _, _directory) = fixture();
        presenter.open_project(Path::new(&project.canonical_path));
        assert!(presenter.set_appearance(AppearanceSettings {
            reduced_motion: true,
            ..Default::default()
        }));
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
        // Read branch choices when opening the menu, including branches added after render.
        // Overflow the dropdown without exceeding Windows' Git ref lock-path limit.
        let source = format!("release/{}", "long-branch-".repeat(6));
        git::git(Path::new(&project.canonical_path), &["branch", &source]).unwrap();
        for (language, width, height) in [
            (Language::Chinese, 1040., 680.),
            (Language::English, 1280., 800.),
        ] {
            cx.simulate_resize(gpui::size(px(width), px(height)));
            view.update_in(cx, |view, window, cx| {
                view.presenter.select_workspace_kind(WorkspaceKind::Local);
                view.set_language(language, window, cx);
                cx.notify();
            });
            cx.run_until_parked();
            assert!(cx.debug_bounds("workspace-base").is_none());
            click_debug(cx, "workspace-mode");
            cx.run_until_parked();
            cx.simulate_keystrokes("down down enter");
            cx.run_until_parked();
            assert_eq!(
                view.read_with(cx, |view, _| view.presenter.model().workspace_draft.kind),
                WorkspaceKind::Worktree
            );
            click_debug(cx, "workspace-base");
            cx.run_until_parked();
            cx.simulate_keystrokes("down down enter");
            cx.run_until_parked();
            assert_eq!(
                view.read_with(cx, |view, _| view
                    .presenter
                    .model()
                    .workspace_draft
                    .base
                    .clone()),
                source
            );
            let context = cx.debug_bounds("composer-context").unwrap();
            let directory = cx.debug_bounds("project-picker-trigger").unwrap();
            let mode = cx.debug_bounds("workspace-mode").unwrap();
            let base = cx.debug_bounds("workspace-base").unwrap();
            let composer = cx.debug_bounds("composer-surface").unwrap();
            assert_eq!(directory.left(), context.left() + px(12.));
            assert_eq!(directory.size.height, mode.size.height);
            assert!(
                directory.center().y >= mode.center().y - px(1.)
                    && directory.center().y <= mode.center().y + px(1.)
            );
            assert_eq!(mode.center().y, base.center().y);
            assert!(directory.right() <= mode.left());
            assert!(mode.right() <= base.left());
            assert!(base.right() <= context.right());
            assert_eq!(base.size.width, px(200.));
            assert!(context.bottom() <= composer.top());
            click_debug(cx, "workspace-mode");
            cx.run_until_parked();
            cx.simulate_keystrokes("down enter");
            cx.run_until_parked();
            assert_eq!(
                view.read_with(cx, |view, _| view.presenter.model().workspace_draft.kind),
                WorkspaceKind::Local
            );
            assert!(cx.debug_bounds("workspace-base").is_none());
        }
    }

    #[gpui::test]
    fn workspace_review_uses_full_page_preserves_drafts_and_navigates_files(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::{
            infrastructure::git,
            presenter::tests::{finish_workspace_operation, seed_cnb_issues, worktree_fixture},
        };
        use gpui::{ScrollDelta, ScrollWheelEvent, point};
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, runner, _directory, start) = worktree_fixture("review task");
        presenter.set_appearance(AppearanceSettings {
            reduced_motion: true,
            ..Default::default()
        });
        runner.emit(Event::RunExited {
            run_id: start.run_id,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        let cwd = Path::new(&start.cwd);
        std::fs::write(
            cwd.join("tracked.txt"),
            format!("{}\n", "long diff content ".repeat(30)).repeat(80),
        )
        .unwrap();
        std::fs::write(cwd.join("staged.rs"), "fn staged() {}\n").unwrap();
        git::git(cwd, &["add", "staged.rs"]).unwrap();
        std::fs::create_dir(cwd.join("docs")).unwrap();
        std::fs::write(cwd.join("docs/新文件.md"), "# New document\n").unwrap();
        presenter.toggle_changes_sidebar();
        finish_workspace_operation(&mut presenter);
        presenter.select_changed_file("tracked.txt".into(), true);
        presenter.set_commit_message("Reviewed draft".into());
        seed_cnb_issues(&mut presenter);
        let original = presenter
            .model()
            .workspace_review
            .as_ref()
            .unwrap()
            .unstaged
            .clone();
        let id = presenter
            .model()
            .workspace_review
            .as_ref()
            .unwrap()
            .workspace_id;
        let (root, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| NexusView::new(presenter, window, cx));
            gpui_kit::component::Root::new(view, window, cx)
        });
        let view = root.read_with(cx, |root, _| {
            root.view().clone().downcast::<NexusView>().unwrap()
        });
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
                let _ = window.draw(cx);
            });
        };
        cx.simulate_resize(gpui::size(px(1040.), px(680.)));
        draw(cx);
        click_debug(cx, "environment-changes");
        draw(cx);
        click_debug(cx, "show-workspace-diff");
        draw(cx);
        for (width, height, language) in [
            (1040., 680., Language::Chinese),
            (1440., 900., Language::English),
        ] {
            cx.simulate_resize(gpui::size(px(width), px(height)));
            view.update_in(cx, |view, window, cx| {
                view.set_language(language, window, cx)
            });
            draw(cx);
            let page = cx.debug_bounds("workspace-review-page").unwrap();
            let navigation = cx.debug_bounds("review-file-list").unwrap();
            let code = cx
                .debug_bounds(format!("review-diff-{id}-Unstaged-tracked.txt").leak())
                .unwrap();
            assert!(page.right() <= px(width) && page.bottom() <= px(height));
            assert!(
                code.size.height > px(height * 0.8),
                "review must use available height: {code:?}"
            );
            assert!(code.size.width > px(480.) && code.left() >= navigation.right());
            assert!(cx.debug_bounds("composer-surface").is_none());
            assert!(cx.debug_bounds("workspace-header-status").is_none());
            assert!(cx.debug_bounds("review-status").is_none());
            assert!(cx.debug_bounds("review-merge-panel").is_none());
        }
        let code_key: &'static str = format!("review-diff-{id}-Unstaged-tracked.txt").leak();
        let viewport = cx.debug_bounds(code_key).unwrap();
        for delta in [point(px(-80.), px(0.)), point(px(0.), px(-60.))] {
            let before = cx
                .debug_bounds(format!("{code_key}-content").leak())
                .unwrap();
            cx.simulate_event(ScrollWheelEvent {
                position: viewport.center(),
                delta: ScrollDelta::Pixels(delta),
                ..Default::default()
            });
            draw(cx);
            let after = cx
                .debug_bounds(format!("{code_key}-content").leak())
                .unwrap();
            assert_eq!(after.origin, before.origin + delta);
        }
        click_debug(cx, format!("{code_key}-copy").leak());
        cx.update(|_, cx| assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), original));
        click_debug(cx, "review-nav-Untracked-docs/新文件.md");
        draw(cx);
        assert!(
            cx.debug_bounds(format!("review-diff-{id}-Untracked-docs/新文件.md").leak())
                .is_some()
        );
        click_debug(cx, "sidebar-cnb");
        draw(cx);
        assert!(cx.debug_bounds("cnb-page").is_some());
        assert!(cx.debug_bounds("workspace-review-page").is_none());
        assert!(cx.debug_bounds("composer-surface").is_none());
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx.debug_bounds("cnb-page").is_some());
        view.update_in(cx, |view, window, cx| {
            view.select_task(start.task_id, window, cx);
        });
        draw(cx);
        assert!(cx.debug_bounds("cnb-page").is_none());
        assert!(cx.debug_bounds("workspace-review-page").is_some());
        assert!(
            cx.debug_bounds(format!("review-diff-{id}-Untracked-docs/新文件.md").leak())
                .is_some()
        );
        std::fs::write(cwd.join("docs/新文件.md"), "# Updated document\n").unwrap();
        click_debug(cx, "refresh-workspace-review");
        view.update(cx, |view, cx| {
            finish_workspace_operation(&mut view.presenter);
            cx.notify();
        });
        draw(cx);
        click_debug(
            cx,
            format!("review-diff-{id}-Untracked-docs/新文件.md-copy").leak(),
        );
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().unwrap(),
                "# Updated document\n"
            )
        });
        click_debug(cx, "close-workspace-review");
        draw(cx);
        assert!(cx.debug_bounds("composer-surface").is_some());
        view.read_with(cx, |view, _| {
            assert_eq!(view.presenter.model().commit_message, "Reviewed draft");
            assert_eq!(
                view.presenter
                    .model()
                    .selected_changes
                    .iter()
                    .collect::<Vec<_>>(),
                vec!["tracked.txt"]
            );
        });
        click_debug(cx, "open-commit-editor");
        draw(cx);
        assert_eq!(
            cx.debug_bounds("generate-commit-message")
                .unwrap()
                .size
                .width,
            px(28.)
        );
    }

    #[gpui::test]
    fn workspace_review_keeps_merge_preview_and_confirmation_on_the_page(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::{
            infrastructure::git,
            presenter::tests::{finish_workspace_operation, worktree_fixture},
        };
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, runner, _directory, start) = worktree_fixture("merge task");
        presenter.set_appearance(AppearanceSettings {
            reduced_motion: true,
            ..Default::default()
        });
        runner.emit(Event::RunExited {
            run_id: start.run_id,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        let cwd = Path::new(&start.cwd);
        std::fs::write(cwd.join("tracked.txt"), "reviewed merge\n").unwrap();
        git::git(cwd, &["commit", "-am", "reviewed change"]).unwrap();
        let task_status = presenter
            .model()
            .latest_log_text(presenter.model().language)
            .to_owned();
        let project = presenter.model().selected_project.clone().unwrap();
        let (root, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| NexusView::new(presenter, window, cx));
            gpui_kit::component::Root::new(view, window, cx)
        });
        let view = root.read_with(cx, |root, _| {
            root.view().clone().downcast::<NexusView>().unwrap()
        });
        cx.simulate_resize(gpui::size(px(1040.), px(680.)));
        view.update_in(cx, |view, window, cx| {
            view.open_workspace_review(start.task_id, window, cx);
            finish_workspace_operation(&mut view.presenter);
            cx.notify();
        });
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
                let _ = window.draw(cx);
            });
        };
        draw(cx);
        click_debug(cx, "review-merge-options");
        draw(cx);
        click_debug(cx, "preview-workspace-merge");
        view.update(cx, |view, cx| {
            finish_workspace_operation(&mut view.presenter);
            cx.notify();
        });
        draw(cx);
        assert!(
            cx.debug_bounds(format!("review-diff-{}-Merge-tracked.txt", start.task_id).leak())
                .is_some()
        );
        assert_eq!(
            std::fs::read_to_string(Path::new(&project.canonical_path).join("tracked.txt"))
                .unwrap(),
            "base\n"
        );
        click_debug(cx, "confirm-workspace-merge");
        view.update(cx, |view, cx| {
            finish_workspace_operation(&mut view.presenter);
            cx.notify();
        });
        draw(cx);
        assert_eq!(
            std::fs::read_to_string(Path::new(&project.canonical_path).join("tracked.txt"))
                .unwrap(),
            "reviewed merge\n"
        );
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.presenter
                    .model()
                    .latest_log_text(view.presenter.model().language),
                task_status
            )
        });
        assert!(cx.debug_bounds("review-status").is_some());
        assert!(cx.debug_bounds("workspace-review-page").is_some());
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx.debug_bounds("workspace-review-page").is_none());
        assert!(cx.debug_bounds("composer-surface").is_some());
    }

    #[gpui::test]
    fn changes_sidebar_generates_editable_messages_and_requires_commit_confirmation(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::presenter::tests::{finish_workspace_operation, worktree_fixture};
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, runner, _directory, start) = worktree_fixture("review task");
        // Keep click targets fixed while testing the confirmation flow.
        assert!(presenter.set_appearance(AppearanceSettings {
            reduced_motion: true,
            ..Default::default()
        }));
        runner.emit(Event::RunExited {
            run_id: start.run_id,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        let cwd = std::path::Path::new(&start.cwd);
        // Opening a review must also notice a branch renamed after the run ended.
        crate::infrastructure::git::git(cwd, &["branch", "-m", "fix/review-task"]).unwrap();
        std::fs::write(cwd.join("tracked.txt"), "reviewed content\n").unwrap();
        let before = crate::infrastructure::git::git(cwd, &["rev-parse", "HEAD"]).unwrap();
        let (root, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| NexusView::new(presenter, window, cx));
            gpui_kit::component::Root::new(view, window, cx)
        });
        let view = root.read_with(cx, |root, _| {
            root.view().clone().downcast::<NexusView>().unwrap()
        });
        cx.simulate_resize(gpui::size(px(1040.), px(680.)));
        view.update_in(cx, |view, window, cx| {
            view.set_language(Language::English, window, cx);
            view.presenter.toggle_changes_sidebar();
            finish_workspace_operation(&mut view.presenter);
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        let content = cx.debug_bounds("conversation-right-sidebar").unwrap();
        assert!(content.size.height > px(0.));
        assert!(content.right() <= px(1040.));
        assert!(content.bottom() <= px(680.));
        assert!(cx.debug_bounds("composer-surface").unwrap().right() <= content.left());
        let card = cx.debug_bounds("environment-card").unwrap();
        assert!(card.size.height < px(240.));
        assert!(cx.debug_bounds("commit-editor").is_none());
        assert!(cx.debug_bounds("conversation-changed-files").is_none());
        click_debug(cx, "environment-changes");
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        click_debug(cx, "review-file-tracked.txt");
        click_debug(cx, "open-commit-editor");
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        click_debug(cx, "generate-commit-message");
        view.update(cx, |view, cx| {
            finish_workspace_operation(&mut view.presenter);
            let request_id = view.presenter.model().commit_message_request.unwrap();
            runner.emit(Event::CommitMessageGenerated {
                request_id,
                message: "fix: Generated description\n\nGenerated body".into(),
            });
            view.presenter.drain_events();
            cx.notify();
        });
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            let input = &view.commit_inputs[&view.presenter.model().conversation.id];
            assert_eq!(
                input.read(cx).value(),
                "fix: Generated description\n\nGenerated body"
            );
            input.update(cx, |input, cx| input.focus(window, cx));
        });
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-a"
        } else {
            "ctrl-a"
        });
        cx.simulate_input("fix: Edited description\n\nReviewed body");
        cx.run_until_parked();
        click_debug(cx, "commit-workspace-files");
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        assert_eq!(
            crate::infrastructure::git::git(cwd, &["rev-parse", "HEAD"]).unwrap(),
            before
        );
        click_debug(cx, "commit-workspace-files");
        cx.simulate_prompt_answer("Commit");
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            finish_workspace_operation(&mut view.presenter);
            assert!(!view.presenter.model().commit_editor_open);
            cx.notify();
        });
        assert_ne!(
            crate::infrastructure::git::git(cwd, &["rev-parse", "HEAD"]).unwrap(),
            before
        );
        assert_eq!(
            crate::infrastructure::git::git(cwd, &["log", "-1", "--format=%s"])
                .unwrap()
                .trim(),
            "fix: Edited description"
        );
        assert!(
            crate::infrastructure::git::git(cwd, &["status", "--porcelain"])
                .unwrap()
                .is_empty()
        );
        click_debug(cx, "environment-changes");
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        click_debug(cx, "open-commit-editor");
        assert!(cx.debug_bounds("commit-editor").is_none());
        assert!(cx.debug_bounds("generate-commit-message").is_none());
        assert!(cx.debug_bounds("environment-card").unwrap().size.height < px(260.));
    }

    #[gpui::test]
    fn cli_installation_settings_show_the_action_and_completion_feedback(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, _, _directory) = fixture();
        let sender = crate::presenter::tests::pending_cli_installation(&mut presenter);
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
        cx.simulate_resize(gpui::size(px(1040.), px(680.)));
        view.update_in(cx, |view, _, cx| {
            view.settings_open = true;
            view.settings_section = SettingsSection::General;
            cx.notify();
        });
        cx.run_until_parked();
        view.update_in(cx, |view, _, cx| {
            view.settings_scroll
                .set_offset(gpui::point(px(0.), px(-10_000.)));
            cx.notify();
        });
        cx.run_until_parked();
        click_debug(cx, "install-cli");
        assert!(view.read_with(cx, |view, _| view.presenter.model().cli_installation_busy));
        assert!(cx.debug_bounds("cli-installation-message").is_none());
        sender.send(Ok(())).unwrap();
        for (width, height, language) in [
            (1040., 680., Language::Chinese),
            (1280., 900., Language::English),
        ] {
            cx.simulate_resize(gpui::size(px(width), px(height)));
            view.update_in(cx, |view, window, cx| {
                view.presenter.drain_events();
                view.set_language(language, window, cx);
                view.settings_scroll
                    .set_offset(gpui::point(px(0.), px(-10_000.)));
                cx.notify();
            });
            cx.run_until_parked();
            for selector in ["install-cli", "cli-installation-message"] {
                let bounds = cx.debug_bounds(selector).unwrap();
                assert!(
                    bounds.left() >= px(0.) && bounds.right() <= px(width),
                    "{selector}: {bounds:?}"
                );
                assert!(
                    bounds.top() >= px(0.) && bounds.bottom() <= px(height),
                    "{selector}: {bounds:?}"
                );
            }
            assert!(!view.read_with(cx, |view, _| view.presenter.model().cli_installation_busy));
        }
    }

    #[gpui::test]
    fn harness_settings_show_versions_sources_and_only_available_actions(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, _, _directory) = fixture();
        crate::presenter::tests::seed_harness_installations(&mut presenter);
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
        for (width, height, language) in [
            (1040., 680., Language::Chinese),
            (1280., 900., Language::English),
        ] {
            cx.simulate_resize(gpui::size(px(width), px(height)));
            view.update_in(cx, |view, window, cx| {
                view.settings_open = true;
                view.settings_section = SettingsSection::Agent;
                view.set_language(language, window, cx);
                view.reduced_motion = true;
                cx.notify();
            });
            cx.run_until_parked();
            for selector in [
                "harness-card-claude",
                "harness-card-codex",
                "harness-card-omp",
                "harness-current-version-claude",
                "harness-latest-version-claude",
                "harness-current-version-codex",
                "harness-latest-version-codex",
                "harness-current-version-omp",
                "harness-latest-version-omp",
            ] {
                let bounds = cx.debug_bounds(selector).unwrap();
                assert!(
                    bounds.left() >= px(0.) && bounds.right() <= px(width),
                    "{selector}: {bounds:?}"
                );
            }
            assert!(cx.debug_bounds("harness-update-claude").is_some());
            assert!(cx.debug_bounds("harness-install-codex").is_some());
            assert!(cx.debug_bounds("harness-update-omp").is_none());
            assert!(cx.debug_bounds("harness-install-claude").is_none());
            for latest in [
                Ok("1.0.0"),
                Ok("0.9.0"),
                Err("最新版本检测失败：网络不可用"),
            ] {
                view.update_in(cx, |view, _, cx| {
                    crate::presenter::tests::seed_harness_installations(&mut view.presenter)
                        .get_mut(&HarnessKind::Claude)
                        .unwrap()
                        .latest_version = latest
                        .map(str::to_owned)
                        .map_err(|error| error.to_owned().into());
                    cx.notify();
                });
                cx.run_until_parked();
                assert!(cx.debug_bounds("harness-update-claude").is_none());
                assert!(cx.debug_bounds("harness-latest-version-claude").is_some());
                assert!(cx.debug_bounds("harness-install-codex").is_some());
            }
            view.update_in(cx, |view, _, cx| {
                crate::presenter::tests::seed_harness_installations(&mut view.presenter);
                cx.notify();
            });
            cx.run_until_parked();
        }
        click_debug(cx, "harness-install-codex");
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.presenter.model().selected_harness),
            HarnessKind::Claude
        );
    }

    #[test]
    fn working_directory_labels_preserve_paths_and_localize_empty_states() {
        let (presenter, _, _directory) = fixture();
        let mut model = AppModel::default();
        for (language, no_directory) in [
            (Language::Chinese, "未选择目录"),
            (Language::English, "No directory selected"),
        ] {
            model.language = language;
            model.selected_project = None;
            assert_eq!(working_directory_label(&model), language.text("未关联项目"));
            model.selected_project = presenter.model().selected_project.clone();
            let long_name = "中文 long directory ".repeat(30);
            for name in ["nexus", "含 空格的目录", long_name.as_str()] {
                let path = Path::new("workspace").join(name).display().to_string();
                let project = model.selected_project.as_mut().unwrap();
                project.canonical_path = path.clone();
                project.display_name = "unrelated project alias".into();
                assert_eq!(working_directory_label(&model), name);
                assert_eq!(model.working_directory(), Some(path.as_str()));
            }
            let root = if cfg!(windows) { "C:\\" } else { "/" };
            model.selected_project.as_mut().unwrap().canonical_path = root.into();
            assert_eq!(working_directory_label(&model), root);

            model
                .selected_project
                .as_mut()
                .unwrap()
                .canonical_path
                .clear();
            assert_eq!(working_directory_label(&model), no_directory);
            assert_eq!(model.working_directory(), None);
        }
    }

    #[gpui::test]
    fn working_directory_picker_searches_switches_and_dismisses_without_resetting_current_project(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, _, directory) = fixture();
        let first = presenter.model().selected_project.clone().unwrap();
        let path = directory.path().join("第二个 Project");
        std::fs::create_dir_all(&path).unwrap();
        presenter.open_project(&path);
        let second = presenter.model().selected_project.clone().unwrap();
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
        view.update_in(cx, |view, window, cx| {
            view.prompt_input
                .update(cx, |input, cx| input.set_value("keep draft", window, cx));
        });
        cx.run_until_parked();
        let trigger = cx.debug_bounds("project-picker-trigger").unwrap();
        let content = cx.debug_bounds("composer-directory").unwrap();
        let name = cx.debug_bounds("composer-directory-name").unwrap();
        assert_eq!(content.left() - trigger.left(), px(8.));
        assert_eq!(trigger.right() - content.right(), px(8.));
        assert!(trigger.top() < content.top() && content.bottom() < trigger.bottom());
        assert!(
            trigger.size.width <= name.size.width + px(40.),
            "directory trigger should fit its icon, name and padding: {trigger:?}, {name:?}"
        );
        click_debug(cx, "composer-directory");
        cx.run_until_parked();
        assert!(cx.debug_bounds("project-picker-surface").is_some());
        assert!(cx.debug_bounds("project-picker-new").is_some());
        let second_row: &'static str = format!("project-picker-{}", second.id).leak();
        let first_row: &'static str = format!("project-picker-{}", first.id).leak();
        assert!(cx.debug_bounds(first_row).is_some());
        let search = cx.debug_bounds("project-picker-search").unwrap();
        let first_bounds = cx.debug_bounds(first_row).unwrap();
        let second_bounds = cx.debug_bounds(second_row).unwrap();
        assert!(search.size.height >= px(CONTROL_HEIGHT));
        assert_eq!(first_bounds.size.height, px(40.));
        assert_eq!(second_bounds.size.height, px(40.));
        assert!(first_bounds.top() >= search.bottom() + px(12.));
        assert!(second_bounds.top() >= search.bottom() + px(12.));
        assert!(
            first_bounds.bottom() + px(4.) <= second_bounds.top()
                || second_bounds.bottom() + px(4.) <= first_bounds.top()
        );
        view.update_in(cx, |view, window, cx| {
            view.project_search_input.update(cx, |input, cx| {
                input.set_value(" 第二个 PROJECT ", window, cx)
            });
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds(first_row).is_none());
        click_debug(cx, second_row);
        cx.run_until_parked();
        assert!(cx.debug_bounds("project-picker-surface").is_none());
        view.read_with(cx, |view, cx| {
            assert_eq!(view.prompt_input.read(cx).value(), "keep draft");
            assert_eq!(
                view.presenter.model().selected_project.as_ref().unwrap().id,
                second.id
            );
        });
        click_debug(cx, "composer-directory");
        cx.run_until_parked();
        assert!(
            cx.debug_bounds(first_row).is_some(),
            "reopening clears search"
        );
        click_debug(cx, first_row);
        cx.run_until_parked();
        assert!(cx.debug_bounds("project-picker-surface").is_none());
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.presenter.model().selected_project.as_ref().unwrap().id,
                first.id
            );
        });
        click_debug(cx, "composer-directory");
        cx.run_until_parked();
        click_debug(cx, "project-picker-none");
        cx.run_until_parked();
        assert!(cx.debug_bounds("project-picker-surface").is_none());
        assert!(cx.debug_bounds("sidebar-projectless").is_some());
        view.read_with(cx, |view, _| {
            assert!(view.presenter.model().selected_project.is_none());
            assert_eq!(
                working_directory_label(view.presenter.model()),
                "未关联项目"
            );
            assert_ne!(
                view.presenter.model().working_directory(),
                Some(first.canonical_path.as_str())
            );
        });
        click_debug(cx, "composer-directory");
        cx.run_until_parked();
        click_debug(cx, first_row);
        cx.run_until_parked();
        click_debug(cx, "composer-directory");
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            view.project_search_input.update(cx, |input, cx| {
                input.set_value("no such project", window, cx)
            });
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds(first_row).is_none());
        assert!(cx.debug_bounds(second_row).is_none());
        assert!(cx.debug_bounds("project-picker-new").is_some());
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(cx.debug_bounds("project-picker-surface").is_none());
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.presenter.model().selected_project.as_ref().unwrap().id,
                first.id
            );
        });
        let empty_directory = tempfile::tempdir().unwrap();
        view.update(cx, |view, cx| {
            view.presenter = Presenter::new(
                crate::infrastructure::storage::Storage::open(
                    &empty_directory.path().join("empty.sqlite"),
                )
                .unwrap(),
                Err(anyhow::anyhow!("no runner")),
                None,
            );
            cx.notify();
        });
        cx.run_until_parked();
        click_debug(cx, "composer-directory");
        cx.run_until_parked();
        assert!(cx.debug_bounds("project-picker-surface").is_some());
        assert!(cx.debug_bounds("project-picker-new").is_some());
        cx.simulate_click(gpui::point(px(10.), px(10.)), Default::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("project-picker-surface").is_none());
        assert!(view.read_with(cx, |view, _| {
            view.presenter.model().selected_project.is_none()
        }));
    }

    #[gpui::test]
    fn projectless_composer_sends_and_sidebar_restores_its_context(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (presenter, runner, _directory) = fixture();
        let project = presenter.model().selected_project.clone().unwrap();
        let models = presenter.model().model_catalog.models().unwrap().to_vec();
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
        cx.run_until_parked();
        click_debug(cx, "sidebar-projectless");
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            let ModelCatalogState::Loading { request_id, .. } =
                view.presenter.model().model_catalog
            else {
                panic!("expected model discovery in the conversation directory");
            };
            runner.emit(nexus_protocol::Event::ModelCatalogLoaded {
                request_id,
                harness: HarnessKind::Claude,
                models,
            });
            view.presenter.drain_events();
            view.prompt_input
                .update(cx, |input, cx| input.set_value("你好", window, cx));
            cx.notify();
        });
        cx.run_until_parked();
        click_debug(cx, "composer-submit");
        cx.run_until_parked();
        let (task_id, cwd) = view.read_with(cx, |view, cx| {
            let model = view.presenter.model();
            assert!(model.selected_project.is_none());
            assert!(model.active_run.is_some());
            assert_eq!(model.messages[0].content, "你好");
            assert_eq!(working_directory_label(model), "未关联项目");
            assert!(view.prompt_input.read(cx).value().is_empty());
            (
                model.selected_task.unwrap(),
                model.working_directory().unwrap().to_owned(),
            )
        });
        let task_row = format!("sidebar-task-{task_id}").leak();
        assert!(cx.debug_bounds(task_row).is_some());
        click_debug(cx, format!("sidebar-project-{}", project.id).leak());
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| {
            view.presenter.model().selected_project.is_some()
        }));
        click_debug(cx, task_row);
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            let model = view.presenter.model();
            assert!(model.selected_project.is_none());
            assert_eq!(model.selected_task, Some(task_id));
            assert_eq!(model.working_directory(), Some(cwd.as_str()));
        });
        view.update_in(cx, |view, window, cx| view.new_task(window, cx));
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            let model = view.presenter.model();
            assert!(model.selected_project.is_none());
            assert!(model.selected_task.is_none());
            assert_ne!(model.working_directory(), Some(cwd.as_str()));
        });
    }

    #[gpui::test]
    fn workspace_opener_tracks_task_switches_and_reports_missing_directories(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::presenter::tests::{finish_workspace_operation, worktree_fixture};
        // Path validation uses smol workers outside GPUI's deterministic test scheduler.
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, runner, directory, start) = worktree_fixture("open directory");
        runner.emit(Event::RunExited {
            run_id: start.run_id,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        presenter.set_appearance(AppearanceSettings {
            reduced_motion: true,
            ..Default::default()
        });
        let mut project = presenter.model().selected_project.clone().unwrap();
        project.canonical_path = directory
            .path()
            .join("missing local 目录")
            .display()
            .to_string();
        std::fs::rename(&start.cwd, directory.path().join("moved-worktree")).unwrap();
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
        cx.run_until_parked();
        click_debug(cx, "composer-open-directory");
        cx.run_until_parked();
        assert!(cx.debug_bounds("animated-menu-surface").is_some());
        view.update(cx, |view, cx| {
            view.select_project(project.clone());
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("animated-menu-surface").is_none());

        for (language, environment, expected) in [
            (Language::Chinese, false, project.canonical_path.as_str()),
            (Language::English, true, start.cwd.as_str()),
        ] {
            view.update_in(cx, |view, window, cx| {
                if environment {
                    view.presenter.select_task(start.task_id);
                    view.presenter.toggle_changes_sidebar();
                    finish_workspace_operation(&mut view.presenter);
                }
                view.set_language(language, window, cx);
                cx.notify();
            });
            cx.run_until_parked();
            assert_eq!(
                view.read_with(cx, |view, _| view
                    .presenter
                    .model()
                    .working_directory()
                    .map(str::to_owned)),
                Some(expected.to_owned())
            );
            click_debug(
                cx,
                if environment {
                    "environment-directory"
                } else {
                    "composer-open-directory"
                },
            );
            cx.run_until_parked();
            cx.simulate_keystrokes("down enter");
            // Path validation runs on a real worker, outside GPUI's fake executor.
            let deadline = Instant::now() + Duration::from_secs(10);
            while !cx.has_pending_prompt() {
                cx.run_until_parked();
                assert!(
                    Instant::now() < deadline,
                    "missing directory error was not shown"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            let (title, detail) = cx.pending_prompt().unwrap();
            assert_eq!(title, language.text("无法打开工作目录"));
            assert!(detail.contains(expected));
            assert!(
                detail.contains(
                    language
                        .text("工作目录不存在或无法访问：{path}")
                        .split("{path}")
                        .next()
                        .unwrap()
                )
            );
            cx.simulate_prompt_answer(language.text("确定"));
            cx.run_until_parked();
        }

        project.canonical_path.clear();
        view.update(cx, |view, cx| {
            view.select_project(project);
            cx.notify();
        });
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| {
            view.presenter.model().working_directory().is_none()
        }));
        assert!(cx.debug_bounds("animated-menu-surface").is_none());
        click_debug(cx, "composer-open-directory");
        cx.run_until_parked();
        assert!(cx.debug_bounds("animated-menu-surface").is_none());
        assert!(!cx.has_pending_prompt());
    }

    #[gpui::test]
    fn working_directory_stays_above_composer_without_overlapping_queue_or_controls(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, runner, directory) = fixture();
        let path = directory
            .path()
            .join("parent".repeat(35))
            .join(format!("{}nexus", "目录 name ".repeat(15)));
        std::fs::create_dir_all(&path).unwrap();
        presenter.open_project(&path);
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
        for (theme, language, width, height) in [
            (ThemePreference::Light, Language::Chinese, 1040., 680.),
            (ThemePreference::Dark, Language::Chinese, 1040., 680.),
            (ThemePreference::Light, Language::English, 1040., 680.),
            (ThemePreference::Dark, Language::English, 1040., 680.),
            (ThemePreference::Dark, Language::English, 1280., 800.),
        ] {
            cx.simulate_resize(gpui::size(px(width), px(height)));
            view.update_in(cx, |view, window, cx| {
                view.set_appearance(
                    AppearanceSettings {
                        theme,
                        reduced_motion: true,
                        ..Default::default()
                    },
                    window,
                    cx,
                );
                view.set_language(language, window, cx);
            });
            for prompt in ["start task", "queued message"] {
                cx.run_until_parked();
                let context = cx.debug_bounds("composer-context").unwrap();
                let directory = cx.debug_bounds("composer-directory").unwrap();
                let opener = cx.debug_bounds("composer-open-directory").unwrap();
                let name = cx.debug_bounds("composer-directory-name").unwrap();
                let composer = cx.debug_bounds("composer-surface").unwrap();
                let send = cx.debug_bounds("composer-submit").unwrap();
                assert_eq!(context.left(), composer.left());
                assert_eq!(context.right(), composer.right());
                assert!(directory.top() > px(HEADER_HEIGHT));
                assert!(directory.bottom() <= composer.top());
                assert!(directory.right() <= opener.left());
                assert!(opener.right() <= context.right());
                assert!(name.right() <= context.right());
                assert!(name.size.height <= px(24.));
                assert!(send.left() >= composer.left() && send.right() <= composer.right());
                assert!(send.top() >= composer.top() && send.bottom() <= px(height));
                view.update_in(cx, |view, window, cx| {
                    view.prompt_input
                        .update(cx, |input, cx| input.set_value(prompt, window, cx));
                });
                cx.run_until_parked();
                click_debug(cx, "composer-submit");
            }
            cx.run_until_parked();
            let queue = cx.debug_bounds("message-queue").unwrap();
            let composer = cx.debug_bounds("composer-surface").unwrap();
            let directory = cx.debug_bounds("composer-directory").unwrap();
            let stop = cx.debug_bounds("composer-cancel").unwrap();
            assert!(directory.bottom() <= composer.top());
            assert!(queue.top() >= composer.top() && queue.bottom() <= stop.top());
            assert!(stop.bottom() <= composer.bottom() && composer.bottom() <= px(height));
            assert!(stop.right() <= cx.debug_bounds("composer-submit").unwrap().left());
            cx.simulate_mouse_move(directory.center(), None, Default::default());
            cx.executor().advance_clock(Duration::from_secs(1));
            cx.run_until_parked();
            let tooltip = cx.debug_bounds("composer-directory-tooltip").unwrap();
            assert!(tooltip.left() >= px(0.) && tooltip.right() <= px(width));
            assert!(tooltip.top() >= px(0.) && tooltip.bottom() <= px(height));
            assert!(tooltip.size.height > px(24.));
            click_debug(cx, "composer-cancel");
            view.update(cx, |view, cx| {
                let model = view.presenter.model();
                assert!(model.run_cancelling);
                assert_eq!(model.queued_messages.len(), 1);
                let queued_id = model.queued_messages[0].id;
                runner.emit(Event::RunExited {
                    run_id: model.active_run.unwrap(),
                    status: RunStatus::Cancelled,
                    exit_code: None,
                });
                view.presenter.drain_events();
                view.presenter.remove_queued_message(queued_id);
                view.presenter.new_task();
                cx.notify();
            });
        }
        cx.run_until_parked();
        let directory_bounds = cx.debug_bounds("composer-directory").unwrap();
        cx.simulate_mouse_move(
            gpui::point(
                directory_bounds.left() + px(20.),
                directory_bounds.center().y,
            ),
            None,
            Default::default(),
        );
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert!(cx.debug_bounds("composer-directory-tooltip").is_some());
        view.update(cx, |view, cx| {
            view.presenter.open_project(directory.path());
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("composer-directory-tooltip").is_none());
        let directory_bounds = cx.debug_bounds("composer-directory").unwrap();
        cx.simulate_mouse_move(directory_bounds.center(), None, Default::default());
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert!(cx.debug_bounds("composer-directory-tooltip").is_some());
    }

    #[test]
    fn catalog_content_groups_providers_and_searches_provider_name_and_full_id() {
        let mut model = AppModel {
            conversation: crate::model::ConversationState {
                selected_harness: HarnessKind::Omp,
                model_catalog: ModelCatalogState::Ready(vec![
                    omp_model("openai", "openai/shared-model"),
                    omp_model("bigmodel", "bigmodel/shared-model"),
                ]),
                ..Default::default()
            },
            ..Default::default()
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
            conversation: crate::model::ConversationState {
                selected_harness: HarnessKind::Omp,
                model_override: Some("private-provider/custom-model".into()),
                model_catalog: ModelCatalogState::Ready(vec![omp_model(
                    "public-provider",
                    "public-provider/custom-model",
                )]),
                ..Default::default()
            },
            ..Default::default()
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
            conversation: crate::model::ConversationState {
                selected_harness: HarnessKind::Omp,
                model_override: Some("explicit-model".into()),
                model_override_name: Some(explicit.display_name.clone()),
                model_catalog: ModelCatalogState::Ready(vec![default, explicit]),
                ..Default::default()
            },
            ..Default::default()
        };
        let content = CatalogModelSelectContent::from_model(&model);
        assert!(content.groups[0].items[0].title.contains("default-model"));
        assert!(!content.groups[0].items[0].title.contains("explicit-model"));
        for (state, status) in [
            (ModelCatalogState::Idle, "会话目录尚未就绪"),
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
    fn generation_model_settings_support_search_and_preserve_conversation_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (presenter, runner, _directory) = fixture();
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
        view.update_in(cx, |view, window, cx| {
            view.toggle_settings(window, cx);
            assert!(matches!(
                view.presenter.model().title_model_catalog,
                ModelCatalogState::Loading { .. }
            ));
        });
        for ((width, height, language), kind) in [
            (1040., 680., Language::Chinese),
            (1280., 800., Language::English),
        ]
        .into_iter()
        .flat_map(|layout| GenerationKind::ALL.map(|kind| (layout, kind)))
        {
            let harness_selector = match kind {
                GenerationKind::Title => "title-harness",
                GenerationKind::Commit => "commit-harness",
            };
            let model_selector = match kind {
                GenerationKind::Title => "title-model",
                GenerationKind::Commit => "commit-model",
            };
            let effort_selector = match kind {
                GenerationKind::Title => "title-effort",
                GenerationKind::Commit => "commit-effort",
            };
            let picker_selector = match kind {
                GenerationKind::Title => "title-model-picker-surface",
                GenerationKind::Commit => "commit-model-picker-surface",
            };
            cx.simulate_resize(gpui::size(px(width), px(height)));
            view.update_in(cx, |view, window, cx| {
                view.settings_open = true;
                view.settings_scroll.set_offset(gpui::point(px(0.), px(0.)));
                view.set_language(language, window, cx);
                view.presenter
                    .select_generation_harness(kind, HarnessKind::Claude);
                view.reduced_motion = true;
                cx.notify();
            });
            cx.run_until_parked();
            let content = cx.debug_bounds("settings-content-general").unwrap();
            let breadcrumb = cx.debug_bounds("settings-breadcrumb-label").unwrap();
            assert_eq!(content.left(), breadcrumb.left());
            let harness = cx.debug_bounds(harness_selector).unwrap();
            let model = cx.debug_bounds(model_selector).unwrap();
            assert_eq!(model.left(), harness.left());
            assert_eq!(model.size, harness.size);
            for (first, second) in [
                ("language-zh-CN", "language-en"),
                ("update-channel-release", "update-channel-nightly"),
            ] {
                let first = cx.debug_bounds(first).unwrap();
                let second = cx.debug_bounds(second).unwrap();
                assert_eq!(first.left(), harness.left());
                assert_eq!(second.right(), harness.right());
                assert_eq!(first.size, second.size);
            }
            assert_eq!(
                cx.debug_bounds("update-check-on-startup").unwrap().left(),
                harness.left()
            );
            if kind == GenerationKind::Commit {
                view.update(cx, |view, cx| {
                    view.settings_scroll
                        .set_offset(gpui::point(px(0.), px(160.) - harness.top()));
                    cx.notify();
                });
                cx.run_until_parked();
            }
            click_debug(cx, harness_selector);

            cx.run_until_parked();
            cx.simulate_keystrokes("down down down enter");
            cx.run_until_parked();
            assert_eq!(
                view.read_with(cx, |view, _| view
                    .presenter
                    .model()
                    .generation_settings(kind)
                    .harness),
                HarnessKind::Omp
            );
            click_debug(cx, model_selector);
            cx.run_until_parked();
            let request_id = view.read_with(cx, |view, _| {
                let ModelCatalogState::Loading { request_id, .. } =
                    view.presenter.model().generation_catalog(kind).clone()
                else {
                    panic!("loading")
                };
                request_id
            });
            let mut long = omp_model("provider", "provider/title-target");
            long.display_name = "Long title model name ".repeat(30);
            runner.emit(Event::ModelCatalogLoaded {
                request_id,
                harness: HarnessKind::Omp,
                models: vec![long],
            });
            view.update(cx, |view, cx| {
                view.presenter.drain_events();
                cx.notify();
            });
            cx.run_until_parked();
            let bounds = cx.debug_bounds(picker_selector).unwrap();
            assert!(
                bounds.left() >= px(0.) && bounds.right() <= px(width),
                "{bounds:?}"
            );
            assert!(
                bounds.top() >= px(0.) && bounds.bottom() <= px(height),
                "{bounds:?}"
            );
            cx.simulate_input("title-target");
            cx.run_until_parked();
            cx.simulate_keystrokes("enter");
            cx.run_until_parked();
            assert!(cx.debug_bounds(picker_selector).is_none());
            let effort_bounds = cx.debug_bounds(effort_selector).unwrap();
            assert!(effort_bounds.right() <= px(width));
            assert!(effort_bounds.bottom() <= px(height));
            click_debug(cx, effort_selector);
            cx.run_until_parked();
            cx.simulate_keystrokes("down down enter");
            cx.run_until_parked();
            view.read_with(cx, |view, _| {
                assert_eq!(
                    view.presenter
                        .model()
                        .generation_settings(kind)
                        .model
                        .as_deref(),
                    Some("provider/title-target")
                );
                assert_eq!(view.presenter.model().selected_harness, HarnessKind::Claude);
                assert!(view.presenter.model().model_override.is_none());
                assert_eq!(
                    view.presenter.model().generation_settings(kind).effort,
                    ThinkingEffort::XHigh
                );
                assert_eq!(view.presenter.model().effort, ThinkingEffort::Default);
            });
            click_debug(cx, effort_selector);
            cx.run_until_parked();
            cx.simulate_keystrokes("down enter");
            cx.run_until_parked();
            assert_eq!(
                view.read_with(cx, |view, _| view
                    .presenter
                    .model()
                    .generation_settings(kind)
                    .effort),
                ThinkingEffort::Default
            );
            let trigger = cx.debug_bounds(model_selector).unwrap();
            assert!(trigger.right() <= px(width));
            assert_eq!(trigger.size, harness.size);
            click_debug(cx, model_selector);
            cx.run_until_parked();
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            assert!(cx.debug_bounds(picker_selector).is_none());
        }
    }

    #[gpui::test]
    fn user_ask_panel_keeps_drafts_scoped_and_submits_each_answer_once(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, runner, _directory) = fixture();

        assert!(presenter.submit("other task", "claude"));
        let other_task = presenter.model().active_task.unwrap();
        let other_run = presenter.model().active_run.unwrap();
        runner.emit(Event::RunExited {
            run_id: other_run,
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        presenter.new_task();
        assert!(presenter.submit("active task", "claude"));
        let active_task = presenter.model().active_task.unwrap();
        let run_id = presenter.model().active_run.unwrap();
        let request_id = Uuid::new_v4();
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
        cx.simulate_resize(gpui::size(px(1040.), px(680.)));
        view.update_in(cx, |view, window, cx| {
            view.prompt_input
                .update(cx, |input, cx| input.set_value("普通消息草稿", window, cx));
        });
        runner.emit(Event::RunUserAskRequested {
            run_id,
            request_id,
            questions: user_ask_ui_questions(),
        });
        view.update_in(cx, |view, _, cx| {
            view.poll_events(Instant::now(), cx);
            cx.notify();
        });
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });

        let stack = cx.debug_bounds("user-ask-stack").unwrap();
        let composer = cx.debug_bounds("composer-surface").unwrap();
        assert!(stack.bottom() <= composer.top());
        assert!(stack.size.height <= px(680. * 0.36 + 1.));

        let option = format!("user-ask-option-{request_id}-target-workspace").leak();
        click_debug(cx, option);
        let next = format!("user-ask-next-{request_id}").leak();
        click_debug(cx, next);
        cx.run_until_parked();
        for option_id in ["tests", "clippy"] {
            let option = format!("user-ask-option-{request_id}-checks-{option_id}").leak();
            click_debug(cx, option);
        }
        click_debug(cx, next);
        cx.run_until_parked();
        let focused = format!("user-ask-option-{request_id}-scope-focused").leak();
        click_debug(cx, focused);
        view.update_in(cx, |view, window, cx| {
            view.user_ask_inputs
                .get(&(request_id, "scope".into()))
                .unwrap()
                .update(cx, |input, cx| input.focus(window, cx));
        });
        cx.simulate_input("整个工作区");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(runner.submitted_user_ask_answers().is_empty());
        view.update_in(cx, |view, _, cx| {
            view.select_user_ask_question(request_id, 3, cx)
        });
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        view.update_in(cx, |view, window, cx| {
            view.user_ask_inputs
                .get(&(request_id, "note".into()))
                .unwrap()
                .update(cx, |input, cx| input.focus(window, cx));
        });
        cx.simulate_input("保持精简");
        cx.run_until_parked();

        view.update_in(cx, |view, window, cx| view.toggle_settings(window, cx));
        view.update_in(cx, |view, window, cx| view.toggle_settings(window, cx));
        view.update_in(cx, |view, window, cx| {
            view.select_task(other_task, window, cx)
        });
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("user-ask-stack").is_none());
        view.update_in(cx, |view, window, cx| {
            view.select_task(active_task, window, cx)
        });
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("user-ask-stack").is_some());
        view.read_with(cx, |view, cx| {
            assert_eq!(view.prompt_input.read(cx).value(), "普通消息草稿");
            let request = &view.presenter.model().pending_user_asks[0];
            assert_eq!(
                request.drafts.get("scope"),
                Some(&UserAskAnswerValue::Text("整个工作区\n".into()))
            );
            assert_eq!(
                request.drafts.get("note"),
                Some(&UserAskAnswerValue::Text("保持精简".into()))
            );
        });

        let collapse = format!("user-ask-collapse-{request_id}").leak();
        click_debug(cx, collapse);
        cx.run_until_parked();
        assert!(
            cx.debug_bounds(&*format!("user-ask-question-{request_id}-note").leak())
                .is_none()
        );
        click_debug(cx, collapse);
        cx.run_until_parked();

        let submit = format!("user-ask-submit-{request_id}").leak();
        let submit_center = cx.debug_bounds(submit).unwrap().center();
        cx.simulate_click(submit_center, Default::default());
        cx.simulate_click(submit_center, Default::default());
        cx.run_until_parked();
        let submissions = runner.submitted_user_ask_answers();
        assert_eq!(submissions.len(), 1);
        assert_eq!(
            submissions[0]
                .iter()
                .map(|answer| (answer.question_id.as_str(), answer.value.clone()))
                .collect::<Vec<_>>(),
            vec![
                (
                    "target",
                    UserAskAnswerValue::Selected(vec!["workspace".into()])
                ),
                (
                    "checks",
                    UserAskAnswerValue::Selected(vec!["tests".into(), "clippy".into()])
                ),
                ("scope", UserAskAnswerValue::Text("整个工作区\n".into())),
                ("note", UserAskAnswerValue::Text("保持精简".into())),
            ]
        );

        runner.emit(Event::RunUserAskAnswerRejected {
            run_id,
            request_id,
            message: "原生请求暂不可用".into(),
        });
        view.update_in(cx, |view, _, cx| {
            view.poll_events(Instant::now(), cx);
            cx.notify();
        });
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        assert!(
            cx.debug_bounds(&*format!("user-ask-error-{request_id}").leak())
                .is_some()
        );
        view.update_in(cx, |view, _, cx| view.submit_user_ask(request_id, cx));
        assert_eq!(runner.submitted_user_ask_answers().len(), 2);

        runner.emit(Event::RunUserAskAnswerSent { run_id, request_id });
        view.update_in(cx, |view, _, cx| {
            view.poll_events(Instant::now(), cx);
            cx.notify();
        });
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        view.update_in(cx, |view, _, cx| view.submit_user_ask(request_id, cx));
        assert_eq!(runner.submitted_user_ask_answers().len(), 2);
        runner.emit(Event::RunUserAskFinished {
            run_id,
            request_id,
            status: nexus_domain::UserAskStatus::Answered,
            message: None,
        });
        view.update_in(cx, |view, _, cx| {
            view.poll_events(Instant::now(), cx);
            cx.notify();
        });
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("user-ask-stack").is_none());
        assert!(view.read_with(cx, |view, _| view.user_ask_inputs.is_empty()));
    }

    #[gpui::test]
    fn user_ask_panel_preserves_native_question_ids_and_option_labels(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, runner, _directory) = fixture();
        assert!(presenter.submit("native questions", "claude"));
        let run_id = presenter.model().active_run.unwrap();
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
        cx.simulate_resize(gpui::size(px(1040.), px(680.)));
        // Claude keys answers by full question text, Codex by explicit ID, OMP by dialog ID.
        for (question_id, choice) in [("Which checks?", true), ("checks", true), ("ui_1", false)] {
            let request_id = Uuid::new_v4();
            runner.emit(Event::RunUserAskRequested {
                run_id,
                request_id,
                questions: vec![UserAskQuestion {
                    id: question_id.into(),
                    prompt: "Which checks?".into(),
                    answer_mode: if choice {
                        UserAskAnswerMode::Choice {
                            multiple: false,
                            allow_custom: true,
                        }
                    } else {
                        UserAskAnswerMode::Text
                    },
                    options: if choice {
                        vec![UserAskOption {
                            id: "Run tests".into(),
                            label: "Run tests".into(),
                            description: Some("Verify the change".into()),
                        }]
                    } else {
                        vec![]
                    },
                }],
            });
            view.update_in(cx, |view, _, cx| view.poll_events(Instant::now(), cx));
            cx.run_until_parked();
            let question_selector = format!("user-ask-question-{request_id}-{question_id}").leak();
            assert!(cx.debug_bounds(question_selector).is_some());
            let stack = cx.debug_bounds("user-ask-stack").unwrap();
            assert!(stack.bottom() <= cx.debug_bounds("composer-surface").unwrap().top());
            if choice {
                click_debug(
                    cx,
                    format!("user-ask-option-{request_id}-{question_id}-Run tests").leak(),
                );
            } else {
                view.update_in(cx, |view, window, cx| {
                    view.user_ask_inputs
                        .get(&(request_id, question_id.into()))
                        .unwrap()
                        .update(cx, |input, cx| input.focus(window, cx));
                });
                cx.simulate_input("Custom answer");
            }
            cx.run_until_parked();
            let count = runner.submitted_user_ask_answers().len();
            assert!(view.read_with(cx, |view, _| view.presenter.can_submit_user_ask(request_id)));
            // Choice + custom text exceeds the panel cap at the minimum window height.
            // Scroll the panel, rather than clicking the clipped button's layout bounds.
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: gpui::point(stack.left() + px(4.), stack.center().y),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-400.))),
                ..Default::default()
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
                let _ = window.draw(cx);
            });
            let submit_selector = format!("user-ask-submit-{request_id}").leak();
            let submit_bounds = cx.debug_bounds(submit_selector).unwrap();
            assert!(
                submit_bounds.bottom() <= stack.bottom(),
                "submit={submit_bounds:?}, stack={stack:?}"
            );
            click_debug(cx, submit_selector);
            cx.run_until_parked();
            let submissions = runner.submitted_user_ask_answers();
            assert_eq!(submissions.len(), count + 1);
            assert_eq!(
                submissions.last().unwrap(),
                &vec![nexus_domain::UserAskAnswer {
                    question_id: question_id.into(),
                    value: if choice {
                        UserAskAnswerValue::Selected(vec!["Run tests".into()])
                    } else {
                        UserAskAnswerValue::Text("Custom answer".into())
                    },
                }]
            );
            runner.emit(Event::RunUserAskAnswerSent { run_id, request_id });
            runner.emit(Event::RunUserAskFinished {
                run_id,
                request_id,
                status: nexus_domain::UserAskStatus::Answered,
                message: None,
            });
            view.update_in(cx, |view, _, cx| view.poll_events(Instant::now(), cx));
            cx.run_until_parked();
            assert!(cx.debug_bounds("user-ask-stack").is_none());
        }
    }

    #[gpui::test]
    fn user_ask_panel_clamps_long_content_above_the_composer(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, runner, _directory) = fixture();
        assert!(presenter.submit("long question", "claude"));
        let run_id = presenter.model().active_run.unwrap();
        let request_id = Uuid::new_v4();
        runner.emit(Event::RunUserAskRequested {
            run_id,
            request_id,
            questions: vec![UserAskQuestion {
                id: "long".into(),
                prompt: "这是一个用于验证最小窗口布局的很长问题。".repeat(18),
                answer_mode: UserAskAnswerMode::Choice {
                    multiple: false,
                    allow_custom: false,
                },
                options: vec![UserAskOption {
                    id: "long-option".into(),
                    label: "这个选项同样很长，需要在面板内完整换行并保持可滚动。".repeat(14),
                    description: Some("补充说明不能越过面板边界或覆盖输入区。".repeat(12)),
                }],
            }],
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
            });
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
                let _ = window.draw(cx);
            });
            let stack = cx.debug_bounds("user-ask-stack").unwrap();
            let composer = cx.debug_bounds("composer-surface").unwrap();
            let stop = cx.debug_bounds("composer-cancel").unwrap();
            let question = cx
                .debug_bounds(&*format!("user-ask-question-{request_id}-long").leak())
                .unwrap();
            assert!(stack.left() >= px(0.) && stack.right() <= px(width));
            assert!(stack.size.height <= px(height * 0.36 + 1.));
            assert!(stack.bottom() <= composer.top());
            assert!(composer.bottom() <= px(height));
            assert!(stop.bottom() <= composer.bottom());
            assert!(question.left() >= stack.left() && question.right() <= stack.right());
        }
    }

    #[gpui::test]
    fn approval_dialog_renders_options_returns_the_choice_and_closes_on_resolution(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, runner, _directory) = fixture();
        // Keep the dialog's slide-in animation from moving click targets between frames.
        assert!(presenter.set_appearance(AppearanceSettings {
            reduced_motion: true,
            ..Default::default()
        }));
        assert!(presenter.submit("approval task", "claude"));
        let run_id = presenter.model().active_run.unwrap();
        let (root, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| NexusView::new(presenter, window, cx));
            gpui_kit::component::Root::new(view, window, cx)
        });
        let view = root.read_with(cx, |root, _| {
            root.view().clone().downcast::<NexusView>().unwrap()
        });
        cx.simulate_resize(gpui::size(px(1040.), px(680.)));
        let request_id = Uuid::new_v4();
        runner.emit(nexus_protocol::Event::RunApprovalRequested {
            run_id,
            request: nexus_protocol::ApprovalRequest {
                request_id,
                title: "Bash".into(),
                details: "echo approved\n".repeat(100),
                options: vec!["Approve".into(), "Deny".into()],
            },
        });
        view.update_in(cx, |view, window, cx| {
            view.poll_events(Instant::now(), cx);
            view.sync_approval_dialog(window, cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("dialog-layer").is_some());
        let details = cx.debug_bounds("approval-details").unwrap();
        assert!(details.size.height <= px(680. * 0.45));
        assert!(details.left() >= px(0.) && details.right() <= px(1040.));
        let approve = cx.debug_bounds("approval-option-0").unwrap();
        assert!(approve.bottom() <= px(680.));
        view.update_in(cx, |view, window, cx| {
            assert!(!view.prompt_input.focus_handle(cx).is_focused(window));
        });
        cx.simulate_keystrokes("enter escape");
        assert!(view.read_with(cx, |view, _| {
            view.presenter.model().responding_approval.is_none()
        }));
        click_debug(cx, "approval-option-0");
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.presenter.model().responding_approval),
            Some(request_id)
        );
        runner.emit(nexus_protocol::Event::RunApprovalResolved { run_id, request_id });
        view.update_in(cx, |view, window, cx| {
            view.poll_events(Instant::now(), cx);
            view.sync_approval_dialog(window, cx);
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("dialog-layer").is_none());
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
