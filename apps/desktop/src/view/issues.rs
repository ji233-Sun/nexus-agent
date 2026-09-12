use super::*;
use crate::model::issues::{Issue, IssueAction, IssueFilter, Label, PAGE_SIZE};
use gpui_kit::component::{scroll::ScrollableElement as _, spinner::Spinner};

pub(super) fn provider_icon(provider: IssueProvider, size: f32, color: u32) -> impl IntoElement {
    // GPUI only paints an SVG when the element has an explicit text color.
    gpui::svg()
        .data(match provider {
            IssueProvider::Cnb => include_bytes!("../../assets/icons/cnb.svg").as_slice(),
            IssueProvider::GitHub => include_bytes!("../../assets/icons/github.svg").as_slice(),
        })
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
    fn send_issue_to_chat(
        &mut self,
        provider: IssueProvider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(prompt) = self.presenter.prepare_issue_chat(provider) else {
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

    fn render_issue_actions(
        &self,
        provider: IssueProvider,
        issue: &Issue,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let issues = self.presenter.model().issues(provider);
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let busy = issues.action_request.is_some();
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
                        action_button(SharedString::from(format!("{}-chat", provider.key())))
                            .debug_selector(move || format!("{}-chat", provider.key()))
                            .primary()
                            .icon(IconName::Bot)
                            .label(locale.text("用 AI 处理"))
                            .tooltip(
                                locale
                                    .text("将完整 Issue 和评论加入聊天草稿，选择 Harness 后发送。"),
                            )
                            .disabled(
                                busy || issues.comments.is_none()
                                    || issues.comments_request.is_some(),
                            )
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.send_issue_to_chat(provider, window, cx)
                            })),
                    )
                    .child(
                        action_button(SharedString::from(format!(
                            "{}-assign-self",
                            provider.key()
                        )))
                        .debug_selector(move || format!("{}-assign-self", provider.key()))
                        .ghost()
                        .icon(IconName::User)
                        .label(locale.text("指派给我"))
                        .disabled(busy)
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.presenter
                                .act_on_issue(provider, IssueAction::AssignSelf);
                            cx.notify();
                        })),
                    )
                    .child(
                        action_button(SharedString::from(format!(
                            "{}-change-state",
                            provider.key()
                        )))
                        .debug_selector(move || format!("{}-change-state", provider.key()))
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
                            app.presenter
                                .act_on_issue(provider, IssueAction::SetState(state));
                            cx.notify();
                        })),
                    )
                    .when(provider == IssueProvider::Cnb, |row| {
                        row.child(
                            action_button(SharedString::from(format!("{}-npc", provider.key())))
                                .debug_selector(move || format!("{}-npc", provider.key()))
                                .ghost()
                                .icon(IconName::Bot)
                                .label("CodeBuddy NPC")
                                .tooltip(locale.text(
                                    "向此 Issue 发布评论，委托 CodeBuddy 完成开发并创建 PR。",
                                ))
                                .disabled(busy || issues.npc_comment.is_some())
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.presenter.act_on_issue(provider, IssueAction::StartNpc);
                                    cx.notify();
                                })),
                        )
                    }),
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
            .when_some(issues.action_error.as_ref(), |el, error| {
                el.child(
                    div()
                        .text_color(rgb(colors.danger))
                        .child(error.render(locale).to_owned()),
                )
            })
            .when_some(issues.action_success.as_ref(), |el, message| {
                el.child(
                    div()
                        .text_color(rgb(colors.success))
                        .child(message.render(locale).to_owned()),
                )
            })
            .when_some(issues.npc_comment.as_ref(), |el, comment| {
                if let Some(url) = comment.action_url() {
                    let url = url.to_owned();
                    el.child(
                        Button::new(SharedString::from(format!("{}-npc-action", provider.key())))
                            .debug_selector(move || format!("{}-npc-action", provider.key()))
                            .outline()
                            .small()
                            .icon(IconName::ExternalLink)
                            .label(locale.text("查看 Action"))
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                } else {
                    el.child(
                        Button::new(SharedString::from(format!(
                            "{}-npc-refresh",
                            provider.key()
                        )))
                        .debug_selector(move || format!("{}-npc-refresh", provider.key()))
                        .outline()
                        .small()
                        .label(locale.text(if issues.npc_request.is_some() {
                            "正在获取 Action 链接…"
                        } else {
                            "刷新 NPC 状态"
                        }))
                        .disabled(issues.npc_request.is_some())
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.presenter.refresh_cnb_npc_action();
                            cx.notify();
                        })),
                    )
                }
            })
            .when_some(issues.npc_error.as_ref(), |el, error| {
                el.child(
                    div()
                        .text_color(rgb(colors.danger))
                        .child(error.render(locale).to_owned()),
                )
            })
    }

    fn render_issue_markdown(
        &self,
        provider: IssueProvider,
        id: String,
        text: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let issues = self.presenter.model().issues(provider);
        let colors = palette(cx);
        TextView::markdown(SharedString::from(id), text)
            .markdown_extensions(if provider == IssueProvider::Cnb {
                super::cnb_media::extensions(
                    issues.repository.as_deref().unwrap_or_default(),
                    issues.cli.as_ref().map(|cli| cli.path.as_path()),
                    self.presenter.model().language,
                )
            } else {
                Default::default()
            })
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

    fn render_issue_comments(
        &self,
        provider: IssueProvider,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let issues = self.presenter.model().issues(provider);
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        div()
            .debug_selector(move || format!("{}-comments", provider.key()))
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
                        Button::new(SharedString::from(format!(
                            "{}-comments-refresh",
                            provider.key()
                        )))
                        .debug_selector(move || format!("{}-comments-refresh", provider.key()))
                        .ghost()
                        .small()
                        .icon(IconName::RotateCw)
                        .label(locale.text("刷新评论"))
                        .disabled(issues.comments_request.is_some())
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.presenter.load_issue_comments(provider);
                            cx.notify();
                        })),
                    ),
            )
            .when(issues.comments_request.is_some(), |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(Spinner::new())
                        .child(locale.text("正在读取全部评论…")),
                )
            })
            .when_some(issues.comments_error.as_ref(), |el, error| {
                el.child(
                    div()
                        .text_color(rgb(colors.danger))
                        .child(error.render(locale).to_owned()),
                )
            })
            .when_some(issues.comments.as_ref(), |el, comments| {
                el.when(comments.is_empty(), |el| {
                    el.child(locale.text("此 Issue 暂无评论。"))
                })
                .children(comments.iter().map(|comment| {
                    let selector = format!("{}-comment-{}", provider.key(), comment.id);
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
                        .child(self.render_issue_markdown(
                            provider,
                            format!("{}-comment-body-{}", provider.key(), comment.id),
                            comment.body.clone(),
                            cx,
                        ))
                        .when_some(comment.action_url(), |el, url| {
                            let url = url.to_owned();
                            el.child(
                                Button::new(SharedString::from(format!(
                                    "{}-comment-action-{}",
                                    provider.key(),
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

    pub(super) fn render_issue_settings(
        &self,
        provider: IssueProvider,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let model = self.presenter.model();
        let issues = model.issues(provider);
        let locale = model.language;
        let colors = palette(cx);
        let status = if issues.detection_request.is_some() {
            locale
                .format(
                    "正在检测 {provider} CLI…",
                    &[("provider", provider.name().into())],
                )
                .to_owned()
        } else if let Some(error) = &issues.detection_error {
            error.render(locale).to_owned()
        } else if let Some(cli) = &issues.cli {
            format!("{} · {}", cli.version, cli.path.display())
        } else {
            locale
                .format(
                    "检测本机 {provider} CLI 后即可读取项目 Issues。",
                    &[("provider", provider.name().into())],
                )
                .to_owned()
        };
        settings::settings_group(
            colors,
            provider.name(),
            [
                div()
                    .py_5()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(provider_icon(provider, 28., colors.text))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(locale.format(
                                "启用 {provider} 集成",
                                &[("provider", provider.name().into())],
                            ))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors.muted))
                                    .child(locale.format(
                                    "自动识别项目的 {provider} remote，使用本机 CLI 浏览 Issues。",
                                    &[("provider", provider.name().into())],
                                )),
                            ),
                    )
                    .child(
                        div()
                            .debug_selector(move || format!("{}-enabled", provider.key()))
                            .child(
                                Switch::new(SharedString::from(format!(
                                    "{}-enabled",
                                    provider.key()
                                )))
                                .accessibility_label(locale.format(
                                    "启用 {provider} 集成",
                                    &[("provider", provider.name().into())],
                                ))
                                .checked(issues.enabled)
                                .on_click(cx.listener(
                                    move |app, enabled, _, cx| {
                                        app.presenter.set_issues_enabled(provider, *enabled);
                                        cx.notify();
                                    },
                                )),
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
                                Button::new(SharedString::from(format!(
                                    "{}-detect",
                                    provider.key()
                                )))
                                .outline()
                                .small()
                                .icon(IconName::RotateCw)
                                .label(locale.text("重新检测"))
                                .disabled(issues.detection_request.is_some())
                                .on_click(cx.listener(
                                    move |app, _, _, cx| {
                                        app.presenter.inspect_issues(provider);
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                Button::new(SharedString::from(format!(
                                    "{}-documentation",
                                    provider.key()
                                )))
                                .ghost()
                                .small()
                                .icon(IconName::ExternalLink)
                                .label(locale.text("安装与登录说明"))
                                .on_click(move |_, _, cx| cx.open_url(provider.documentation())),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors.muted))
                            .line_height(relative(1.6))
                            .child(locale.format(
                                "先在终端安装 {cli} 并运行 {login}。登录凭据由 CLI 管理。",
                                &[
                                    ("cli", provider.executable().into()),
                                    ("login", provider.login_command().into()),
                                ],
                            )),
                    ),
            ],
        )
    }

    pub(super) fn render_issues(
        &self,
        provider: IssueProvider,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let model = self.presenter.model();
        let issues = model.issues(provider);
        let colors = palette(cx);
        let locale = model.language;
        let repository = issues.repository.clone().unwrap_or_default();
        let repo_url = provider.repository_url(&repository);
        div()
            .debug_selector(move || format!("{}-page", provider.key()))
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
                    .child(provider_icon(provider, 22., colors.text))
                    .child(
                        div()
                            .debug_selector(move || format!("{}-breadcrumb", provider.key()))
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .debug_selector(move || {
                                        format!("{}-breadcrumb-root", provider.key())
                                    })
                                    .flex_none()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(provider.name()),
                            )
                            .children(repository.split('/').enumerate().flat_map(
                                |(index, segment)| {
                                    [
                                        div()
                                            .debug_selector(move || {
                                                format!(
                                                    "{}-breadcrumb-separator-{index}",
                                                    provider.key()
                                                )
                                            })
                                            .flex_none()
                                            .text_color(rgb(colors.muted))
                                            .child("/"),
                                        div()
                                            .debug_selector(move || {
                                                format!(
                                                    "{}-breadcrumb-segment-{index}",
                                                    provider.key()
                                                )
                                            })
                                            .min_w_0()
                                            .truncate()
                                            .child(segment.to_owned()),
                                    ]
                                },
                            )),
                    )
                    .child(
                        Button::new(SharedString::from(format!("{}-repository", provider.key())))
                            .debug_selector(move || format!("{}-repository", provider.key()))
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
                            .debug_selector(move || format!("{}-issue-tab", provider.key()))
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
            .child(if issues.cli.is_none() {
                div()
                    .p_8()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(locale.format(
                        "安装并登录 {provider} CLI 后即可查看 Issues。",
                        &[("provider", provider.name().into())],
                    ))
                    .when_some(issues.detection_error.as_ref(), |el, error| {
                        el.child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(colors.muted))
                                .child(error.render(locale).to_owned()),
                        )
                    })
                    .child(
                        Button::new(SharedString::from(format!(
                            "{}-open-settings",
                            provider.key()
                        )))
                        .outline()
                        .label(locale.text("打开 Source Control 设置"))
                        .on_click(cx.listener(
                            move |app, _, window, cx| {
                                app.settings_open = true;
                                app.select_settings_section(
                                    SettingsSection::SourceControl,
                                    window,
                                    cx,
                                );
                                cx.notify();
                            },
                        )),
                    )
                    .into_any_element()
            } else if issues.detail_number.is_some() {
                self.render_issue_detail(provider, cx).into_any_element()
            } else {
                self.render_issue_list(provider, cx).into_any_element()
            })
    }

    fn render_issue_list(
        &self,
        provider: IssueProvider,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let issues = self.presenter.model().issues(provider);
        let colors = palette(cx);
        let locale = self.presenter.model().language;
        let loading = issues.list_request.is_some();
        let page = issues.page;
        let filter = issues.filter;
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
                            .debug_selector(move || {
                                format!("{}-filter-{}", provider.key(), choice.state())
                            })
                            .selected(filter == choice)
                            .disabled(loading)
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.presenter.load_issues(provider, 1, choice);
                                app.issues_scroll.set_offset(gpui::point(px(0.), px(0.)));
                                cx.notify();
                            }))
                    }))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors.muted))
                            .child(locale.format(
                                "{count} 个 Issues",
                                &[("count", issues.total.to_string())],
                            )),
                    )
                    .child(
                        Button::new(SharedString::from(format!("{}-refresh", provider.key())))
                            .ghost()
                            .small()
                            .icon(IconName::RotateCw)
                            .tooltip(locale.text("刷新 Issues"))
                            .accessibility_label(locale.text("刷新 Issues"))
                            .disabled(loading)
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.presenter.load_issues(provider, page, filter);
                                cx.notify();
                            })),
                    ),
            )
            .when_some(issues.list_error.as_ref(), |el, error| {
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
                    .id(SharedString::from(format!(
                        "{}-list-scroll",
                        provider.key()
                    )))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.issues_scroll)
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
                        !loading && issues.issues.is_empty() && issues.list_error.is_none(),
                        |el| {
                            el.child(
                                div()
                                    .debug_selector(move || format!("{}-empty", provider.key()))
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
                            .children(issues.issues.iter().map(|issue| {
                                let number = issue.number.clone();
                                let selector = number.clone();
                                div()
                                    .id(SharedString::from(format!(
                                        "{}-issue-{number}",
                                        provider.key()
                                    )))
                                    .debug_selector(move || {
                                        format!("{}-issue-{selector}", provider.key())
                                    })
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
                                        app.presenter.select_issue(provider, number.clone());
                                        app.issue_detail_scroll
                                            .set_offset(gpui::point(px(0.), px(0.)));
                                        cx.notify();
                                    }))
                            })),
                    )
                    .vertical_scrollbar(&self.issues_scroll),
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
                                    (
                                        "pages",
                                        issues.total.div_ceil(PAGE_SIZE).max(page).to_string(),
                                    ),
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
                                        format!("{}-next", provider.key())
                                    } else {
                                        format!("{}-previous", provider.key())
                                    }
                                })
                                .disabled(
                                    loading
                                        || if next {
                                            page * PAGE_SIZE >= issues.total
                                                || (provider == IssueProvider::GitHub
                                                    && !issues
                                                        .page_cursors
                                                        .contains_key(&(page + 1)))
                                        } else {
                                            page <= 1
                                        },
                                )
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.presenter.load_issues(
                                        provider,
                                        if next { page + 1 } else { page - 1 },
                                        filter,
                                    );
                                    app.issues_scroll.set_offset(gpui::point(px(0.), px(0.)));
                                    cx.notify();
                                }))
                        }),
                    )),
            )
    }

    fn render_issue_detail(
        &self,
        provider: IssueProvider,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let issues = self.presenter.model().issues(provider);
        let colors = palette(cx);
        let locale = self.presenter.model().language;
        let number = issues.detail_number.clone().unwrap_or_default();
        let issue_url =
            provider.issue_url(issues.repository.as_deref().unwrap_or_default(), &number);
        div()
            .debug_selector(move || format!("{}-detail", provider.key()))
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
                        Button::new(SharedString::from(format!("{}-back", provider.key())))
                            .debug_selector(move || format!("{}-back", provider.key()))
                            .ghost()
                            .small()
                            .icon(IconName::ArrowLeft)
                            .label(locale.text("返回 Issues"))
                            .disabled(issues.action_request.is_some())
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.presenter.close_issue(provider);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("{}-open-issue", provider.key())))
                            .ghost()
                            .small()
                            .icon(IconName::ExternalLink)
                            .label(locale.format(
                                "在 {provider} 中打开",
                                &[("provider", provider.name().into())],
                            ))
                            .on_click(move |_, _, cx| cx.open_url(&issue_url)),
                    ),
            )
            .child(
                div()
                    .id(SharedString::from(format!(
                        "{}-detail-scroll",
                        provider.key()
                    )))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.issue_detail_scroll)
                    .px_8()
                    .pb_8()
                    .child(
                        div()
                            .max_w(px(CONTENT_WIDTH))
                            .mx_auto()
                            .flex()
                            .flex_col()
                            .gap_5()
                            .when(issues.detail_request.is_some(), |el| {
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
                            .when_some(issues.detail_error.as_ref(), |el, error| {
                                el.child(
                                    div()
                                        .text_color(rgb(colors.danger))
                                        .child(error.render(locale).to_owned()),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "{}-detail-retry",
                                        provider.key()
                                    )))
                                    .outline()
                                    .small()
                                    .label(locale.text("重试"))
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.presenter.select_issue(provider, number.clone());
                                        cx.notify();
                                    })),
                                )
                            })
                            .when_some(issues.detail.as_ref(), |el, issue| {
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
                                        .child(self.render_issue_actions(provider, issue, cx)),
                                )
                                .child(
                                    div()
                                        .debug_selector(move || {
                                            format!("{}-issue-body", provider.key())
                                        })
                                        .min_w_0()
                                        .child(self.render_issue_markdown(
                                            provider,
                                            format!("{}-body-{}", provider.key(), issue.number),
                                            if issue.body.trim().is_empty() {
                                                locale.text("此 Issue 暂无描述。").to_owned()
                                            } else {
                                                issue.body.clone()
                                            },
                                            cx,
                                        )),
                                )
                                .child(self.render_issue_comments(provider, cx))
                            }),
                    )
                    .vertical_scrollbar(&self.issue_detail_scroll),
            )
    }
}
