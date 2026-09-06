use super::*;
use crate::model::tools::{ToolActivity, ToolCategory, ToolDetail};
use gpui::{Axis, HighlightStyle};
use gpui_kit::base::{ScrollableMask, TextView as CodeView, TextViewStyle as CodeStyle};
use gpui_kit::component::scroll::ScrollableElement as _;
use std::{ops::Range, rc::Rc, sync::LazyLock};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, ThemeSet},
    parsing::SyntaxSet,
};

const BATCH_HEIGHT: f32 = 240.;
const DETAIL_HEIGHT: f32 = 200.;

impl NexusView {
    pub(super) fn render_tool_batch(
        &self,
        batch: &[ToolActivity<'_>],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let first = batch[0].call.id;
        let id: ElementId = (ElementId::from(first), "tool-batch").into();
        let expanded = self.expanded_messages.contains(&id);
        let active_run = self.presenter.model().active_run;
        let mut categories = Vec::new();
        for tool in batch {
            let label = tool.category().label(locale);
            if !categories.contains(&label) {
                categories.push(label);
            }
        }
        let failures = batch.iter().filter(|tool| tool.is_error()).count();
        let running = batch
            .iter()
            .filter(|tool| tool.is_running(active_run))
            .count();
        let separator = match locale {
            Language::Chinese => "、",
            Language::English => ", ",
        };
        let mut summary = format!("{} · {}", categories.join(separator), batch.len());
        if running > 0 {
            summary.push_str(&locale.format(
                " · {running} 项进行中",
                &[("running", (running).to_string())],
            ));
        }
        if failures > 0 {
            summary.push_str(&locale.format(
                " · {failures} 项失败",
                &[("failures", (failures).to_string())],
            ));
        }
        let toggle_id = id.clone();
        let height = if batch
            .iter()
            .any(|tool| self.expanded_messages.contains(&tool.call.id.into()))
        {
            BATCH_HEIGHT
        } else {
            (batch.len() as f32 * 34. - 4.).min(BATCH_HEIGHT)
        };
        let scroll = window
            .use_keyed_state((id.clone(), "scroll"), cx, |_, _| ScrollHandle::new())
            .read(cx)
            .clone();
        div()
            .w_full()
            .min_w_0()
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                Button::new(id.clone())
                    .ghost()
                    .small()
                    .h(px(32.))
                    .w_full()
                    .justify_start()
                    .px_2()
                    .debug_selector(move || format!("tool-batch-{first}"))
                    .accessibility_label(if expanded {
                        locale.text("收起工具调用批次")
                    } else {
                        locale.text("展开工具调用批次")
                    })
                    .icon(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(rgb(colors.text_secondary))
                            .child(summary),
                    )
                    .on_click(cx.listener(move |app, _, _, cx| {
                        if !app.expanded_messages.remove(&toggle_id) {
                            app.expanded_messages.insert(toggle_id.clone());
                        }
                        cx.notify();
                    })),
            )
            .when(expanded, |element| {
                element.child(
                    div()
                        .relative()
                        .w_full()
                        .min_w_0()
                        .child(
                            div()
                                .id((id.clone(), "items"))
                                .debug_selector(move || format!("tool-batch-viewport-{first}"))
                                .w_full()
                                .h(px(height))
                                .overflow_y_scroll()
                                .lock_scroll_axis()
                                .track_scroll(&scroll)
                                .flex()
                                .flex_col()
                                .gap_1()
                                .children(
                                    batch
                                        .iter()
                                        .map(|tool| self.render_tool_row(tool, window, cx)),
                                ),
                        )
                        .child(ScrollableMask::new(Axis::Vertical, &scroll).id(id.clone()))
                        .vertical_scrollbar(&scroll),
                )
            })
            .into_any_element()
    }

    fn render_tool_row(
        &self,
        tool: &ToolActivity<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let id = tool.call.id;
        let expanded = self.expanded_messages.contains(&id.into());
        let error = tool.is_error();
        let running = tool.is_running(self.presenter.model().active_run);
        let status = if error {
            locale.text("失败")
        } else if running {
            locale.text("运行中")
        } else if tool.result.is_some() {
            locale.text("完成")
        } else if tool.call.tool.is_some() {
            locale.text("未完成")
        } else {
            locale.text("已记录")
        };
        let icon = match tool.category() {
            ToolCategory::Command => IconName::SquareTerminal,
            ToolCategory::Read => IconName::BookOpen,
            ToolCategory::Search => IconName::Search,
            ToolCategory::Edit | ToolCategory::Create => IconName::FileText,
            ToolCategory::Other => IconName::Asterisk,
        };
        let preview = window.use_keyed_state((ElementId::from(id), "preview"), cx, |_, _| {
            (locale, tool.preview(locale))
        });
        if preview.read(cx).0 != locale {
            preview.update(cx, |state, _| *state = (locale, tool.preview(locale)));
        }
        let preview = preview.read(cx).1.clone();
        div()
            .w_full()
            .min_w_0()
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                Button::new(id)
                    .ghost()
                    .small()
                    .h(px(30.))
                    .w_full()
                    .justify_start()
                    .px_2()
                    .debug_selector(move || format!("tool-row-{id}"))
                    .accessibility_label(
                        locale.format(
                            "{preview}，{status}，{0}详情",
                            &[
                                ("preview", (preview).to_string()),
                                ("status", (status).to_string()),
                                (
                                    "0",
                                    (if expanded {
                                        locale.text("收起")
                                    } else {
                                        locale.text("展开")
                                    })
                                    .to_string(),
                                ),
                            ],
                        ),
                    )
                    .icon(icon)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(rgb(colors.text_secondary))
                            .child(preview),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(rgb(if error { colors.danger } else { colors.muted }))
                            .child(status),
                    )
                    .child(
                        Icon::new(if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size(px(12.)),
                    )
                    .on_click(cx.listener(move |app, _, _, cx| {
                        if !app.expanded_messages.remove(&id.into()) {
                            app.expanded_messages.insert(id.into());
                        }
                        cx.notify();
                    })),
            )
            .when(expanded, |element| {
                let result_id = tool.result.map(|result| result.id);
                let details =
                    window.use_keyed_state((ElementId::from(id), "details"), cx, |_, _| {
                        (result_id, locale, Rc::new(tool.details(locale)))
                    });
                if details.read(cx).0 != result_id || details.read(cx).1 != locale {
                    details.update(cx, |state, _| {
                        *state = (result_id, locale, Rc::new(tool.details(locale)))
                    });
                }
                let details = details.read(cx).2.clone();
                element.child(
                    div()
                        .pl_6()
                        .pr_2()
                        .pb_2()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .children(details.iter().enumerate().map(|(index, detail)| {
                            render_detail(id, index, detail, locale, window, cx)
                        }))
                        .when(tool.result.is_none(), |element| {
                            element.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors.muted))
                                    .child(if running {
                                        locale.text("等待工具返回结果…")
                                    } else {
                                        locale.text("未收到此工具的执行结果。")
                                    }),
                            )
                        }),
                )
            })
            .into_any_element()
    }
}

fn render_detail(
    tool_id: Uuid,
    index: usize,
    detail: &ToolDetail,
    locale: Language,
    window: &mut Window,
    cx: &mut gpui::App,
) -> AnyElement {
    let colors = palette(cx);
    let dark = cx.global::<ResolvedAppearance>().dark;
    let id: ElementId = (
        ElementId::from(tool_id),
        SharedString::from(format!("detail-{index}")),
    )
        .into();
    let scroll = window
        .use_keyed_state((id.clone(), "scroll"), cx, |_, _| ScrollHandle::new())
        .read(cx)
        .clone();
    let copy = detail.text.clone();
    let language = detail.language.clone();
    let is_diff = detail.diff;
    let (added, removed) = diff_lines(&detail.text).fold((0, 0), |(added, removed), (_, kind)| {
        (
            added + usize::from(kind == Some('+')),
            removed + usize::from(kind == Some('-')),
        )
    });
    div()
        .w_full()
        .min_w_0()
        .flex_none()
        .rounded(px(CONTROL_RADIUS))
        .overflow_hidden()
        .bg(rgb(colors.surface))
        .border_1()
        .border_color(rgb(colors.border))
        .child(
            div()
                .h(px(30.))
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .text_color(rgb(colors.muted))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(detail.title.clone()),
                )
                .when(is_diff, |element| {
                    element
                        .child(
                            div()
                                .text_color(rgb(colors.success))
                                .child(format!("+{added}")),
                        )
                        .child(
                            div()
                                .text_color(rgb(colors.danger))
                                .child(format!("−{removed}")),
                        )
                })
                .child(
                    Button::new((id.clone(), "copy"))
                        .ghost()
                        .small()
                        .h(px(24.))
                        .label(locale.text("复制"))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                        }),
                ),
        )
        .child(
            div()
                .id((id.clone(), "viewport"))
                .debug_selector(move || format!("tool-detail-{tool_id}-{index}"))
                .h(px(
                    (detail.text.lines().count() as f32 * 21. + 20.).clamp(52., DETAIL_HEIGHT)
                ))
                .w_full()
                .min_w_0()
                .relative()
                .overflow_hidden()
                .child(
                    div()
                        .id((id.clone(), "code-scroll"))
                        .size_full()
                        .overflow_y_scroll()
                        .lock_scroll_axis()
                        .track_scroll(&scroll)
                        .child(
                            div()
                                .debug_selector(move || {
                                    format!("tool-detail-content-{tool_id}-{index}")
                                })
                                .w_full()
                                .min_w_0()
                                .child(
                                    CodeView::markdown(
                                        id.clone(),
                                        fenced_code(&detail.text, &detail.language),
                                    )
                                    .selectable(true)
                                    .style(
                                        CodeStyle::default()
                                            .with_foreground(rgb(colors.text).into())
                                            .with_dark(dark)
                                            .with_code_block(
                                                gpui::StyleRefinement::default()
                                                    .font_family(MONO_FONT)
                                                    .text_size(px(12.))
                                                    .line_height(relative(1.65))
                                                    .p(px(10.))
                                                    .bg(rgb(colors.surface)),
                                            ),
                                    )
                                    .code_block_highlighter(move |block| {
                                        code_highlights(&block.code(), &language, is_diff, dark)
                                    }),
                                ),
                        ),
                )
                .child(ScrollableMask::new(Axis::Vertical, &scroll).id(id))
                .vertical_scrollbar(&scroll),
        )
        .into_any_element()
}

fn fenced_code(code: &str, language: &str) -> String {
    let fence = "`".repeat(
        code.split(|character| character != '`')
            .map(str::len)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(3),
    );
    // File extensions are data, so never let them inject Markdown syntax.
    let language = if language.chars().all(|c| c.is_ascii_alphanumeric()) {
        language
    } else {
        "text"
    };
    format!("{fence}{language}\n{code}\n{fence}")
}

fn diff_lines(code: &str) -> impl Iterator<Item = (&str, Option<char>)> {
    let mut in_hunk = false;
    code.split_inclusive('\n').map(move |line| {
        if line.starts_with("@@") || line.starts_with("diff ") {
            in_hunk = line.starts_with("@@");
        }
        let kind = match line.chars().next() {
            Some('+') if in_hunk || !line.starts_with("+++") => Some('+'),
            Some('-') if in_hunk || !line.starts_with("---") => Some('-'),
            Some(' ') => Some(' '),
            _ => None,
        };
        (line, kind)
    })
}

pub(super) fn code_highlights(
    code: &str,
    language: &str,
    diff: bool,
    dark: bool,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let colors = Palette::for_dark(dark);
    static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
    static THEMES: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);
    let syntax = SYNTAXES
        .find_syntax_by_token(language)
        .unwrap_or_else(|| SYNTAXES.find_syntax_plain_text());
    let theme = &THEMES.themes[if dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    }];
    let mut before = HighlightLines::new(syntax, theme);
    let mut after = HighlightLines::new(syntax, theme);
    let mut ranges = Vec::new();
    let mut offset = 0;
    for (line, kind) in diff_lines(code) {
        let (prefix, background) = if diff && kind == Some('+') {
            (1, Some(rgb(colors.diff_added).into()))
        } else if diff && kind == Some('-') {
            (1, Some(rgb(colors.diff_removed).into()))
        } else if diff && kind == Some(' ') {
            (1, None)
        } else if diff {
            ranges.push((
                offset..offset + line.len(),
                HighlightStyle {
                    color: Some(rgb(colors.muted).into()),
                    ..Default::default()
                },
            ));
            offset += line.len();
            continue;
        } else {
            (0, None)
        };
        if prefix > 0 {
            ranges.push((
                offset..offset + prefix,
                HighlightStyle {
                    background_color: background,
                    color: Some(
                        rgb(if line.starts_with('+') {
                            colors.success
                        } else if line.starts_with('-') {
                            colors.danger
                        } else {
                            colors.muted
                        })
                        .into(),
                    ),
                    ..Default::default()
                },
            ));
        }
        let text = &line[prefix..];
        let highlighter = if diff && line.starts_with('-') {
            &mut before
        } else {
            if diff && line.starts_with(' ') {
                let _ = before.highlight_line(text, &SYNTAXES);
            }
            &mut after
        };
        let start = offset;
        offset += prefix;
        if let Ok(tokens) = highlighter.highlight_line(text, &SYNTAXES) {
            for (style, token) in tokens {
                let color = style.foreground;
                let mut color = rgba(u32::from_be_bytes([color.r, color.g, color.b, color.a]));
                let backdrop: gpui::Rgba = background
                    .unwrap_or_else(|| rgb(colors.surface).into())
                    .into();
                // Some bundled syntax themes have low-contrast comments. Preserve
                // their hue while moving toward the readable foreground as needed.
                let target = rgb(colors.text);
                while contrast_ratio(color, backdrop) < 4.5 {
                    color.r += (target.r - color.r) * 0.2;
                    color.g += (target.g - color.g) * 0.2;
                    color.b += (target.b - color.b) * 0.2;
                }
                ranges.push((
                    offset..offset + token.len(),
                    HighlightStyle {
                        color: Some(color.into()),
                        background_color: background,
                        font_weight: style
                            .font_style
                            .contains(FontStyle::BOLD)
                            .then_some(gpui::FontWeight::BOLD),
                        font_style: style
                            .font_style
                            .contains(FontStyle::ITALIC)
                            .then_some(gpui::FontStyle::Italic),
                        ..Default::default()
                    },
                ));
                offset += token.len();
            }
        } else {
            ranges.push((
                offset..start + line.len(),
                HighlightStyle {
                    background_color: background,
                    ..Default::default()
                },
            ));
            offset = start + line.len();
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_and_diff_highlights_preserve_unicode_and_addition_deletion_colors() {
        let code = "fn main() { println!(\"你好\"); }\n";
        for dark in [true, false] {
            let colors = Palette::for_dark(dark);
            let styles = code_highlights(code, "rs", false, dark);
            assert!(
                styles
                    .windows(2)
                    .any(|pair| pair[0].1.color != pair[1].1.color)
            );
            let diff = format!("@@ -1 +1 @@\n-old\n+{code}");
            let styles = code_highlights(&diff, "rs", true, dark);
            let color_at = |offset| {
                styles
                    .iter()
                    .find(|(range, _)| range.contains(&offset))
                    .unwrap()
                    .1
            };
            assert_eq!(
                color_at(diff.find("-old").unwrap()).background_color,
                Some(rgb(colors.diff_removed).into())
            );
            assert_eq!(
                color_at(diff.find("+fn").unwrap()).background_color,
                Some(rgb(colors.diff_added).into())
            );
            assert_eq!(
                color_at(diff.find("你好").unwrap()).background_color,
                Some(rgb(colors.diff_added).into())
            );
            for (range, style) in styles {
                assert!(diff.is_char_boundary(range.start) && diff.is_char_boundary(range.end));
                assert!(
                    contrast_ratio(
                        style.color.unwrap().into(),
                        style
                            .background_color
                            .unwrap_or_else(|| rgb(colors.surface).into())
                            .into()
                    ) >= 4.5
                );
            }
            let comment = code_highlights("// Readable comment\n", "rs", false, dark);
            assert!(comment.iter().all(|(_, style)| contrast_ratio(
                style.color.unwrap().into(),
                rgb(colors.surface)
            ) >= 4.5));
        }
        assert_ne!(
            code_highlights(code, "rs", false, false),
            code_highlights(code, "rs", false, true)
        );
        assert_eq!(
            fenced_code("```\n# literal", "md"),
            "````md\n```\n# literal\n````"
        );
        assert!(fenced_code("literal", "bad\n# injected").starts_with("```text\n"));
        let diff = "--- a/file.md\n+++ b/file.md\n@@ -1 +1 @@\n----\n++++\n";
        assert_eq!(
            diff_lines(diff).map(|(_, kind)| kind).collect::<Vec<_>>(),
            vec![None, None, None, Some('-'), Some('+')]
        );
    }
}
