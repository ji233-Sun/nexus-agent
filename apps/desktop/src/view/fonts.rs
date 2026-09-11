use super::*;
use crate::{i18n::LocalizedText, infrastructure::fonts, model::FontSettings};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};

#[derive(Clone)]
pub(super) struct FontChoice {
    family: String,
    label: SharedString,
    available: bool,
}

impl SearchableListItem for FontChoice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.label.clone()
    }
    fn value(&self) -> &String {
        &self.family
    }
    fn disabled(&self) -> bool {
        !self.available
    }
}

pub(super) struct FontControls {
    pub(super) reading: Entity<SelectState<SearchableVec<FontChoice>>>,
    pub(super) code: Entity<SelectState<SearchableVec<FontChoice>>>,
    importing: bool,
    status: Option<Result<LocalizedText, LocalizedText>>,
}

fn font_choices(settings: &FontSettings, locale: Language, cx: &gpui::App) -> Vec<FontChoice> {
    let mut choices = cx
        .text_system()
        .all_font_names()
        .into_iter()
        .filter(|name| !name.starts_with('.') || name == ".SystemUIFont")
        .map(|family| {
            let label = match family.as_str() {
                ".SystemUIFont" => locale.text("系统界面字体").to_owned(),
                "DengXian" => "等线 · DengXian".into(),
                "SimSun" => "宋体 · SimSun".into(),
                "NSimSun" => "新宋体 · NSimSun".into(),
                "Songti SC" => "宋体（简体） · Songti SC".into(),
                "Songti TC" => "宋体（繁体） · Songti TC".into(),
                _ => family.clone(),
            }
            .into();
            FontChoice {
                family,
                label,
                available: true,
            }
        })
        .collect::<Vec<_>>();
    for family in [&settings.reading, &settings.code].into_iter().flatten() {
        if !choices.iter().any(|choice| &choice.family == family) {
            choices.push(FontChoice {
                family: family.clone(),
                label: locale
                    .format(
                        "{font}（不可用，使用默认字体）",
                        &[("font", family.clone())],
                    )
                    .into(),
                available: false,
            });
        }
    }
    choices
}

impl NexusView {
    pub(super) fn new_font_controls(
        settings: &FontSettings,
        locale: Language,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FontControls {
        let choices = font_choices(settings, locale, cx);
        let mut select = |value: &Option<String>| {
            cx.new(|cx| {
                let mut state =
                    SelectState::new(SearchableVec::new(choices.clone()), None, window, cx)
                        .searchable(true);
                if let Some(value) = value {
                    state.set_selected_value(value, window, cx);
                }
                state
            })
        };
        let controls = FontControls {
            reading: select(&settings.reading),
            code: select(&settings.code),
            importing: false,
            status: None,
        };
        for (select, code) in [(&controls.reading, false), (&controls.code, true)] {
            cx.subscribe_in(
                select,
                window,
                move |view, _, event: &SelectEvent<SearchableVec<FontChoice>>, window, cx| {
                    let SelectEvent::Confirm(family) = event;
                    let mut settings = view.presenter.model().fonts.clone();
                    if code {
                        settings.code = family.clone();
                    } else {
                        settings.reading = family.clone();
                    }
                    view.set_fonts(settings, window, cx);
                },
            )
            .detach();
        }
        controls
    }

    pub(super) fn sync_font_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = &self.presenter.model().fonts;
        let choices = font_choices(settings, self.presenter.model().language, cx);
        for (select, value) in [
            (&self.font_controls.reading, &settings.reading),
            (&self.font_controls.code, &settings.code),
        ] {
            select.update(cx, |select, cx| {
                select.set_items(SearchableVec::new(choices.clone()), window, cx);
                select.set_selected_value(&value.clone().unwrap_or_default(), window, cx);
                cx.notify();
            });
        }
    }

    pub(super) fn set_fonts(
        &mut self,
        settings: FontSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.presenter.set_fonts(settings) {
            Ok(()) => {
                configure_fonts(&self.presenter.model().fonts, cx);
                self.refresh_appearance(window, cx);
                window.refresh();
                self.font_controls.status = None;
            }
            Err(error) => {
                self.font_controls.status = Some(Err(LocalizedText::new(
                    "无法保存字体偏好：{error}",
                    &[("error", error.to_string())],
                )))
            }
        }
        self.sync_font_controls(window, cx);
        cx.notify();
    }

    fn import_font(&mut self, _: &gpui::ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.font_controls.importing {
            return;
        }
        self.font_controls.importing = true;
        self.font_controls.status = None;
        cx.notify();
        let text_system = cx.text_system().clone();
        let title = self.presenter.model().language.text("导入字体");
        cx.spawn_in(window, async move |this, cx| {
            let result = if let Some(file) = rfd::AsyncFileDialog::new()
                .set_title(title)
                .add_filter("TrueType / OpenType", &["ttf", "otf"])
                .pick_file()
                .await
            {
                Some(
                    cx.background_executor()
                        .spawn(async move {
                            fonts::directory().and_then(|directory| {
                                fonts::import(&directory, file.path(), &text_system)
                            })
                        })
                        .await,
                )
            } else {
                None
            };
            let _ = this.update_in(cx, |view, window, cx| {
                view.font_controls.importing = false;
                view.font_controls.status = result.map(|result| match result {
                    Ok(()) => Ok(LocalizedText::from("已导入字体，可在上方列表中选择。")),
                    Err(error) => Err(LocalizedText::new(
                        "字体导入失败：{error}",
                        &[("error", format!("{error:#}"))],
                    )),
                });
                view.sync_font_controls(window, cx);
                configure_fonts(&view.presenter.model().fonts, cx);
                view.refresh_appearance(window, cx);
                window.refresh();
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_font_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let selector = |select, label| {
            Select::new(select)
                .w_full()
                .accessibility_label(locale.text(label))
                .placeholder(locale.text("默认字体"))
                .search_placeholder(locale.text("搜索字体，例如宋体、等线…"))
                .cleanable(true)
                .menu_max_h(px(280.))
        };
        settings::settings_group(
            colors,
            locale.text("字体"),
            [
                settings::settings_row(
                    colors,
                    locale.text("正文字体"),
                    locale.text("用于助手回复中的正文与标题。"),
                    div().w_full().debug_selector(|| "font-reading-select".into())
                        .child(selector(&self.font_controls.reading, "正文字体")),
                ),
                settings::settings_row(
                    colors,
                    locale.text("代码字体"),
                    locale.text("用于代码块、工具输出与差异；建议选择等宽字体。"),
                    div().w_full().debug_selector(|| "font-code-select".into())
                        .child(selector(&self.font_controls.code, "代码字体")),
                ),
                div().py_4().flex().flex_col().gap_3()
                    .child(
                        div().text_size(px(12.)).text_color(rgb(colors.muted))
                            .child(locale.text("选择本机字体，或导入 TTF / OTF 文件。导入的字体保存在应用中，重启后仍可使用。")),
                    )
                    .child(
                        div().flex().gap_2()
                            .child(
                                Button::new("import-font")
                                    .debug_selector(|| "font-import".into())
                                    .outline()
                                    .label(locale.text(if self.font_controls.importing { "正在导入…" } else { "导入字体" }))
                                    .disabled(self.font_controls.importing)
                                    .on_click(cx.listener(Self::import_font)),
                            )
                            .child(
                                Button::new("reset-fonts")
                                    .debug_selector(|| "font-reset".into())
                                    .ghost()
                                    .label(locale.text("恢复默认字体"))
                                    .disabled(self.presenter.model().fonts == FontSettings::default())
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        view.set_fonts(FontSettings::default(), window, cx)
                                    })),
                            ),
                    )
                    .when_some(self.font_controls.status.as_ref(), |element, status| {
                        let (text, color) = match status {
                            Ok(text) => (text, colors.success),
                            Err(text) => (text, colors.danger),
                        };
                        element.child(
                            div().debug_selector(|| "font-import-status".into())
                                .text_size(px(12.)).text_color(rgb(color))
                                .child(text.render(locale).to_owned()),
                        )
                    }),
                div().py_4().flex().flex_col().gap_3()
                    .child(
                        div().text_size(px(12.)).text_color(rgb(colors.muted))
                            .child(locale.text("字体预览")),
                    )
                    .child(
                        div().debug_selector(|| "font-preview-reading".into())
                            .font(reading_font(cx)).text_size(px(17.)).line_height(px(30.))
                            .child(locale.text("清晰的文字，让思路自然展开。The quick brown fox · 0123456789")),
                    )
                    .child(
                        div().debug_selector(|| "font-preview-code".into())
                            .font_family(mono_font(cx)).text_size(px(14.)).line_height(px(22.))
                            .px_3().py_2().rounded(px(CONTROL_RADIUS)).bg(rgb(colors.surface))
                            .child("fn main() { println!(\"Hello, 世界!\"); }"),
                    ),
            ],
        )
    }
}
