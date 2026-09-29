mod appearance;
mod general;
mod generation;
mod harnesses;
mod history;
mod remote;

use super::*;
use crate::i18n::probe_status;
use crate::infrastructure::harness_installation::documentation;
use crate::model::harness_installation::{InstallMethod, MaintenanceRequest};
use crate::model::updates::{UpdateChannel, UpdateState, installed_tag};
use gpui_kit::component::scroll::Scrollbar;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SettingsSection {
    General,
    Appearance,
    Agent,
    Providers,
    SourceControl,
    Voice,
    Remote,
    Archived,
    RuntimeLog,
}

impl SettingsSection {
    const ALL: [Self; 9] = [
        Self::General,
        Self::Appearance,
        Self::Agent,
        Self::Providers,
        Self::SourceControl,
        Self::Voice,
        Self::Remote,
        Self::Archived,
        Self::RuntimeLog,
    ];

    fn id(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Appearance => "appearance",
            Self::Agent => "agent",
            Self::Providers => "providers",
            Self::SourceControl => "source-control",
            Self::Voice => "voice",
            Self::Remote => "remote",
            Self::Archived => "archived",
            Self::RuntimeLog => "runtime-log",
        }
    }

    fn label(self, locale: Language) -> &'static str {
        match self {
            Self::General => locale.text("通用"),
            Self::Appearance => locale.text("外观"),
            Self::Agent => locale.text("执行引擎"),
            Self::Providers => locale.text("凭据配置"),
            Self::SourceControl => "Source Control",
            Self::Voice => locale.text("语音输入"),
            Self::Remote => locale.text("远程访问"),
            Self::Archived => locale.text("归档对话"),
            Self::RuntimeLog => locale.text("运行日志"),
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::General => IconName::Settings2,
            Self::Appearance => IconName::Palette,
            Self::Agent => IconName::Bot,
            Self::Providers => IconName::Cpu,
            Self::SourceControl => IconName::Network,
            Self::Voice => IconName::Play,
            Self::Remote => IconName::Globe,
            Self::Archived => IconName::Inbox,
            Self::RuntimeLog => IconName::FileText,
        }
    }
}

impl NexusView {
    pub(super) fn select_settings_section(
        &mut self,
        section: SettingsSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_section == section {
            return;
        }
        self.settings_section = section;
        if section == SettingsSection::SourceControl {
            for provider in IssueProvider::ALL {
                self.presenter.inspect_issues(provider);
            }
        }
        self.settings_scroll.set_offset(gpui::point(px(0.), px(0.)));
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    pub(super) fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let material = materials(cx);
        let section = self.settings_section;
        let content =
            match section {
                SettingsSection::General => self.render_general_settings(cx).into_any_element(),
                SettingsSection::Appearance => div()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .child(self.render_appearance_settings(cx))
                    .child(self.render_font_settings(cx))
                    .into_any_element(),
                SettingsSection::Agent => self.render_agent_settings(cx).into_any_element(),
                SettingsSection::Providers => self.render_provider_profiles(cx).into_any_element(),
                SettingsSection::SourceControl => div()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .children(IssueProvider::ALL.map(|provider| {
                        self.render_issue_settings(provider, cx).into_any_element()
                    }))
                    .into_any_element(),
                SettingsSection::Voice => self.render_voice_settings(cx).into_any_element(),
                SettingsSection::Remote => self.render_remote_settings(cx).into_any_element(),
                SettingsSection::Archived => self.render_archived_settings(cx).into_any_element(),
                SettingsSection::RuntimeLog => self.render_runtime_log(cx).into_any_element(),
            };
        let titlebar_inset = if cfg!(target_os = "macos") { 36. } else { 0. };

        div()
            .debug_selector(|| "settings-page".into())
            .size_full()
            .bg(material.chrome)
            .flex()
            .child(
                div()
                    .debug_selector(|| "settings-navigation".into())
                    .w(px(SIDEBAR_WIDTH))
                    .h_full()
                    .flex_none()
                    .pt(px(titlebar_inset))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(HEADER_HEIGHT))
                            .flex_none()
                            .px_5()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(brand_mark(32.))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("Nexus Agent"),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .px(px(14.))
                            .py_5()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .px(px(10.))
                                    .pb_3()
                                    .child(section_label(colors, locale.text("设置"))),
                            )
                            .children(SettingsSection::ALL.map(|section| {
                                Button::new(section.id())
                                    .ghost()
                                    .small()
                                    .w_full()
                                    .h(px(CONTROL_HEIGHT))
                                    .accessibility_label(section.label(locale))
                                    .child(
                                        div()
                                            .w_full()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .text_size(px(13.))
                                            .child(Icon::new(section.icon()).size(px(16.)))
                                            .child(section.label(locale)),
                                    )
                                    .debug_selector(move || {
                                        format!("settings-nav-{}", section.id())
                                    })
                                    .selected(section == self.settings_section)
                                    .on_click(cx.listener(move |app, _, window, cx| {
                                        app.select_settings_section(section, window, cx);
                                    }))
                            })),
                    )
                    .child(
                        div().flex_none().p_3().child(
                            Button::new("back-to-workspace")
                                .debug_selector(|| "back-to-workspace".into())
                                .ghost()
                                .small()
                                .w_full()
                                .h(px(CONTROL_HEIGHT))
                                .icon(IconName::ArrowLeft)
                                .accessibility_label(locale.text("返回工作区"))
                                .child(control_label(locale.text("返回工作区")))
                                .on_click(cx.listener(|app, _, window, cx| {
                                    app.toggle_settings(window, cx)
                                })),
                        ),
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
                            .debug_selector(|| "settings-breadcrumb".into())
                            .border_b(px(0.5))
                            .border_color(material.edge)
                            .h(px(HEADER_HEIGHT))
                            .flex_none()
                            .px_8()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(CONTENT_WIDTH))
                                    .mx_auto()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        div()
                                            .debug_selector(|| "settings-breadcrumb-label".into())
                                            .text_color(rgb(colors.muted))
                                            .child(locale.text("设置")),
                                    )
                                    .child(div().text_color(rgb(colors.muted)).child("/"))
                                    .child(section.label(locale)),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-scroll")
                            .size_full()
                            .overflow_y_scroll()
                            .lock_scroll_axis()
                            .track_scroll(&self.settings_scroll)
                            .child(
                                div().min_w_0().p_8().child(
                                    div()
                                        .debug_selector(move || {
                                            format!("settings-content-{}", section.id())
                                        })
                                        .w_full()
                                        .max_w(px(CONTENT_WIDTH))
                                        .mx_auto()
                                        .child(content),
                                ),
                            )
                            .map(|content| {
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_hidden()
                                    .child(content)
                                    // Keep the scrollbar fixed outside the scrolled content.
                                    .child(Scrollbar::vertical(&self.settings_scroll))
                            }),
                    ),
            )
    }
}

pub(super) fn settings_group(
    colors: Palette,
    title: impl Into<SharedString>,
    rows: impl IntoIterator<Item = gpui::Div>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(16.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(colors.text))
                .child(title.into()),
        )
        .child(
            div()
                .bg(rgb(colors.elevated))
                .border_1()
                .border_color(rgb(colors.border))
                .rounded(px(CARD_RADIUS))
                .px_5()
                .children(rows.into_iter().enumerate().map(|(index, row)| {
                    row.when(index > 0, |row| {
                        row.border_t(px(0.5)).border_color(rgb(colors.border))
                    })
                })),
        )
}

pub(super) fn settings_row(
    colors: Palette,
    label: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: impl IntoElement,
) -> gpui::Div {
    div()
        .min_h(px(64.))
        .py_4()
        .flex()
        .items_center()
        .gap_5()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(label.into()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(colors.muted))
                        .line_height(relative(1.6))
                        .child(description.into()),
                ),
        )
        .child(
            div()
                .w(px(320.))
                .flex_none()
                .flex()
                .justify_start()
                .text_color(rgb(colors.text_secondary))
                .whitespace_normal()
                .child(control),
        )
}

pub(super) fn control_label(label: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .text_left()
        .truncate()
        .child(label.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_sidebar_label_follows_ui_language() {
        assert_eq!(SettingsSection::Voice.label(Language::Chinese), "语音输入");
        assert_eq!(
            SettingsSection::Voice.label(Language::English),
            "Voice input"
        );
    }
}
