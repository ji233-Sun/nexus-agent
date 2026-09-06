use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CatalogModelChoice {
    FollowDefault,
    Model(String),
    Status(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CatalogModelItem {
    pub(super) choice: CatalogModelChoice,
    pub(super) title: String,
    pub(super) trigger_title: String,
    detail: String,
    pub(super) search_text: String,
    pub(super) disabled: bool,
}

impl CatalogModelItem {
    fn follow_default(title: String, trigger_title: String, search_text: String) -> Self {
        Self {
            choice: CatalogModelChoice::FollowDefault,
            detail: title.clone(),
            title,
            trigger_title,
            search_text,
            disabled: false,
        }
    }

    fn model(model: &ModelDescriptor, locale: Language) -> Self {
        let title = model.display_name.clone();
        let mut trigger_title = catalog_model_trigger_title(model);
        if !model.availability.is_selectable() {
            trigger_title.push_str(&format!(" · {}", locale.text("不可用")));
        }
        let provider = model.provider.as_deref().unwrap_or_default();
        let availability = match &model.availability {
            nexus_domain::ModelAvailability::Available => "",
            nexus_domain::ModelAvailability::Unknown => locale.text("未验证"),
            nexus_domain::ModelAvailability::Unavailable { reason } => reason,
        };
        Self {
            choice: CatalogModelChoice::Model(model.id.clone()),
            search_text: format!("{provider} {} {}", model.display_name, model.id),
            title,
            trigger_title,
            detail: format!("{} · {availability}", catalog_model_row_title(model))
                .trim_end_matches(" · ")
                .to_owned(),
            disabled: !model.availability.is_selectable(),
        }
    }

    fn unavailable(
        model_id: &str,
        name: Option<&str>,
        state: &ModelCatalogState,
        locale: Language,
    ) -> Self {
        let availability = match state {
            ModelCatalogState::Loading { .. } => locale.text("验证中"),
            ModelCatalogState::Ready(_) | ModelCatalogState::Empty => locale.text("不可用"),
            ModelCatalogState::Idle
            | ModelCatalogState::NotReady(_)
            | ModelCatalogState::Failed { .. } => locale.text("未验证"),
        };
        let title = format!("{} · {availability}", name.unwrap_or(model_id));
        Self {
            choice: CatalogModelChoice::Model(model_id.to_owned()),
            trigger_title: title.clone(),
            search_text: format!("{} {model_id} {availability}", name.unwrap_or(model_id)),
            title,
            detail: format!("{model_id} · {availability}"),
            disabled: true,
        }
    }

    fn status(title: String) -> Self {
        Self {
            choice: CatalogModelChoice::Status(title.clone()),
            trigger_title: title.clone(),
            search_text: title.clone(),
            detail: title.clone(),
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
pub(super) struct CatalogModelGroup {
    pub(super) title: String,
    pub(super) items: Vec<CatalogModelItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CatalogModelSelectContent {
    pub(super) groups: Vec<CatalogModelGroup>,
    pub(super) selected: CatalogModelChoice,
    locale: Language,
}

impl CatalogModelSelectContent {
    pub(super) fn from_model(model: &AppModel) -> Self {
        let locale = model.language;
        let selected = model
            .model_override
            .as_ref()
            .map(|model_id| CatalogModelChoice::Model(model_id.clone()))
            .unwrap_or(CatalogModelChoice::FollowDefault);
        let mut groups = vec![CatalogModelGroup {
            title: locale.text("默认").into(),
            items: vec![catalog_follow_default_item(model)],
        }];
        let catalog_models = model.model_catalog.models().unwrap_or_default();

        if let Some(model_id) = model.model_override.as_deref()
            && !catalog_models.iter().any(|entry| entry.id == model_id)
        {
            groups.push(CatalogModelGroup {
                title: locale.text("当前选择").into(),
                items: vec![CatalogModelItem::unavailable(
                    model_id,
                    model.model_override_name.as_deref(),
                    &model.model_catalog,
                    locale,
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
                .push(CatalogModelItem::model(descriptor, locale));
        }
        groups.extend(
            provider_groups
                .into_iter()
                .map(|(title, items)| CatalogModelGroup { title, items }),
        );

        let status = match &model.model_catalog {
            ModelCatalogState::Idle if model.selected_project.is_none() => {
                Some(locale.text("选择项目后加载模型目录").into())
            }
            ModelCatalogState::Idle => Some(locale.text("模型目录尚未加载").into()),
            ModelCatalogState::Loading { .. } => Some(locale.text("正在加载模型目录…").into()),
            ModelCatalogState::Empty => Some(locale.text("当前模型目录为空").into()),
            ModelCatalogState::NotReady(message) => Some(message.render(locale).to_owned()),
            ModelCatalogState::Failed { message, .. } => Some(locale.format(
                "模型目录加载失败：{message}",
                &[("message", message.render(locale).to_owned())],
            )),
            ModelCatalogState::Ready(_) => None,
        };
        if let Some(status) = status {
            groups.push(CatalogModelGroup {
                title: locale.text("状态").into(),
                items: vec![CatalogModelItem::status(status)],
            });
        }

        Self {
            groups,
            selected,
            locale,
        }
    }

    pub(super) fn selected_index(&self) -> Option<IndexPath> {
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
    let locale = model.language;
    let profile_model = model
        .selected_provider_profile()
        .and_then(|profile| profile.model.as_deref());
    if let Some(model_id) = profile_model {
        if let Some(descriptor) = model
            .model_catalog
            .models()
            .and_then(|models| models.iter().find(|entry| entry.id == model_id))
        {
            let item = CatalogModelItem::model(descriptor, locale);
            return CatalogModelItem::follow_default(
                locale.format("跟随 Profile 默认 · {0}", &[("0", item.detail)]),
                locale.format("默认 · {0}", &[("0", item.trigger_title)]),
                format!(
                    "default 默认 profile {} {}",
                    descriptor.display_name, descriptor.id
                ),
            );
        }
        let verification = match &model.model_catalog {
            ModelCatalogState::Loading { .. } => locale.text("验证中"),
            ModelCatalogState::Failed { .. } => locale.text("目录加载失败"),
            ModelCatalogState::Ready(_) | ModelCatalogState::Empty => locale.text("目录未验证"),
            ModelCatalogState::Idle | ModelCatalogState::NotReady(_) => locale.text("未验证"),
        };
        return CatalogModelItem::follow_default(
            locale.format(
                "跟随 Profile 默认 · {model_id}（{verification}）",
                &[
                    ("model_id", (model_id).to_string()),
                    ("verification", (verification).to_string()),
                ],
            ),
            locale.format(
                "默认 · {model_id} · {verification}",
                &[
                    ("model_id", (model_id).to_string()),
                    ("verification", (verification).to_string()),
                ],
            ),
            format!("default 默认 profile {model_id} {verification}"),
        );
    }

    if let Some(descriptor) = model
        .model_catalog
        .models()
        .and_then(|models| models.iter().find(|model| model.is_default))
    {
        let item = CatalogModelItem::model(descriptor, locale);
        return CatalogModelItem::follow_default(
            locale.format("跟随 CLI 默认 · {0}", &[("0", item.detail)]),
            locale.format("CLI 默认 · {0}", &[("0", item.trigger_title)]),
            format!(
                "default 默认 cli {} {}",
                descriptor.display_name, descriptor.id
            ),
        );
    }

    let suffix = match &model.model_catalog {
        ModelCatalogState::Loading { .. } => locale.text(" · 目录加载中"),
        ModelCatalogState::Failed { .. } => locale.text(" · 目录加载失败"),
        ModelCatalogState::Empty => locale.text(" · 目录为空"),
        ModelCatalogState::Idle | ModelCatalogState::Ready(_) => "",
        ModelCatalogState::NotReady(_) => locale.text(" · 未就绪"),
    };
    CatalogModelItem::follow_default(
        locale.text("跟随 CLI 默认模型").into(),
        locale.format("CLI 默认模型{suffix}", &[("suffix", (suffix).to_string())]),
        "default 默认 cli".into(),
    )
}

pub(super) struct ModelPickerList {
    content: CatalogModelSelectContent,
    groups: Vec<CatalogModelGroup>,
    query: String,
}

impl ModelPickerList {
    pub(super) fn new(content: CatalogModelSelectContent) -> Self {
        Self {
            groups: content.groups.clone(),
            content,
            query: String::new(),
        }
    }

    pub(super) fn replace_content(&mut self, content: CatalogModelSelectContent) {
        self.content = content;
        self.filter();
    }

    fn filter(&mut self) {
        self.groups = self
            .content
            .groups
            .iter()
            .filter_map(|group| {
                let items: Vec<_> = group
                    .items
                    .iter()
                    .filter(|item| item.matches(&self.query))
                    .cloned()
                    .collect();
                (!items.is_empty()).then(|| CatalogModelGroup {
                    title: group.title.clone(),
                    items,
                })
            })
            .collect();
    }

    pub(super) fn item(&self, index: IndexPath) -> Option<&CatalogModelItem> {
        self.groups.get(index.section)?.items.get(index.row)
    }

    pub(super) fn selected_index(&self) -> Option<IndexPath> {
        self.groups
            .iter()
            .enumerate()
            .find_map(|(section, group)| {
                group
                    .items
                    .iter()
                    .position(|item| item.choice == self.content.selected)
                    .map(|row| IndexPath::new(row).section(section))
            })
            .or_else(|| {
                self.groups.iter().enumerate().find_map(|(section, group)| {
                    group
                        .items
                        .iter()
                        .position(|item| !item.disabled)
                        .map(|row| IndexPath::new(row).section(section))
                })
            })
    }
}

impl ListDelegate for ModelPickerList {
    type Item = ListItem;

    fn sections_count(&self, _: &gpui::App) -> usize {
        self.groups.len()
    }

    fn items_count(&self, section: usize, _: &gpui::App) -> usize {
        self.groups
            .get(section)
            .map_or(0, |group| group.items.len())
    }

    fn set_selected_index(
        &mut self,
        _: Option<IndexPath>,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) {
    }

    fn perform_search(
        &mut self,
        query: &str,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> gpui::Task<()> {
        self.query = query.to_owned();
        self.filter();
        gpui::Task::ready(())
    }

    fn render_item(
        &mut self,
        index: IndexPath,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let item = self.item(index)?;
        let colors = palette(cx);
        let detail = item.detail.clone();
        Some(
            ListItem::new(("model-row", index.section * 100_000 + index.row))
                .debug_selector(move || format!("model-row-{}-{}", index.section, index.row))
                .h(px(54.))
                .min_w_0()
                .overflow_hidden()
                .px_3()
                .disabled(item.disabled)
                .confirmed(item.choice == self.content.selected)
                .check_icon(IconName::Check)
                .tooltip(move |window, cx| Tooltip::new(detail.clone()).build(window, cx))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_ellipsis()
                                .text_size(px(14.))
                                .child(item.title.clone()),
                        )
                        .child(
                            div()
                                .text_ellipsis()
                                .text_size(px(12.))
                                .text_color(rgb(colors.muted))
                                .child(item.detail.clone()),
                        ),
                ),
        )
    }

    fn render_section_header(
        &mut self,
        section: usize,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        Some(
            div()
                .h(px(26.))
                .px_3()
                .pt_1()
                .text_size(px(12.))
                .text_color(rgb(palette(cx).muted))
                .child(self.groups.get(section)?.title.clone()),
        )
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        div()
            .p_4()
            .text_color(rgb(palette(cx).muted))
            .child(self.content.locale.text("没有匹配的模型"))
    }
}

impl NexusView {
    pub(super) fn model_selector(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let model = self.presenter.model();
        let locale = model.language;
        let content = &self.catalog_model_select_content;
        let selected = content
            .selected_index()
            .and_then(|index| content.groups.get(index.section)?.items.get(index.row));
        let label = selected
            .map(|item| item.trigger_title.clone())
            .unwrap_or_else(|| locale.text("跟随默认").into());
        let tooltip = format!(
            "{} · {}\n{}",
            model.selected_harness,
            model
                .selected_provider_profile()
                .map_or(locale.text("CLI 默认配置"), |profile| profile
                    .name
                    .as_str()),
            selected.map_or("", |item| item.detail.as_str())
        );
        let button = Button::new("composer-model")
            .debug_selector(|| "composer-model".into())
            .ghost()
            .small()
            .h(px(COMPACT_CONTROL_HEIGHT))
            .max_w(px(300.))
            .min_w_0()
            .child(harness_icon(model.selected_harness, palette(cx), 16.))
            .label(label)
            .tooltip(tooltip)
            .accessibility_label(locale.text("选择 Harness、配置和模型"))
            .child(Icon::new(IconName::ChevronDown).size(px(14.)));
        if model.active_run.is_some() {
            return button.disabled(true).into_any_element();
        }
        let app = cx.entity();
        Popover::new("model-picker")
            .anchor(Anchor::BottomLeft)
            .appearance(false)
            .open(self.model_picker_open)
            .track_focus(&self.catalog_model_select.focus_handle(cx))
            .trigger(button)
            .on_open_change(move |open, window, cx| {
                app.update(cx, |app, cx| {
                    app.model_picker_open = *open;
                    if *open {
                        app.catalog_model_select.update(cx, |list, cx| {
                            list.set_query("", window, cx);
                            let selected = list.delegate().selected_index();
                            list.set_selected_index(selected, window, cx);
                            list.scroll_to_selected_item(window, cx);
                        });
                    }
                    cx.notify();
                });
            })
            .when(self.model_picker_open, |popover| {
                popover.child(self.render_model_picker(window, cx))
            })
            .into_any_element()
    }

    fn render_model_picker(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let locale = model.language;
        let colors = palette(cx);
        let material = materials(cx);
        let width = px(680.).min(window.viewport_size().width - px(32.));
        let height = px(420.).min(window.viewport_size().height - px(64.));
        div()
            .id("model-picker-surface")
            .debug_selector(|| "model-picker-surface".into())
            .w(width)
            .h(height)
            .flex()
            .rounded(px(CARD_RADIUS))
            .overflow_hidden()
            .bg(material.floating)
            .border_1()
            .border_color(material.edge)
            .shadow(material.shadow())
            .child(
                div()
                    .id("model-configurations")
                    .debug_selector(|| "model-configurations".into())
                    .w(px(190.))
                    .h_full()
                    .flex_none()
                    .overflow_y_scroll()
                    .p_2()
                    .border_r_1()
                    .border_color(material.edge)
                    .children(HarnessKind::ALL.map(|harness| {
                        let probe = model.harnesses.get(&harness);
                        let profiles = model
                            .provider_profiles
                            .iter()
                            .filter(|profile| profile.harness == harness);
                        div()
                            .mb_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .h(px(30.))
                                    .px_2()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors.text_secondary))
                                    .child(harness_icon(harness, colors, 16.))
                                    .child(harness.to_string()),
                            )
                            .child(self.model_configuration_row(
                                harness,
                                None,
                                locale.text("CLI 默认配置"),
                                probe.is_some_and(|probe| probe.available && probe.authenticated),
                                cx,
                            ))
                            .children(profiles.map(|profile| {
                                self.model_configuration_row(
                                    harness,
                                    Some(profile.id),
                                    &profile.name,
                                    probe.is_some_and(|probe| probe.available)
                                        && profile.credential_configured,
                                    cx,
                                )
                            }))
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(rgb(colors.surface))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .h(px(38.))
                            .px_3()
                            .flex_none()
                            .child(
                                div()
                                    .min_w_0()
                                    .text_ellipsis()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors.text_secondary))
                                    .child(model.selected_provider_profile().map_or_else(
                                        || locale.text("CLI 默认配置").to_owned(),
                                        |profile| profile.name.clone(),
                                    )),
                            )
                            .child(
                                Button::new("composer-model-refresh")
                                    .ghost()
                                    .small()
                                    .icon(IconName::RotateCw)
                                    .tooltip(locale.text("刷新模型目录"))
                                    .accessibility_label(locale.text("刷新模型目录"))
                                    .disabled(model.selected_project.is_none())
                                    .on_click(cx.listener(Self::refresh_model_catalog)),
                            ),
                    )
                    .child(
                        div().flex_1().min_h_0().min_w_0().child(
                            List::new(&self.catalog_model_select)
                                .search_placeholder(locale.text("按 Provider、名称或模型 ID 搜索"))
                                .size_full(),
                        ),
                    )
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .flex_none()
                            .border_t_1()
                            .border_color(rgb(colors.border))
                            .text_size(px(11.))
                            .text_color(rgb(colors.muted))
                            .child(locale.text("↑ ↓ 浏览 · Enter 选择 · Esc 关闭")),
                    ),
            )
    }

    fn model_configuration_row(
        &self,
        harness: HarnessKind,
        profile_id: Option<Uuid>,
        name: &str,
        ready: bool,
        cx: &mut Context<Self>,
    ) -> Button {
        let model = self.presenter.model();
        let locale = model.language;
        let colors = palette(cx);
        let selected = model.selected_harness == harness
            && model.selected_provider_profile().map(|profile| profile.id) == profile_id;
        let id = format!(
            "model-config-{}-{}",
            harness.as_str(),
            profile_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "cli".into())
        );
        let selector = id.clone();
        Button::new(id)
            .debug_selector(move || selector.clone())
            .ghost()
            .small()
            .w_full()
            .h(px(44.))
            .selected(selected)
            .justify_start()
            .min_w_0()
            .tooltip(format!("{harness} · {name}"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_left()
                    .child(div().text_ellipsis().child(name.to_owned()))
                    .when(!ready, |row| {
                        row.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(colors.muted))
                                .child(locale.text("未就绪")),
                        )
                    }),
            )
            .when(selected, |row| {
                row.child(Icon::new(IconName::Check).size(px(14.)))
            })
            .on_click(cx.listener(move |app, _, window, cx| {
                let executable = app.executable_input.read(cx).value().to_string();
                if app
                    .presenter
                    .select_model_configuration(harness, profile_id, &executable)
                {
                    app.sync_executable(window, cx);
                    app.sync_provider_profile_form(profile_id, window, cx);
                    app.presenter.notify_remote_changed();
                }
                app.catalog_model_select
                    .update(cx, |list, cx| list.focus(window, cx));
                cx.notify();
            }))
    }
}
