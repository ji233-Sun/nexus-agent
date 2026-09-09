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
const DIFF_LINE_HEIGHT: f32 = 20.;
const DIFF_FONT_SIZE: f32 = 13.;

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
        let height = if batch.iter().any(|tool| {
            tool.category() == ToolCategory::Edit
                && self.expanded_messages.contains(&tool.call.id.into())
        }) {
            // Show the complete diff card and a short result, not a clipped card
            // inside a second scroll viewport. Other tool batches stay compact.
            360.
        } else if batch
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
                            render_detail(
                                format!("tool-detail-{id}-{index}").into(),
                                detail,
                                locale,
                            )
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

pub(super) fn render_detail(
    key: SharedString,
    detail: &ToolDetail,
    locale: Language,
) -> impl IntoElement {
    DetailView {
        key,
        detail: detail.clone(),
        locale,
        layout: DetailLayout::Card,
    }
}

pub(super) fn render_review_detail(
    key: SharedString,
    detail: ToolDetail,
    original: String,
    locale: Language,
) -> impl IntoElement {
    DetailView {
        key,
        detail,
        locale,
        layout: DetailLayout::Review { original },
    }
}

enum DetailLayout {
    Card,
    Review { original: String },
}

#[derive(IntoElement)]
struct DetailView {
    key: SharedString,
    detail: ToolDetail,
    locale: Language,
    layout: DetailLayout,
}

impl gpui::RenderOnce for DetailView {
    fn render(self, window: &mut Window, cx: &mut gpui::App) -> impl IntoElement {
        render_detail_content(self.key, &self.detail, self.layout, self.locale, window, cx)
    }
}

fn render_detail_content(
    key: SharedString,
    detail: &ToolDetail,
    layout: DetailLayout,
    locale: Language,
    window: &mut Window,
    cx: &mut gpui::App,
) -> AnyElement {
    let colors = palette(cx);
    let dark = cx.global::<ResolvedAppearance>().dark;
    let id = ElementId::from(key.clone());
    let scroll = window
        .use_keyed_state((id.clone(), "scroll"), cx, |_, _| ScrollHandle::new())
        .read(cx)
        .clone();
    let full_height = matches!(layout, DetailLayout::Review { .. });
    let copy = match layout {
        DetailLayout::Card => detail.text.clone(),
        DetailLayout::Review { original } => original,
    };
    let language = detail.language.clone();
    let is_diff = detail.diff;
    let lines = if is_diff {
        diff_lines(&detail.text).collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let added = lines.iter().filter(|line| line.kind == Some('+')).count();
    let removed = lines.iter().filter(|line| line.kind == Some('-')).count();
    let number_width = lines
        .iter()
        .flat_map(|line| [line.old, line.new])
        .flatten()
        .max()
        .unwrap_or(1)
        .to_string()
        .len()
        .max(2) as f32
        * 8.
        + 12.;
    let gutter_width = number_width * 2. + 8.;
    let viewport_key = key.clone();
    // Keep one selectable code document; the gutter and full-row washes sit
    // behind it. They share the same line metrics and scroll coordinate space.
    let code = CodeView::markdown(id.clone(), fenced_code(&detail.text, &detail.language))
        .selectable(true)
        .style(
            CodeStyle::default()
                .with_foreground(rgb(colors.text).into())
                .with_dark(dark)
                .with_code_block({
                    let style = gpui::StyleRefinement::default()
                        .font_family(MONO_FONT)
                        .text_size(px(if is_diff { DIFF_FONT_SIZE } else { 12. }))
                        .line_height(if is_diff {
                            px(DIFF_LINE_HEIGHT).into()
                        } else {
                            relative(1.65)
                        })
                        .p(px(if is_diff { 0. } else { 10. }))
                        .bg(if is_diff {
                            rgba(0)
                        } else {
                            rgb(colors.surface)
                        });
                    if is_diff {
                        style.whitespace_nowrap()
                    } else {
                        style
                    }
                }),
        )
        .code_block_highlighter(move |block| {
            let mut highlights = code_highlights(&block.code(), &language, is_diff, dark);
            if is_diff {
                for (_, style) in &mut highlights {
                    style.background_color = None;
                    style.font_weight = None;
                    style.font_style = None;
                }
            }
            highlights
        });
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .map(|element| {
            if full_height {
                element.flex_1().min_h_0().h_full()
            } else {
                element
                    .flex_none()
                    .rounded(px(CONTROL_RADIUS))
                    .border_1()
                    .border_color(rgb(colors.border))
            }
        })
        .overflow_hidden()
        .bg(rgb(colors.surface))
        .child(
            div()
                .h(px(if full_height { 40. } else { 30. }))
                .flex_none()
                .px_3()
                .border_b_1()
                .border_color(rgb(colors.border))
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .text_color(rgb(colors.muted))
                .when(is_diff, |element| {
                    element.child(Icon::new(IconName::FileText).size(px(13.)))
                })
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(rgb(colors.text_secondary))
                        .font_weight(gpui::FontWeight::MEDIUM)
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
                .child(div().flex_1())
                .child(
                    Button::new((id.clone(), "copy"))
                        .debug_selector({
                            let key = key.clone();
                            move || format!("{key}-copy")
                        })
                        .ghost()
                        .small()
                        .h(px(24.))
                        .icon(IconName::Copy)
                        .accessibility_label(locale.text(if full_height {
                            "复制差异"
                        } else {
                            "复制"
                        }))
                        .tooltip(locale.text(if full_height {
                            "复制差异"
                        } else {
                            "复制"
                        }))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                        }),
                ),
        )
        .child(
            div()
                .id((id.clone(), "viewport"))
                .debug_selector(move || viewport_key.to_string())
                .map(|element| {
                    if full_height {
                        element.flex_1().min_h_0()
                    } else {
                        element.h(px(if is_diff {
                            (lines.len() as f32 * DIFF_LINE_HEIGHT + 8.).clamp(48., DETAIL_HEIGHT)
                        } else {
                            (detail.text.lines().count() as f32 * 21. + 20.)
                                .clamp(52., DETAIL_HEIGHT)
                        }))
                    }
                })
                .w_full()
                .min_w_0()
                .relative()
                .overflow_hidden()
                .child(
                    div()
                        .id((id.clone(), "code-scroll"))
                        .size_full()
                        .map(|element| {
                            if is_diff {
                                element.overflow_scroll()
                            } else {
                                element.overflow_y_scroll()
                            }
                        })
                        .lock_scroll_axis()
                        .track_scroll(&scroll)
                        .child(
                            div()
                                .debug_selector(move || format!("{key}-content"))
                                .w_full()
                                .min_w_0()
                                .relative()
                                .when(is_diff, |element| {
                                    let style = gpui::TextStyle {
                                        font_family: MONO_FONT.into(),
                                        ..Default::default()
                                    };
                                    let width = lines.iter().fold(px(0.), |width, line| {
                                        let text = line.text.trim_end_matches(['\r', '\n']);
                                        width.max(
                                            window
                                                .text_system()
                                                .shape_line(
                                                    text.to_owned().into(),
                                                    px(DIFF_FONT_SIZE),
                                                    &[style.to_run(text.len())],
                                                    None,
                                                )
                                                .width,
                                        )
                                    });
                                    element
                                        .min_w_full()
                                        .w(width + px(gutter_width + 16.))
                                        .pl(px(gutter_width))
                                        .pr_2()
                                        .child(div().absolute().top_0().left_0().w_full().children(
                                            lines.iter().map(|line| {
                                                let (background, marker) = match line.kind {
                                                    Some('+') => {
                                                        (colors.diff_added, colors.success)
                                                    }
                                                    Some('-') => {
                                                        (colors.diff_removed, colors.danger)
                                                    }
                                                    Some(' ') => (colors.surface, colors.muted),
                                                    _ => (colors.recessed, colors.muted),
                                                };
                                                div()
                                                    .h(px(DIFF_LINE_HEIGHT))
                                                    .w_full()
                                                    .bg(rgb(background))
                                                    .border_l_2()
                                                    .border_color(rgb(
                                                        if matches!(line.kind, Some('+' | '-')) {
                                                            marker
                                                        } else {
                                                            background
                                                        },
                                                    ))
                                                    .flex()
                                                    .font_family(MONO_FONT)
                                                    .text_size(px(11.))
                                                    .line_height(px(DIFF_LINE_HEIGHT))
                                                    .text_color(rgb(marker))
                                                    .children([line.old, line.new].map(|number| {
                                                        div()
                                                            .w(px(number_width))
                                                            .flex_none()
                                                            .pr_2()
                                                            .text_right()
                                                            .child(
                                                                number
                                                                    .map(|n| n.to_string())
                                                                    .unwrap_or_default(),
                                                            )
                                                    }))
                                            }),
                                        ))
                                })
                                .child(code),
                        ),
                )
                .child(ScrollableMask::new(Axis::Vertical, &scroll).id(id))
                .when(is_diff, |element| element.horizontal_scrollbar(&scroll))
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
    let newline = if code.ends_with('\n') { "" } else { "\n" };
    format!("{fence}{language}\n{code}{newline}{fence}")
}

struct DiffLine<'a> {
    text: &'a str,
    kind: Option<char>,
    old: Option<usize>,
    new: Option<usize>,
}

fn diff_lines(code: &str) -> impl Iterator<Item = DiffLine<'_>> {
    let mut in_hunk = false;
    let (mut old, mut new) = (None, None);
    let mut remaining = (0, 0);
    code.split_inclusive('\n').map(move |line| {
        if line.starts_with("@@") || line.starts_with("diff ") {
            in_hunk = line.starts_with("@@");
            (old, new) = (None, None);
            remaining = (0, 0);
            if let Some((before, after)) = line
                .strip_prefix("@@ -")
                .and_then(|line| line.split_once(" +"))
                && let Some((after, _)) = after.split_once(" @@")
            {
                let range = |text: &str| {
                    let (start, count) = text.split_once(',').unwrap_or((text, "1"));
                    Some((start.parse::<usize>().ok()?, count.parse::<usize>().ok()?))
                };
                if let (Some(before), Some(after)) = (range(before), range(after)) {
                    (old, new) = (Some(before.0), Some(after.0));
                    remaining = (before.1, after.1);
                }
            }
        }
        let kind = match line.chars().next() {
            Some('+') if in_hunk || !line.starts_with("+++") => Some('+'),
            Some('-') if in_hunk || !line.starts_with("---") => Some('-'),
            Some(' ') => Some(' '),
            _ => None,
        };
        let result = DiffLine {
            text: line,
            kind,
            old: old.filter(|_| matches!(kind, Some('-' | ' '))),
            new: new.filter(|_| matches!(kind, Some('+' | ' '))),
        };
        if result.old.is_some() {
            old = old.and_then(|line| line.checked_add(1));
            remaining.0 = remaining.0.saturating_sub(1);
        }
        if result.new.is_some() {
            new = new.and_then(|line| line.checked_add(1));
            remaining.1 = remaining.1.saturating_sub(1);
        }
        if old.is_some() && new.is_some() && remaining == (0, 0) {
            in_hunk = false;
            (old, new) = (None, None);
        }
        result
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
    let mut syntax = SYNTAXES
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
    for DiffLine {
        text: line, kind, ..
    } in diff_lines(code)
    {
        if diff && kind.is_none() {
            if let Some(path) = line
                .strip_prefix("+++ ")
                .or_else(|| line.strip_prefix("--- "))
            {
                // A workspace review can contain several files/languages.
                if path.trim() != "/dev/null" {
                    syntax = Path::new(path.trim().trim_matches('"'))
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .and_then(|extension| SYNTAXES.find_syntax_by_extension(extension))
                        .unwrap_or_else(|| SYNTAXES.find_syntax_plain_text());
                }
            }
            if line.starts_with("@@") || line.starts_with("diff ") || line.starts_with("+++ ") {
                before = HighlightLines::new(syntax, theme);
                after = HighlightLines::new(syntax, theme);
            }
        }
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
            let header = "diff --git a/main.rs b/main.rs\n--- a/main.rs\n+++ b/main.rs\n";
            let review = code_highlights(&format!("{header}{diff}"), "diff", true, dark);
            assert_eq!(
                review
                    .into_iter()
                    .filter(|(range, _)| range.start >= header.len())
                    .map(|(range, style)| (
                        range.start - header.len()..range.end - header.len(),
                        style
                    ))
                    .collect::<Vec<_>>(),
                code_highlights(&diff, "rs", true, dark),
                "workspace patches should use each file's syntax, not diff syntax"
            );
        }
        assert_ne!(
            code_highlights(code, "rs", false, false),
            code_highlights(code, "rs", false, true)
        );
        assert_eq!(
            fenced_code("```\n# literal", "md"),
            "````md\n```\n# literal\n````"
        );
        assert_eq!(fenced_code("line\n", "rs"), "```rs\nline\n```");
        assert!(fenced_code("literal", "bad\n# injected").starts_with("```text\n"));
        let diff = "--- a/file.md\n+++ b/file.md\n@@ -1 +1 @@\n----\n++++\n";
        assert_eq!(
            diff_lines(diff).map(|line| line.kind).collect::<Vec<_>>(),
            vec![None, None, None, Some('-'), Some('+')]
        );
    }

    #[test]
    fn diff_gutters_follow_hunks_without_inventing_snippet_line_numbers() {
        use super::super::review::{ReviewSection, patch_files};
        let patch = "--- a/main.rs\n+++ b/main.rs\n@@ -9,3 +12,3 @@ fn main() {\n context\n-old\n+你好\n tail\n@@ -20,0 +24,2 @@\n+\n++++\n\\ No newline at end of file\n--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n----\n";
        let rows = diff_lines(patch).collect::<Vec<_>>();
        assert_eq!(rows.iter().map(|row| row.text).collect::<String>(), patch);
        assert_eq!(
            rows.iter()
                .filter(|row| row.kind.is_some())
                .map(|row| (row.kind, row.old, row.new))
                .collect::<Vec<_>>(),
            vec![
                (Some(' '), Some(9), Some(12)),
                (Some('-'), Some(10), None),
                (Some('+'), None, Some(13)),
                (Some(' '), Some(11), Some(14)),
                (Some('+'), None, Some(24)),
                (Some('+'), None, Some(25)),
                (Some('-'), Some(1), None),
            ]
        );
        for patch in [
            "-old\n+new",
            "@@ invalid @@\n-old\n+new",
            "@@@ combined diff @@@\n-old\n+new",
        ] {
            assert!(diff_lines(patch).all(|line| line.old.is_none() && line.new.is_none()));
        }
        let full =
            format!("diff --git a/main.rs b/main.rs\nindex 0000000..1111111 100644\n{patch}");
        let files = patch_files(&full, ReviewSection::Unstaged);
        assert_eq!(files.len(), 1);
        let detail = files[0].detail(Language::English);
        assert!(detail.text.starts_with("@@ -9,3 +12,3 @@"));
        assert_eq!(files[0].patch, full, "copy preserves the complete patch");
        assert_eq!(detail.language, "rs");
        assert_eq!(
            diff_lines(&detail.text).filter_map(|line| line.new).next(),
            Some(12)
        );
    }

    #[test]
    fn review_navigation_decodes_real_git_paths_and_preserves_binary_changes() {
        use super::super::review::{ReviewSection, patch_files};
        use crate::{infrastructure::git, model::workspace::Workspace};
        let (_directory, project) = git::tests::repository_fixture();
        let cwd = Path::new(&project.canonical_path);
        let mut names = vec!["a b/空 格.rs", "spaces b/path.rs", "plain.rs"];
        if cfg!(unix) {
            names.extend(["quote\"name.rs", "tab\tname.rs", "line\nname.rs"]);
        }
        for name in &names {
            let path = cwd.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "fn before() {}\n").unwrap();
        }
        std::fs::write(cwd.join("image.bin"), [0, 1, 2]).unwrap();
        git::git(cwd, &["add", "."]).unwrap();
        git::git(cwd, &["commit", "-m", "review fixtures"]).unwrap();
        for name in &names {
            std::fs::write(cwd.join(name), "fn after() {}\n").unwrap();
        }
        std::fs::write(cwd.join("image.bin"), [0, 2, 3]).unwrap();
        // Presentation must not depend on a user's Git header or color preferences.
        git::git(cwd, &["config", "diff.mnemonicPrefix", "true"]).unwrap();
        git::git(cwd, &["config", "color.ui", "always"]).unwrap();
        for quote in ["true", "false"] {
            git::git(cwd, &["config", "core.quotePath", quote]).unwrap();
            let review = git::changes::review(&Workspace::local(&project)).unwrap();
            let files = patch_files(&review.unstaged, ReviewSection::Unstaged);
            assert_eq!(files.len(), names.len() + 1);
            for name in &names {
                let file = files.iter().find(|file| file.path == *name).unwrap();
                assert_eq!((file.additions, file.deletions), (1, 1));
                assert!(
                    file.detail(Language::English)
                        .text
                        .contains("+fn after() {}")
                );
            }
            let binary = files.iter().find(|file| file.path == "image.bin").unwrap();
            assert_eq!((binary.additions, binary.deletions), (0, 0));
            assert!(!binary.detail(Language::English).diff);
            assert!(binary.patch.contains("GIT binary patch"));
        }
    }

    struct DiffHarness {
        detail: ToolDetail,
    }

    impl Render for DiffHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(480.))
                .child(gpui_kit::base::TextSelectionLayer)
                .child(render_detail(
                    "test-diff".into(),
                    &self.detail,
                    Language::English,
                ))
        }
    }

    #[gpui::test]
    fn diff_keeps_compact_lines_scrolls_both_axes_and_copies_the_original(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{ScrollDelta, ScrollWheelEvent, point};

        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let text = format!(
            "@@ -1,30 +1,30 @@\n{}{}",
            "-old\n".repeat(30),
            format!("+{}\n", "你好 very long line ".repeat(12)).repeat(30)
        );
        let (_, cx) = cx.add_window_view(|_, _| DiffHarness {
            detail: ToolDetail {
                title: "main.rs".into(),
                text: text.clone(),
                language: "rs".into(),
                diff: true,
            },
        });
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            });
        };
        draw(cx);
        let viewport = cx.debug_bounds("test-diff").unwrap();
        let content = cx.debug_bounds("test-diff-content").unwrap();
        assert_eq!(viewport.size.height, px(DETAIL_HEIGHT));
        assert!(content.size.width > viewport.size.width);
        // No wrapping or extra paragraph spacing, including Unicode and blank lines.
        assert_eq!(
            content.size.height,
            px(text.lines().count() as f32 * DIFF_LINE_HEIGHT)
        );
        let start = viewport.origin + point(px(65.), px(DIFF_LINE_HEIGHT + 5.));
        let end = viewport.origin + point(px(115.), px(DIFF_LINE_HEIGHT * 2. + 5.));
        cx.simulate_mouse_down(start, gpui::MouseButton::Left, Default::default());
        cx.simulate_mouse_move(end, gpui::MouseButton::Left, Default::default());
        cx.simulate_mouse_up(end, gpui::MouseButton::Left, Default::default());
        assert_eq!(
            cx.update(gpui_kit::base::TextSelection::selected_text),
            "-old\n-old\n\n", // TextView adds a code-block separator after the selected lines.
            "cross-line selection must exclude gutter numbers"
        );
        for delta in [point(px(-80.), px(0.)), point(px(0.), px(-60.))] {
            let before = cx.debug_bounds("test-diff-content").unwrap();
            cx.simulate_event(ScrollWheelEvent {
                position: viewport.center(),
                delta: ScrollDelta::Pixels(delta),
                ..Default::default()
            });
            draw(cx);
            let after = cx.debug_bounds("test-diff-content").unwrap();
            assert_eq!(after.origin, before.origin + delta);
        }
        let scrolled = cx.debug_bounds("test-diff-content").unwrap();
        for dark in [false, true] {
            cx.update(|_, cx| {
                theme::apply_theme(
                    ResolvedAppearance {
                        dark,
                        ..*cx.global::<ResolvedAppearance>()
                    },
                    cx,
                );
            });
            draw(cx);
            assert_eq!(cx.debug_bounds("test-diff-content").unwrap(), scrolled);
        }
        let copy = cx.debug_bounds("test-diff-copy").unwrap().center();
        cx.simulate_click(copy, Default::default());
        cx.update(|_, cx| assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), text));
    }
}
