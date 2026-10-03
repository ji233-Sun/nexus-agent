use super::settings::{settings_group, settings_row};
use super::*;

/// The profile form owns its fields and all transitions between saved profiles and drafts.
pub(super) struct ProviderProfileForm {
    pub(super) id: Option<Uuid>,
    pub(super) name: Entity<InputState>,
    pub(super) api_key_env: Entity<InputState>,
    pub(super) api_key: Entity<InputState>,
    pub(super) base_url_env: Entity<InputState>,
    pub(super) base_url: Entity<InputState>,
    pub(super) model: Entity<InputState>,
}

impl ProviderProfileForm {
    pub(super) fn new(
        profile: Option<&ProviderProfile>,
        harness: HarnessKind,
        language: Language,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> Self {
        let draft = profile_form_draft(profile, harness);
        let mut field = |value: String, masked: bool| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(value)
                    .masked(masked)
            })
        };
        let form = Self {
            id: draft.id,
            name: field(draft.name, false),
            api_key_env: field(draft.api_key_env, false),
            api_key: field(draft.api_key, true),
            base_url_env: field(draft.base_url_env, false),
            base_url: field(draft.base_url, false),
            model: field(draft.model, false),
        };
        form.set_language(language, window, cx);
        form
    }

    pub(super) fn draft(&self, cx: &gpui::App) -> ProviderProfileDraft {
        ProviderProfileDraft {
            id: self.id,
            name: self.name.read(cx).value().to_string(),
            api_key_env: self.api_key_env.read(cx).value().to_string(),
            api_key: self.api_key.read(cx).value().to_string(),
            base_url_env: self.base_url_env.read(cx).value().to_string(),
            base_url: self.base_url.read(cx).value().to_string(),
            model: self.model.read(cx).value().to_string(),
        }
    }

    fn reset(
        &mut self,
        profile: Option<&ProviderProfile>,
        harness: HarnessKind,
        window: &mut Window,
        cx: &mut gpui::App,
    ) {
        let draft = profile_form_draft(profile, harness);
        self.id = draft.id;
        for (input, value) in [
            (&self.name, draft.name),
            (&self.api_key_env, draft.api_key_env),
            (&self.api_key, draft.api_key),
            (&self.base_url_env, draft.base_url_env),
            (&self.base_url, draft.base_url),
            (&self.model, draft.model),
        ] {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
    }

    pub(super) fn set_language(&self, language: Language, window: &mut Window, cx: &mut gpui::App) {
        for (input, placeholder) in [
            (&self.name, "例如 DeepSeek Production"),
            (&self.api_key_env, "例如 DEEPSEEK_API_KEY"),
            (&self.api_key, "新建时必填；编辑时留空保留"),
            (&self.base_url_env, "可选，例如 OPENAI_BASE_URL"),
            (&self.base_url, "可选，例如 https://api.example.com/v1"),
            (&self.model, "可选，例如 deepseek/deepseek-v4-pro"),
        ] {
            input.update(cx, |input, cx| {
                input.set_placeholder(language.text(placeholder), window, cx)
            });
        }
    }
}

impl NexusView {
    pub(super) fn select_provider_profile(
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

    pub(super) fn new_provider_profile(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_provider_profile_form(None, window, cx);
        self.provider_form
            .name
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    pub(super) fn save_provider_profile(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let draft = self.provider_form.draft(cx);
        if let Some(profile_id) = self.presenter.save_provider_profile(draft) {
            self.sync_provider_profile_form(Some(profile_id), window, cx);
            self.presenter.notify_remote_changed();
        }
        cx.notify();
    }

    pub(super) fn delete_provider_profile(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile_id) = self.provider_form.id else {
            return;
        };
        if self.presenter.delete_provider_profile(profile_id) {
            self.sync_provider_profile_form(None, window, cx);
            self.presenter.notify_remote_changed();
        }
        cx.notify();
    }

    pub(super) fn sync_provider_profile_form(
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
        self.provider_form.reset(
            profile.as_ref(),
            self.presenter.model().conversation.selected_harness,
            window,
            cx,
        );
    }

    pub(super) fn render_provider_profiles(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let model = self.presenter.model();
        let active_run = model.active_run_count() > 0;
        let editing_profile = self.provider_form.id.and_then(|profile_id| {
            model
                .provider_profiles
                .iter()
                .find(|profile| profile.id == profile_id)
        });
        let form_title = editing_profile
            .map(|profile| locale.format("编辑 · {0}", &[("0", (profile.name).to_string())]))
            .unwrap_or_else(|| locale.text("新建 Provider Profile").into());
        let api_key_label = match editing_profile {
            Some(profile) if profile.credential_configured => locale.text("API Key · 已安全保存"),
            Some(_) => locale.text("API Key · 需要重新填写"),
            None => "API Key",
        };

        div()
            .flex()
            .flex_col()
            .gap_8()
            .child(settings_group(
                colors,
                locale.text("当前配置"),
                [
                    settings_row(
                        colors,
                        locale.text("执行引擎"),
                        locale.text("每种引擎分别管理自己的 Provider Profile。"),
                        self.harness_selector("settings-provider-harness", false, cx),
                    ),
                    settings_row(
                        colors,
                        "Provider Profile",
                        locale.text("选择已有配置，或使用 CLI 当前凭据。"),
                        self.provider_profile_selector(
                            "settings-provider-profile",
                            false,
                            true,
                            cx,
                        ),
                    ),
                ],
            ))
            .child(settings_group(
                colors,
                form_title,
                [
                    settings_row(
                        colors,
                        locale.text("名称"),
                        locale.text("用于区分不同服务商或账户。"),
                        Input::new(&self.provider_form.name)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::Bot).small()),
                    ),
                    settings_row(
                        colors,
                        locale.text("API Key 环境变量"),
                        locale.text("目标引擎读取 API Key 的环境变量名。"),
                        Input::new(&self.provider_form.api_key_env)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::SquareTerminal).small()),
                    ),
                    settings_row(
                        colors,
                        api_key_label,
                        locale.text("保存在系统凭据库；编辑时留空保留原值。"),
                        Input::new(&self.provider_form.api_key)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::EyeOff).small()),
                    ),
                    settings_row(
                        colors,
                        locale.text("Base URL 环境变量"),
                        locale.text("可选，目标引擎读取服务地址的环境变量名。"),
                        Input::new(&self.provider_form.base_url_env)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::SquareTerminal).small()),
                    ),
                    settings_row(
                        colors,
                        "Base URL",
                        locale.text("可选，自定义 API 服务地址。"),
                        Input::new(&self.provider_form.base_url)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::Globe).small()),
                    ),
                    settings_row(
                        colors,
                        locale.text("默认模型"),
                        locale.text("可选，使用此配置时优先选择的模型。"),
                        Input::new(&self.provider_form.model)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.))
                            .prefix(Icon::new(IconName::Cpu).small()),
                    ),
                    div()
                        .py_4()
                        .flex()
                        .items_center()
                        .flex_wrap()
                        .gap_3()
                        .child(
                            Button::new("new-provider-profile")
                                .ghost()
                                .small()
                                .h(px(CONTROL_HEIGHT))
                                .icon(IconName::Plus)
                                .label(locale.text("新建配置"))
                                .disabled(active_run)
                                .on_click(cx.listener(Self::new_provider_profile)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    Button::new("delete-provider-profile")
                                        .danger()
                                        .outline()
                                        .small()
                                        .size(px(CONTROL_HEIGHT))
                                        .icon(IconName::Delete)
                                        .tooltip(locale.text("删除当前 Provider Profile"))
                                        .disabled(active_run || editing_profile.is_none())
                                        .on_click(cx.listener(Self::delete_provider_profile)),
                                )
                                .child(
                                    Button::new("save-provider-profile")
                                        .primary()
                                        .small()
                                        .h(px(CONTROL_HEIGHT))
                                        .icon(IconName::Check)
                                        .label(locale.text("保存并启用"))
                                        .disabled(active_run)
                                        .on_click(cx.listener(Self::save_provider_profile)),
                                ),
                        ),
                ],
            ))
    }
}

pub(super) fn profile_form_draft(
    profile: Option<&ProviderProfile>,
    harness: HarnessKind,
) -> ProviderProfileDraft {
    let info = harness.info();
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
            api_key_env: info.api_key_env.into(),
            api_key: String::new(),
            base_url_env: info.base_url_env.into(),
            base_url: String::new(),
            model: String::new(),
        })
}

impl NexusView {
    pub(super) fn provider_profile_selector(
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
            .filter(|profile| profile.harness == model.conversation.selected_harness)
            .cloned()
            .collect::<Vec<_>>();
        let app = cx.entity().clone();
        let button_id = id;
        Button::new(button_id)
            .icon(IconName::Globe)
            .disabled(model.conversation.active_run.is_some())
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
}
