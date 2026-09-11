use super::*;
use crate::{
    infrastructure::cnb::DOCUMENTATION,
    model::cnb::{Issue, IssueAction, IssueFilter, Label, PAGE_SIZE},
};
use gpui_kit::component::{scroll::ScrollableElement as _, spinner::Spinner};

pub(super) fn cnb_icon(size: f32, color: u32) -> impl IntoElement {
    // GPUI only paints an SVG when the element has an explicit text color.
    gpui::svg()
        .data(include_bytes!("../../assets/icons/cnb.svg").as_slice())
        .size(px(size))
        .flex_none()
        .text_color(rgb(color))
}

fn issue_color(issue: &Issue, colors: Palette) -> u32 {
    if issue.state == "closed" {
        colors.accent
    } else {
        colors.success
    }
}

fn issue_date(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|date| {
            date.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| "—".into())
}

fn issue_label(label: &Label, colors: Palette) -> impl IntoElement {
    let color = label
        .color
        .strip_prefix('#')
        .filter(|hex| hex.len() == 6)
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
        .unwrap_or(colors.accent);
    div()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .rounded(px(6.))
        .bg(rgb(colors.recessed))
        .text_size(px(11.))
        .text_color(rgb(colors.text_secondary))
        .child(div().size(px(6.)).rounded_full().bg(rgb(color)))
        .child(div().max_w(px(160.)).truncate().child(label.name.clone()))
}

impl NexusView {
    fn send_cnb_issue_to_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.presenter.prepare_cnb_chat() else {
            return;
        };
        self.prompt_input.update(cx, |input, cx| {
            let draft = input.value();
            let text = if draft.is_empty() {
                prompt
            } else {
                format!("{draft}\n\n{prompt}")
            };
            input.replace_all(text, window, cx);
        });
        self.settings_open = false;
        self.expanded_messages.clear();
        self.timeline_scroll.scroll_to_bottom();
        self.model_picker_open = true;
        self.focus_prompt(window, cx);
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    fn render_cnb_actions(&self, issue: &Issue, cx: &mut Context<Self>) -> impl IntoElement {
        let cnb = &self.presenter.model().cnb;
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let busy = cnb.action_request.is_some();
        let state = if issue.state == "closed" {
            IssueFilter::Open
        } else {
            IssueFilter::Closed
        };
        let action_button = |id| {
            Button::new(id)
                .h(px(CONTROL_HEIGHT))
                .px_3()
                .rounded(px(CONTROL_RADIUS))
        };
        div()
            .debug_selector(|| "cnb-issue-actions".into())
            .px_5()
            .py_4()
            .border_t_1()
            .border_color(rgb(colors.border))
            .flex()
            .flex_col()
            .gap_3()
            .text_size(px(12.))
            .text_color(rgb(colors.text_secondary))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        action_button("cnb-chat")
                            .debug_selector(|| "cnb-chat".into())
                            .primary()
                            .icon(IconName::Bot)
                            .label(locale.text("用 AI 处理"))
                            .tooltip(
                                locale
                                    .text("将完整 Issue 和评论加入聊天草稿，选择 Harness 后发送。"),
                            )
                            .disabled(
                                busy || cnb.comments.is_none() || cnb.comments_request.is_some(),
                            )
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.send_cnb_issue_to_chat(window, cx)
                            })),
                    )
                    .child(
                        action_button("cnb-assign-self")
                            .debug_selector(|| "cnb-assign-self".into())
                            .ghost()
                            .icon(IconName::User)
                            .label(locale.text("指派给我"))
                            .disabled(busy)
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.presenter.act_on_cnb_issue(IssueAction::AssignSelf);
                                cx.notify();
                            })),
                    )
                    .child(
                        action_button("cnb-change-state")
                            .debug_selector(|| "cnb-change-state".into())
                            .ghost()
                            .icon(if state == IssueFilter::Closed {
                                IconName::CircleCheck
                            } else {
                                IconName::RotateCw
                            })
                            .label(locale.text(if state == IssueFilter::Closed {
                                "关闭 Issue"
                            } else {
                                "重新打开 Issue"
                            }))
                            .disabled(busy)
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.presenter.act_on_cnb_issue(IssueAction::SetState(state));
                                cx.notify();
                            })),
                    )
                    .child(
                        action_button("cnb-npc")
                            .debug_selector(|| "cnb-npc".into())
                            .ghost()
                            .icon(IconName::Bot)
                            .label("CodeBuddy NPC")
                            .tooltip(
                                locale.text(
                                    "向此 Issue 发布评论，委托 CodeBuddy 完成开发并创建 PR。",
                                ),
                            )
                            .disabled(busy || cnb.npc_comment.is_some())
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.presenter.act_on_cnb_issue(IssueAction::StartNpc);
                                cx.notify();
                            })),
                    ),
            )
            .when(busy, |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(Spinner::new())
                        .child(locale.text("正在执行 Issue 操作…")),
                )
            })
            .when_some(cnb.action_error.as_ref(), |el, error| {
                el.child(
                    div()
                        .text_color(rgb(colors.danger))
                        .child(error.render(locale).to_owned()),
                )
            })
            .when_some(cnb.action_success.as_ref(), |el, message| {
                el.child(
                    div()
                        .text_color(rgb(colors.success))
                        .child(message.render(locale).to_owned()),
                )
            })
            .when_some(cnb.npc_comment.as_ref(), |el, comment| {
                if let Some(url) = comment.action_url() {
                    let url = url.to_owned();
                    el.child(
                        Button::new("cnb-npc-action")
                            .debug_selector(|| "cnb-npc-action".into())
                            .outline()
                            .small()
                            .icon(IconName::ExternalLink)
                            .label(locale.text("查看 Action"))
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                } else {
                    el.child(
                        Button::new("cnb-npc-refresh")
                            .debug_selector(|| "cnb-npc-refresh".into())
                            .outline()
                            .small()
                            .label(locale.text(if cnb.npc_request.is_some() {
                                "正在获取 Action 链接…"
                            } else {
                                "刷新 NPC 状态"
                            }))
                            .disabled(cnb.npc_request.is_some())
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.presenter.refresh_cnb_npc_action();
                                cx.notify();
                            })),
                    )
                }
            })
            .when_some(cnb.npc_error.as_ref(), |el, error| {
                el.child(
                    div()
                        .text_color(rgb(colors.danger))
                        .child(error.render(locale).to_owned()),
                )
            })
    }

    fn render_cnb_markdown(
        &self,
        id: String,
        text: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let cnb = &self.presenter.model().cnb;
        let colors = palette(cx);
        TextView::markdown(SharedString::from(id), text)
            .markdown_extensions(super::cnb_media::extensions(
                cnb.repository.as_deref().unwrap_or_default(),
                cnb.cli.as_ref().map(|cli| cli.path.as_path()),
                self.presenter.model().language,
            ))
            .text_size(px(14.))
            .line_height(relative(1.7))
            .style(
                TextViewStyle::default()
                    .paragraph_gap(gpui::rems(1.))
                    .code_block(
                        gpui::StyleRefinement::default()
                            .font_family(mono_font(cx))
                            .text_size(px(12.))
                            .p_4()
                            .bg(rgb(colors.elevated))
                            .rounded(px(CONTROL_RADIUS)),
                    ),
            )
    }

    fn render_cnb_comments(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let cnb = &self.presenter.model().cnb;
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        div()
            .debug_selector(|| "cnb-comments".into())
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(locale.text("评论"))
                    .child(
                        Button::new("cnb-comments-refresh")
                            .debug_selector(|| "cnb-comments-refresh".into())
                            .ghost()
                            .small()
                            .icon(IconName::RotateCw)
                            .label(locale.text("刷新评论"))
                            .disabled(cnb.comments_request.is_some())
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.presenter.load_cnb_comments();
                                cx.notify();
                            })),
                    ),
            )
            .when(cnb.comments_request.is_some(), |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(Spinner::new())
                        .child(locale.text("正在读取全部评论…")),
                )
            })
            .when_some(cnb.comments_error.as_ref(), |el, error| {
                el.child(
                    div()
                        .text_color(rgb(colors.danger))
                        .child(error.render(locale).to_owned()),
                )
            })
            .when_some(cnb.comments.as_ref(), |el, comments| {
                el.when(comments.is_empty(), |el| {
                    el.child(locale.text("此 Issue 暂无评论。"))
                })
                .children(comments.iter().map(|comment| {
                    let selector = format!("cnb-comment-{}", comment.id);
                    div()
                        .debug_selector(move || selector.clone())
                        .min_w_0()
                        .p_4()
                        .rounded(px(CARD_RADIUS))
                        .bg(rgb(colors.surface))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_3()
                                .text_size(px(12.))
                                .child(comment.author.name().to_owned())
                                .child(
                                    div()
                                        .text_color(rgb(colors.muted))
                                        .child(issue_date(&comment.created_at)),
                                ),
                        )
                        .child(self.render_cnb_markdown(
                            format!("cnb-comment-body-{}", comment.id),
                            comment.body.clone(),
                            cx,
                        ))
                        .when_some(comment.action_url(), |el, url| {
                            let url = url.to_owned();
                            el.child(
                                Button::new(SharedString::from(format!(
                                    "cnb-comment-action-{}",
                                    comment.id
                                )))
                                .ghost()
                                .small()
                                .icon(IconName::ExternalLink)
                                .label(locale.text("查看 Action"))
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                            )
                        })
                }))
            })
    }

    pub(super) fn render_cnb_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let cnb = &model.cnb;
        let locale = model.language;
        let colors = palette(cx);
        let status = if cnb.detection_request.is_some() {
            locale.text("正在检测 CNB CLI…").to_owned()
        } else if let Some(error) = &cnb.detection_error {
            error.render(locale).to_owned()
        } else if let Some(cli) = &cnb.cli {
            format!("CNB CLI {} · {}", cli.version, cli.path.display())
        } else {
            locale
                .text("检测本机 CNB CLI 后即可读取项目 Issues。")
                .to_owned()
        };
        settings::settings_group(
            colors,
            "CNB",
            [
                div()
                    .py_5()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(cnb_icon(28., colors.text))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(locale.text("启用 CNB 集成"))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors.muted))
                                    .child(locale.text(
                                        "自动识别项目的 CNB remote，使用本机 CLI 浏览 Issues。",
                                    )),
                            ),
                    )
                    .child(
                        div().debug_selector(|| "cnb-enabled".into()).child(
                            Switch::new("cnb-enabled")
                                .accessibility_label(locale.text("启用 CNB 集成"))
                                .checked(cnb.enabled)
                                .on_click(cx.listener(|app, enabled, _, cx| {
                                    app.presenter.set_cnb_enabled(*enabled);
                                    cx.notify();
                                })),
                        ),
                    ),
                div()
                    .py_5()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors.muted))
                            .child(status),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("cnb-detect")
                                    .outline()
                                    .small()
                                    .icon(IconName::RotateCw)
                                    .label(locale.text("重新检测"))
                                    .disabled(cnb.detection_request.is_some())
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.presenter.inspect_cnb();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("cnb-documentation")
                                    .ghost()
                                    .small()
                                    .icon(IconName::ExternalLink)
                                    .label(locale.text("安装与登录说明"))
                                    .on_click(|_, _, cx| cx.open_url(DOCUMENTATION)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors.muted))
                            .line_height(relative(1.6))
                            .child(locale.text(
                                "先在终端安装 CNB CLI 并运行 cnb login。登录凭据由 CLI 管理。",
                            )),
                    ),
            ],
        )
    }

    pub(super) fn render_cnb(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let cnb = &model.cnb;
        let colors = palette(cx);
        let locale = model.language;
        let repository = cnb.repository.clone().unwrap_or_default();
        let repo_url = format!("https://cnb.cool/{repository}");
        div()
            .debug_selector(|| "cnb-page".into())
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
                    .h(px(HEADER_HEIGHT))
                    .flex_none()
                    .px_6()
                    .flex()
                    .items_center()
                    .gap_3()
                    .text_size(px(13.))
                    .child(cnb_icon(22., colors.text))
                    .child(
                        div()
                            .debug_selector(|| "cnb-breadcrumb".into())
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .debug_selector(|| "cnb-breadcrumb-root".into())
                                    .flex_none()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("CNB"),
                            )
                            .children(repository.split('/').enumerate().flat_map(
                                |(index, segment)| {
                                    [
                                        div()
                                            .debug_selector(move || {
                                                format!("cnb-breadcrumb-separator-{index}")
                                            })
                                            .flex_none()
                                            .text_color(rgb(colors.muted))
                                            .child("/"),
                                        div()
                                            .debug_selector(move || {
                                                format!("cnb-breadcrumb-segment-{index}")
                                            })
                                            .min_w_0()
                                            .truncate()
                                            .child(segment.to_owned()),
                                    ]
                                },
                            )),
                    )
                    .child(
                        Button::new("cnb-repository")
                            .debug_selector(|| "cnb-repository".into())
                            .ghost()
                            .small()
                            .flex_none()
                            .icon(IconName::ExternalLink)
                            .tooltip(locale.text("在浏览器中打开仓库"))
                            .accessibility_label(locale.text("在浏览器中打开仓库"))
                            .on_click(move |_, _, cx| cx.open_url(&repo_url)),
                    ),
            )
            .child(
                div()
                    .px_5()
                    .pt_2()
                    .flex_none()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.surface))
                    .child(
                        div()
                            .debug_selector(|| "cnb-issue-tab".into())
                            .w(px(120.))
                            .px_4()
                            .py_3()
                            .rounded_t(px(8.))
                            .bg(rgb(colors.canvas))
                            .border_b_2()
                            .border_color(rgb(colors.accent))
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(Icon::new(IconName::Inbox).size(px(15.)))
                            .child("Issue"),
                    ),
            )
            .child(if cnb.cli.is_none() {
                div()
                    .p_8()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(locale.text("安装并登录 CNB CLI 后即可查看 Issues。"))
                    .when_some(cnb.detection_error.as_ref(), |el, error| {
                        el.child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(colors.muted))
                                .child(error.render(locale).to_owned()),
                        )
                    })
                    .child(
                        Button::new("cnb-open-settings")
                            .outline()
                            .label(locale.text("打开 Source Control 设置"))
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.settings_open = true;
                                app.select_settings_section(
                                    SettingsSection::SourceControl,
                                    window,
                                    cx,
                                );
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            } else if cnb.detail_number.is_some() {
                self.render_cnb_detail(cx).into_any_element()
            } else {
                self.render_cnb_list(cx).into_any_element()
            })
    }

    fn render_cnb_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let cnb = &self.presenter.model().cnb;
        let colors = palette(cx);
        let locale = self.presenter.model().language;
        let loading = cnb.list_request.is_some();
        let page = cnb.page;
        let filter = cnb.filter;
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_6()
                    .py_4()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .children([IssueFilter::Open, IssueFilter::Closed].map(|choice| {
                        Button::new(choice.label())
                            .ghost()
                            .small()
                            .label(locale.text(choice.label()))
                            .debug_selector(move || format!("cnb-filter-{}", choice.state()))
                            .selected(filter == choice)
                            .disabled(loading)
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.presenter.load_cnb_issues(1, choice);
                                app.cnb_scroll.set_offset(gpui::point(px(0.), px(0.)));
                                cx.notify();
                            }))
                    }))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors.muted))
                            .child(
                                locale.format(
                                    "{count} 个 Issues",
                                    &[("count", cnb.total.to_string())],
                                ),
                            ),
                    )
                    .child(
                        Button::new("cnb-refresh")
                            .ghost()
                            .small()
                            .icon(IconName::RotateCw)
                            .tooltip(locale.text("刷新 Issues"))
                            .accessibility_label(locale.text("刷新 Issues"))
                            .disabled(loading)
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.presenter.load_cnb_issues(page, filter);
                                cx.notify();
                            })),
                    ),
            )
            .when_some(cnb.list_error.as_ref(), |el, error| {
                el.child(
                    div()
                        .mx_6()
                        .mb_3()
                        .p_3()
                        .rounded(px(CONTROL_RADIUS))
                        .bg(rgb(colors.surface))
                        .text_color(rgb(colors.danger))
                        .text_size(px(12.))
                        .child(error.render(locale).to_owned()),
                )
            })
            .child(
                div()
                    .id("cnb-list-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.cnb_scroll)
                    .px_6()
                    .pb_4()
                    .when(loading, |el| {
                        el.child(
                            div()
                                .py_8()
                                .flex()
                                .justify_center()
                                .items_center()
                                .gap_3()
                                .child(Spinner::new())
                                .child(locale.text("正在读取 Issues…")),
                        )
                    })
                    .when(
                        !loading && cnb.issues.is_empty() && cnb.list_error.is_none(),
                        |el| {
                            el.child(
                                div()
                                    .debug_selector(|| "cnb-empty".into())
                                    .py_16()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap_3()
                                    .text_color(rgb(colors.muted))
                                    .child(Icon::new(IconName::Inbox).size(px(32.)))
                                    .child(locale.text("当前筛选下没有 Issues")),
                            )
                        },
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .children(cnb.issues.iter().map(|issue| {
                                let number = issue.number.clone();
                                let selector = number.clone();
                                div()
                                    .id(SharedString::from(format!("cnb-issue-{number}")))
                                    .debug_selector(move || format!("cnb-issue-{selector}"))
                                    .p_4()
                                    .rounded(px(CARD_RADIUS))
                                    .bg(rgb(colors.elevated))
                                    .border_1()
                                    .border_color(rgb(colors.border))
                                    .cursor_pointer()
                                    .hover(|style| {
                                        style
                                            .bg(rgb(colors.hover))
                                            .border_color(rgb(colors.input_border))
                                    })
                                    .flex()
                                    .items_start()
                                    .gap_3()
                                    .child(
                                        Icon::new(if issue.state == "closed" {
                                            IconName::CircleCheck
                                        } else {
                                            IconName::Info
                                        })
                                        .size(px(18.))
                                        .mt(px(2.))
                                        .text_color(rgb(issue_color(issue, colors))),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_size(px(14.))
                                                    .font_weight(gpui::FontWeight::MEDIUM)
                                                    .child(issue.title.clone()),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_wrap()
                                                    .items_center()
                                                    .gap_2()
                                                    .text_size(px(11.))
                                                    .text_color(rgb(colors.muted))
                                                    .child(format!("#{}", issue.number))
                                                    .child("·")
                                                    .child(issue.author.name().to_owned())
                                                    .child("·")
                                                    .child(issue_date(&issue.updated_at))
                                                    .when(!issue.priority.is_empty(), |row| {
                                                        row.child(issue.priority.clone())
                                                    })
                                                    .children(
                                                        issue.labels.iter().map(|label| {
                                                            issue_label(label, colors)
                                                        }),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .text_size(px(12.))
                                            .text_color(rgb(colors.muted))
                                            .child(locale.text("评论"))
                                            .child(issue.comment_count.to_string()),
                                    )
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.presenter.select_cnb_issue(number.clone());
                                        app.cnb_detail_scroll
                                            .set_offset(gpui::point(px(0.), px(0.)));
                                        cx.notify();
                                    }))
                            })),
                    )
                    .vertical_scrollbar(&self.cnb_scroll),
            )
            .child(
                div()
                    .px_6()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors.muted))
                            .child(locale.format(
                                "第 {page} / {pages} 页",
                                &[
                                    ("page", page.to_string()),
                                    ("pages", cnb.total.div_ceil(PAGE_SIZE).max(page).to_string()),
                                ],
                            )),
                    )
                    .child(div().flex().gap_2().children(
                        [(false, "上一页"), (true, "下一页")].map(|(next, label)| {
                            Button::new(label)
                                .outline()
                                .small()
                                .label(locale.text(label))
                                .debug_selector(move || {
                                    if next {
                                        "cnb-next".into()
                                    } else {
                                        "cnb-previous".into()
                                    }
                                })
                                .disabled(
                                    loading
                                        || if next {
                                            page * PAGE_SIZE >= cnb.total
                                        } else {
                                            page <= 1
                                        },
                                )
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.presenter.load_cnb_issues(
                                        if next { page + 1 } else { page - 1 },
                                        filter,
                                    );
                                    app.cnb_scroll.set_offset(gpui::point(px(0.), px(0.)));
                                    cx.notify();
                                }))
                        }),
                    )),
            )
    }

    fn render_cnb_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let cnb = &self.presenter.model().cnb;
        let colors = palette(cx);
        let locale = self.presenter.model().language;
        let number = cnb.detail_number.clone().unwrap_or_default();
        let issue_url = format!(
            "https://cnb.cool/{}/-/issues/{number}",
            cnb.repository.as_deref().unwrap_or_default()
        );
        div()
            .debug_selector(|| "cnb-detail".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_6()
                    .py_3()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Button::new("cnb-back")
                            .debug_selector(|| "cnb-back".into())
                            .ghost()
                            .small()
                            .icon(IconName::ArrowLeft)
                            .label(locale.text("返回 Issues"))
                            .disabled(cnb.action_request.is_some())
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.presenter.close_cnb_issue();
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("cnb-open-issue")
                            .ghost()
                            .small()
                            .icon(IconName::ExternalLink)
                            .label(locale.text("在 CNB 中打开"))
                            .on_click(move |_, _, cx| cx.open_url(&issue_url)),
                    ),
            )
            .child(
                div()
                    .id("cnb-detail-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.cnb_detail_scroll)
                    .px_8()
                    .pb_8()
                    .child(
                        div()
                            .max_w(px(CONTENT_WIDTH))
                            .mx_auto()
                            .flex()
                            .flex_col()
                            .gap_5()
                            .when(cnb.detail_request.is_some(), |el| {
                                el.child(
                                    div()
                                        .py_8()
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .child(Spinner::new())
                                        .child(locale.text("正在读取 Issue 详情…")),
                                )
                            })
                            .when_some(cnb.detail_error.as_ref(), |el, error| {
                                el.child(
                                    div()
                                        .text_color(rgb(colors.danger))
                                        .child(error.render(locale).to_owned()),
                                )
                                .child(
                                    Button::new("cnb-detail-retry")
                                        .outline()
                                        .small()
                                        .label(locale.text("重试"))
                                        .on_click(cx.listener(move |app, _, _, cx| {
                                            app.presenter.select_cnb_issue(number.clone());
                                            cx.notify();
                                        })),
                                )
                            })
                            .when_some(cnb.detail.as_ref(), |el, issue| {
                                let color = issue_color(issue, colors);
                                let metadata = [
                                    ("作者", issue.author.name().to_owned()),
                                    (
                                        "处理人",
                                        if issue.assignees.is_empty() {
                                            locale.text("未分配").into()
                                        } else {
                                            issue
                                                .assignees
                                                .iter()
                                                .map(|user| user.name())
                                                .collect::<Vec<_>>()
                                                .join(", ")
                                        },
                                    ),
                                    (
                                        "优先级",
                                        if issue.priority.is_empty() {
                                            "—".into()
                                        } else {
                                            issue.priority.clone()
                                        },
                                    ),
                                    ("创建时间", issue_date(&issue.created_at)),
                                    ("更新时间", issue_date(&issue.updated_at)),
                                    ("评论", issue.comment_count.to_string()),
                                ]
                                .into_iter()
                                .enumerate()
                                .map(|(index, (label, value))| {
                                    let tooltip = value.clone();
                                    div()
                                        .id(("cnb-issue-field", index))
                                        .debug_selector(move || format!("cnb-issue-field-{index}"))
                                        .flex_1()
                                        .flex_basis(px(180.))
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .tooltip(move |window, cx| {
                                            Tooltip::new(tooltip.clone()).build(window, cx)
                                        })
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(rgb(colors.muted))
                                                .child(locale.text(label)),
                                        )
                                        .child(
                                            div()
                                                .debug_selector(move || {
                                                    format!("cnb-issue-field-value-{index}")
                                                })
                                                .min_w_0()
                                                .truncate()
                                                .text_size(px(13.))
                                                .font_weight(gpui::FontWeight::MEDIUM)
                                                .child(value),
                                        )
                                });
                                el.child(
                                    div()
                                        .text_size(px(24.))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(issue.title.clone()),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .px_3()
                                                .py_1()
                                                .rounded_full()
                                                .bg(rgb(color).opacity(0.12))
                                                .text_color(rgb(color))
                                                .text_size(px(12.))
                                                .child(locale.text(if issue.state == "closed" {
                                                    "已关闭"
                                                } else {
                                                    "未关闭"
                                                })),
                                        )
                                        .child(
                                            div()
                                                .text_color(rgb(colors.muted))
                                                .child(format!("#{}", issue.number)),
                                        )
                                        .children(
                                            issue
                                                .labels
                                                .iter()
                                                .map(|label| issue_label(label, colors)),
                                        ),
                                )
                                .child(
                                    div()
                                        .debug_selector(|| "cnb-issue-summary".into())
                                        .min_w_0()
                                        .rounded(px(CARD_RADIUS))
                                        .border_1()
                                        .border_color(rgb(colors.border))
                                        .bg(rgb(colors.surface))
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .debug_selector(|| "cnb-issue-metadata".into())
                                                .p_5()
                                                .flex()
                                                .flex_wrap()
                                                .gap_x_6()
                                                .gap_y_4()
                                                .children(metadata),
                                        )
                                        .child(self.render_cnb_actions(issue, cx)),
                                )
                                .child(
                                    div()
                                        .debug_selector(|| "cnb-issue-body".into())
                                        .min_w_0()
                                        .child(self.render_cnb_markdown(
                                            format!("cnb-body-{}", issue.number),
                                            if issue.body.trim().is_empty() {
                                                locale.text("此 Issue 暂无描述。").to_owned()
                                            } else {
                                                issue.body.clone()
                                            },
                                            cx,
                                        )),
                                )
                                .child(self.render_cnb_comments(cx))
                            }),
                    )
                    .vertical_scrollbar(&self.cnb_detail_scroll),
            )
    }
}
