use super::settings::settings_group;
use super::*;
use crate::infrastructure::voice::{self, Provider};
use gpui_kit::component::menu::DropdownMenu as _;

impl NexusView {
    pub(super) fn poll_voice_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.presenter.model().voice.operation.is_some();
        if let Some(text) = self.presenter.poll_voice() {
            self.append_voice_text(&text, window, cx);
        }
        if active {
            cx.notify();
        }
    }

    pub(super) fn append_voice_text(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Read at completion, not recording start: preserve edits made during recognition.
        self.prompt_input.update(cx, |input, cx| {
            let draft = input.value();
            let combined = if draft.is_empty() {
                text.to_owned()
            } else {
                format!("{draft}\n{text}")
            };
            input.replace_all(combined, window, cx);
        });
    }

    pub(super) fn render_voice_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let ui_locale = model.language;
        let voice = &model.voice;
        let colors = palette(cx);
        let label = ui_locale.text(if voice.operation.is_some() {
            "停止录音"
        } else if voice.ready() {
            "语音输入"
        } else {
            "配置语音输入"
        });
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                Button::new("voice-record")
                    .ghost()
                    .small()
                    .debug_selector(|| "voice-record".into())
                    .size(px(CONTROL_HEIGHT))
                    .p_0()
                    .accessibility_label(label)
                    .tooltip(label)
                    .when(voice.operation.is_some(), |button| {
                        button.danger().outline()
                    })
                    .child(if voice.operation.is_some() {
                        Icon::new(IconName::Pause).size(px(16.)).into_any_element()
                    } else {
                        gpui::svg()
                            .data(include_bytes!("../../assets/icons/microphone.svg").as_slice())
                            .size(px(16.))
                            .text_color(rgb(colors.text_secondary))
                            .into_any_element()
                    })
                    .disabled(model.selected_codex_thread.is_some() || voice.transcribing)
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.presenter.model().voice.operation.is_some() {
                            this.presenter.stop_voice();
                        } else if !this.presenter.model().voice.ready() {
                            this.settings_open = true;
                            this.select_settings_section(SettingsSection::Voice, window, cx);
                        } else if let Err(error) = this.presenter.start_voice() {
                            this.presenter.voice_error(error);
                        }
                        cx.notify();
                    })),
            )
            .when(voice.operation.is_some(), |row| {
                row.child(
                    Button::new("voice-cancel")
                        .ghost()
                        .small()
                        .size(px(COMPACT_CONTROL_HEIGHT))
                        .p_0()
                        .icon(IconName::Close)
                        .accessibility_label(ui_locale.text("取消语音"))
                        .tooltip(ui_locale.text("取消语音"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.presenter.cancel_voice();
                            cx.notify();
                        })),
                )
            })
    }

    pub(super) fn render_voice_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let ui_locale = model.language;
        let voice = &model.voice;
        let selected = voice.settings.provider;
        let colors = palette(cx);
        div().debug_selector(|| "voice-settings".into()).flex().flex_col().gap_8()
            .child(settings_group(colors, ui_locale.text("语音输入 Provider"), [
                div().py_4().flex().flex_col().gap_4()
            .child(div().text_size(px(13.)).text_color(rgb(colors.muted)).line_height(relative(1.6))
                .child(ui_locale.text("先选择 Provider，再完成配置。语音只回填可编辑草稿，不会自动发送。")))
            .child(div().flex().gap_2().children(voice::supported_providers().iter().copied().map(|provider| {
                Button::new(match provider { Provider::MacOs => "voice-macos", Provider::Mimo => "voice-mimo" })
                    .outline().small().h(px(CONTROL_HEIGHT)).flex_1().min_w_0()
                    .label(ui_locale.text(match provider { Provider::MacOs => "macOS 系统语音识别", Provider::Mimo => "MiMo ASR" }))
                    .selected(selected == Some(provider))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Err(error) = this.presenter.select_voice_provider(provider) { this.presenter.voice_error(error); }
                        this.voice_key_input.update(cx, |input, cx| input.set_value("", window, cx));
                        cx.notify();
                    }))
            })))
            .when(selected.is_none(), |el| el.child(div().text_size(px(12.)).text_color(rgb(colors.muted)).child(ui_locale.text("尚未选择 Provider。"))))
            ]))
            .when(selected == Some(Provider::Mimo), |el| el.child(
                settings_group(colors, ui_locale.text("MiMo ASR"), [
                div().debug_selector(|| "voice-mimo-config".into()).py_4().flex().flex_col().gap_4()
                    .child(ui_locale.text(if voice.mimo_configured { "已配置（系统凭据库）；不代表服务请求已验证。" } else { "待配置：保存 API Key 后才能录音。" }))
                    .child(div().text_size(px(12.)).text_color(rgb(colors.muted)).line_height(relative(1.6))
                        .child(ui_locale.text("停止录音后，音频将发送给 MiMo。最长 60 秒，Base64 音频不超过 10 MB。保存 Key 不会上传音频。")))
                    .child(Input::new(&self.voice_key_input))
                    .child(Button::new("voice-save-key").primary().small().h(px(CONTROL_HEIGHT)).label(ui_locale.text("保存 API Key")).on_click(cx.listener(|this, _, window, cx| {
                        let key = this.voice_key_input.read(cx).value().to_string();
                        match this.presenter.save_voice_key(&key) {
                            Ok(()) => this.voice_key_input.update(cx, |input, cx| input.set_value("", window, cx)),
                            Err(error) => this.presenter.voice_error(error),
                        }
                        cx.notify();
                    })))
                ])
            ))
            .when(selected == Some(Provider::MacOs), |el| el.child(
                settings_group(colors, ui_locale.text("macOS 系统语音识别"), [
                div().py_4().flex().flex_col().gap_4()
                    .child(div().text_size(px(13.)).text_color(rgb(colors.muted)).line_height(relative(1.6))
                        .child(ui_locale.text("无需 API Key。首次录音时请求麦克风和 Speech 权限；拒绝后请在系统设置中恢复。")))
                    .child(div().text_size(px(12.)).text_color(rgb(colors.muted)).line_height(relative(1.6))
                        .child(ui_locale.text("系统识别可能需要联网，音频可能发送给 Apple；每段最多 60 秒。")))
                    .child(div().text_size(px(13.)).line_height(relative(1.6)).child(voice::native_status(voice.settings.locale.as_deref())))
                    .child(Button::new("voice-locale").outline().small().h(px(CONTROL_HEIGHT)).label(voice.settings.locale.clone().unwrap_or_else(|| ui_locale.text("跟随系统语言").into()))
                        .dropdown_menu({ let view = cx.entity().downgrade(); move |menu, _, _| {
                            let mut menu = menu;
                            for locale in std::iter::once(None).chain(voice::native_locales().into_iter().map(Some)) {
                                let view = view.clone();
                                menu = menu.item(PopupMenuItem::new(locale.clone().unwrap_or_else(|| ui_locale.text("跟随系统语言").into()))
                                    .on_click(move |_, _, cx| { let _ = view.update(cx, |this, cx| {
                                        if let Err(error) = this.presenter.set_voice_locale(locale.clone()) { this.presenter.voice_error(error); }
                                        cx.notify();
                                    }); }));
                            }
                            menu
                        }}))
                    .child(Button::new("voice-permissions-refresh").ghost().small().h(px(CONTROL_HEIGHT)).icon(IconName::RotateCw).label(ui_locale.text("刷新权限状态")).on_click(cx.listener(|_, _, _, cx| cx.notify())))
                ])
            ))
            .when(!voice.status.render(ui_locale).is_empty(), |el| el.child(div().text_size(px(13.)).text_color(rgb(colors.text_secondary)).child(voice.status.render(ui_locale).to_owned())))
    }
}
