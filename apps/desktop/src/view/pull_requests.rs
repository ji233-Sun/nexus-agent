use super::*;
use crate::model::{
    issues::{Comment, IssueFilter, IssueLaunchKind, PAGE_SIZE},
    pull_requests::{MergeMethod, PullAction, PullRunKind, pull_url},
};
use gpui_kit::component::{scroll::ScrollableElement as _, spinner::Spinner};

impl NexusView {
    pub(super) fn render_pull_requests(
        &self,
        provider: IssueProvider,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self
            .presenter
            .model()
            .issues(provider)
            .pulls
            .detail_number
            .is_some()
        {
            self.render_pull_detail(provider, cx)
        } else {
            self.render_pull_list(provider, cx)
        }
    }

    fn render_pull_list(&self, provider: IssueProvider, cx: &mut Context<Self>) -> AnyElement {
        let pulls = &self.presenter.model().issues(provider).pulls;
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let page = pulls.page;
        let filter = pulls.filter;
        let loading = pulls.list_request.is_some();
        let next = if provider == IssueProvider::GitHub {
            pulls.page_cursors.contains_key(&(page + 1))
        } else {
            page * PAGE_SIZE < pulls.total
        };
        div()
            .debug_selector(move || format!("{}-pull-list", provider.key()))
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
                        Button::new(SharedString::from(format!(
                            "pull-filter-{}",
                            choice.state()
                        )))
                        .debug_selector(move || {
                            format!("{}-pull-filter-{}", provider.key(), choice.state())
                        })
                        .ghost()
                        .small()
                        .label(locale.text(choice.label()))
                        .selected(filter == choice)
                        .disabled(loading)
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.presenter.load_pull_requests(provider, 1, choice);
                            app.issues_scroll.set_offset(gpui::point(px(0.), px(0.)));
                            cx.notify();
                        }))
                    }))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors.muted))
                            .child(
                                locale
                                    .format("{count} 个 PR", &[("count", pulls.total.to_string())]),
                            ),
                    )
                    .child(
                        Button::new("pull-refresh")
                            .debug_selector(move || format!("{}-pull-refresh", provider.key()))
                            .ghost()
                            .small()
                            .icon(IconName::RotateCw)
                            .label(locale.text("刷新 PR"))
                            .disabled(loading)
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.presenter.load_pull_requests(provider, page, filter);
                                cx.notify();
                            })),
                    ),
            )
            .when_some(pulls.list_error.as_ref(), |el, error| {
                el.child(
                    div()
                        .px_6()
                        .pb_3()
                        .text_color(rgb(colors.danger))
                        .child(error.render(locale).to_owned()),
                )
            })
            .child(
                div()
                    .id(SharedString::from(format!(
                        "{}-pull-list-scroll",
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
                                .items_center()
                                .gap_3()
                                .child(Spinner::new())
                                .child(locale.text("正在读取 PR…")),
                        )
                    })
                    .when(
                        !loading && pulls.pulls.is_empty() && pulls.list_error.is_none(),
                        |el| {
                            el.child(
                                div()
                                    .py_16()
                                    .text_color(rgb(colors.muted))
                                    .child(locale.text("暂无 PR。")),
                            )
                        },
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .children(pulls.pulls.iter().map(|pull| {
                                let number = pull.number.clone();
                                let selector = format!("{}-pull-{number}", provider.key());
                                div()
                                    .id(SharedString::from(selector.clone()))
                                    .debug_selector(move || selector.clone())
                                    .p_4()
                                    .min_w_0()
                                    .rounded(px(CARD_RADIUS))
                                    .bg(rgb(colors.surface))
                                    .border_1()
                                    .border_color(rgb(colors.border))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(colors.elevated)))
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(14.))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .child(pull.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .gap_2()
                                            .text_size(px(12.))
                                            .text_color(rgb(colors.muted))
                                            .child(format!(
                                                "#{} · {} · {}",
                                                pull.number,
                                                pull.author,
                                                locale.text(pull_state(&pull.state))
                                            ))
                                            .child(format!(
                                                "{} → {}",
                                                pull.head_branch, pull.base_branch
                                            ))
                                            .when(pull.draft, |el| el.child(locale.text("草稿"))),
                                    )
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.presenter.select_pull_request(provider, number.clone());
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
                    .flex_none()
                    .border_t_1()
                    .border_color(rgb(colors.border))
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
                                        pulls.total.div_ceil(PAGE_SIZE).max(page).to_string(),
                                    ),
                                ],
                            )),
                    )
                    .child(div().flex().gap_2().children(
                        [(false, "上一页"), (true, "下一页")].map(|(forward, label)| {
                            Button::new(label)
                                .debug_selector(move || {
                                    format!(
                                        "{}-pull-{}",
                                        provider.key(),
                                        if forward { "next" } else { "previous" }
                                    )
                                })
                                .outline()
                                .small()
                                .label(locale.text(label))
                                .disabled(loading || if forward { !next } else { page <= 1 })
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.presenter.load_pull_requests(
                                        provider,
                                        if forward { page + 1 } else { page - 1 },
                                        filter,
                                    );
                                    app.issues_scroll.set_offset(gpui::point(px(0.), px(0.)));
                                    cx.notify();
                                }))
                        }),
                    )),
            )
            .into_any_element()
    }

    fn render_pull_detail(&self, provider: IssueProvider, cx: &mut Context<Self>) -> AnyElement {
        let issues = self.presenter.model().issues(provider);
        let pulls = &issues.pulls;
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let url = pull_url(
            provider,
            issues.repository.as_deref().unwrap_or_default(),
            pulls.detail_number.as_deref().unwrap_or_default(),
        );
        let busy = pulls.detail_request.is_some() || pulls.action_request.is_some();
        div()
            .debug_selector(move || format!("{}-pull-detail", provider.key()))
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
                    .gap_2()
                    .child(
                        Button::new("pull-back")
                            .debug_selector(move || format!("{}-pull-back", provider.key()))
                            .ghost()
                            .small()
                            .label(locale.text("返回列表"))
                            .disabled(pulls.action_request.is_some())
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.presenter.close_pull_request(provider);
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("pull-detail-refresh")
                            .debug_selector(move || {
                                format!("{}-pull-detail-refresh", provider.key())
                            })
                            .ghost()
                            .small()
                            .icon(IconName::RotateCw)
                            .label(locale.text("刷新 PR"))
                            .disabled(busy)
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.presenter.refresh_pull_request(provider);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("pull-open")
                            .ghost()
                            .small()
                            .icon(IconName::ExternalLink)
                            .label(locale.format(
                                "在 {provider} 中打开",
                                &[("provider", provider.name().into())],
                            ))
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    ),
            )
            .child(
                div()
                    .id(SharedString::from(format!(
                        "{}-pull-detail-scroll",
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
                            .when(pulls.detail_request.is_some(), |el| {
                                el.child(
                                    div()
                                        .py_4()
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .child(Spinner::new())
                                        .child(locale.text("正在读取 PR、全部审查讨论与 CI…")),
                                )
                            })
                            .when_some(pulls.detail_error.as_ref(), |el, error| {
                                el.child(
                                    div()
                                        .text_color(rgb(colors.danger))
                                        .child(error.render(locale).to_owned()),
                                )
                            })
                            .when_some(pulls.detail.as_ref(), |el, detail| {
                                let pull = &detail.pull;
                                el.child(
                                    div()
                                        .text_size(px(24.))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(pull.title.clone()),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap_2()
                                        .text_size(px(12.))
                                        .text_color(rgb(colors.muted))
                                        .child(format!(
                                            "#{} · {} · {}",
                                            pull.number,
                                            pull.author,
                                            locale.text(pull_state(&pull.state))
                                        ))
                                        .child(format!(
                                            "{}:{} → {}",
                                            pull.head_repository,
                                            pull.head_branch,
                                            pull.base_branch
                                        ))
                                        .when(pull.draft, |el| el.child(locale.text("草稿"))),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(rgb(colors.muted))
                                        .child(pull.merge_status.clone()),
                                )
                                .child(self.render_pull_actions(provider, cx))
                                .child(self.render_issue_markdown(
                                    provider,
                                    format!("{}-pull-body-{}", provider.key(), pull.number),
                                    pull.body.clone(),
                                    cx,
                                ))
                                .child(self.render_pull_checks(provider, cx))
                                .when(!detail.stack.is_empty(), |el| {
                                    el.child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            .child(locale.text("Stack PR（依赖顺序）"))
                                            .children(detail.stack.iter().map(|item| {
                                                let number = item.number.clone();
                                                Button::new(SharedString::from(format!(
                                                    "stack-{number}"
                                                )))
                                                .debug_selector(move || {
                                                    format!("{}-stack-{number}", provider.key())
                                                })
                                                .ghost()
                                                .small()
                                                .label(format!(
                                                    "#{} {} · {} → {}",
                                                    item.number,
                                                    item.title,
                                                    item.head_branch,
                                                    item.base_branch
                                                ))
                                                .disabled(busy)
                                                .on_click(cx.listener({
                                                    let number = item.number.clone();
                                                    move |app, _, _, cx| {
                                                        app.presenter.select_pull_request(
                                                            provider,
                                                            number.clone(),
                                                        );
                                                        app.issue_detail_scroll.set_offset(
                                                            gpui::point(px(0.), px(0.)),
                                                        );
                                                        cx.notify();
                                                    }
                                                }))
                                            })),
                                    )
                                })
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_3()
                                        .child(locale.text("审查结论"))
                                        .children(detail.reviews.iter().map(|review| {
                                            div()
                                                .p_4()
                                                .rounded(px(CARD_RADIUS))
                                                .bg(rgb(colors.surface))
                                                .flex()
                                                .flex_col()
                                                .gap_3()
                                                .child(format!(
                                                    "{} · {} · Review {}",
                                                    review.author, review.state, review.id
                                                ))
                                                .child(self.render_issue_markdown(
                                                    provider,
                                                    format!(
                                                        "{}-pull-review-{}",
                                                        provider.key(),
                                                        review.id
                                                    ),
                                                    review.body.clone(),
                                                    cx,
                                                ))
                                        })),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_3()
                                        .child(locale.text("审查 Conversations"))
                                        .children(detail.threads.iter().map(|thread| {
                                            let selector = format!(
                                                "{}-pull-thread-{}",
                                                provider.key(),
                                                thread.id
                                            );
                                            div()
                                                .debug_selector(move || selector.clone())
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
                                                        .gap_2()
                                                        .text_size(px(12.))
                                                        .child(format!(
                                                            "{}:{} · {}",
                                                            thread.path,
                                                            thread
                                                                .line
                                                                .map(|line| line.to_string())
                                                                .unwrap_or_default(),
                                                            thread.id
                                                        ))
                                                        .when(
                                                            provider == IssueProvider::GitHub,
                                                            |el| {
                                                                el.child(locale.text(
                                                                    if thread.resolved {
                                                                        "已解决"
                                                                    } else {
                                                                        "未解决"
                                                                    },
                                                                ))
                                                            },
                                                        )
                                                        .when(thread.outdated, |el| {
                                                            el.child(locale.text("已过时"))
                                                        }),
                                                )
                                                .children(thread.comments.iter().map(|comment| {
                                                    self.render_pull_comment(provider, comment, cx)
                                                }))
                                        })),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_3()
                                        .child(locale.text("评论"))
                                        .children(detail.comments.iter().map(|comment| {
                                            self.render_pull_comment(provider, comment, cx)
                                        })),
                                )
                            }),
                    )
                    .vertical_scrollbar(&self.issue_detail_scroll),
            )
            .into_any_element()
    }

    fn render_pull_comment(
        &self,
        provider: IssueProvider,
        comment: &Comment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = palette(cx);
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors.muted))
                    .child(format!(
                        "{} · {} · {}",
                        comment.author.name(),
                        comment.created_at,
                        comment.id
                    )),
            )
            .child(self.render_issue_markdown(
                provider,
                format!("{}-pull-comment-{}", provider.key(), comment.id),
                comment.body.clone(),
                cx,
            ))
            .into_any_element()
    }

    fn render_pull_checks(&self, provider: IssueProvider, cx: &mut Context<Self>) -> AnyElement {
        let Some(detail) = self
            .presenter
            .model()
            .issues(provider)
            .pulls
            .detail
            .as_ref()
        else {
            return div().into_any_element();
        };
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        div()
            .debug_selector(move || format!("{}-pull-checks", provider.key()))
            .flex()
            .flex_col()
            .gap_3()
            .child(locale.text("当前 Head 的 CI"))
            .when(detail.checks.is_empty(), |el| {
                el.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(colors.muted))
                        .child(locale.text("当前 Head 暂无 CI 检查。")),
                )
            })
            .children(detail.checks.iter().enumerate().map(|(index, check)| {
                let url = check.url.clone();
                div()
                    .p_3()
                    .rounded(px(CONTROL_RADIUS))
                    .bg(rgb(colors.surface))
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_color(rgb(if check.failed() {
                                        colors.danger
                                    } else {
                                        colors.text_secondary
                                    }))
                                    .child(format!("{} · {}", check.name, check.state)),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors.muted))
                                    .child(check.description.clone()),
                            ),
                    )
                    .when(check.url.starts_with("https://"), |el| {
                        el.child(
                            Button::new(SharedString::from(format!("pull-check-{index}")))
                                .ghost()
                                .small()
                                .icon(IconName::ExternalLink)
                                .label(locale.text("查看 CI"))
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                    })
            }))
            .into_any_element()
    }

    fn render_pull_actions(&self, provider: IssueProvider, cx: &mut Context<Self>) -> AnyElement {
        let model = self.presenter.model();
        let pulls = &model.issues(provider).pulls;
        let Some(detail) = pulls.detail.as_ref() else {
            return div().into_any_element();
        };
        let locale = model.language;
        let colors = palette(cx);
        let busy = pulls.detail_request.is_some()
            || pulls.action_request.is_some()
            || pulls.detail_error.is_some();
        let closed = detail.pull.state != "open";
        let method = pulls.merge_method;
        div()
            .debug_selector(move || format!("{}-pull-actions", provider.key()))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .children(MergeMethod::ALL.map(|choice| {
                        Button::new(SharedString::from(format!(
                            "pull-merge-method-{}",
                            choice.key()
                        )))
                        .ghost()
                        .small()
                        .label(choice.key())
                        .selected(method == choice)
                        .disabled(busy || closed || pulls.confirmation.is_some())
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.presenter.set_pull_merge_method(provider, choice);
                            cx.notify();
                        }))
                    }))
                    .child(
                        Button::new("pull-merge")
                            .debug_selector(move || format!("{}-pull-merge", provider.key()))
                            .primary()
                            .small()
                            .label(locale.text("合并 PR"))
                            .disabled(busy || !detail.pull.can_merge())
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.presenter
                                    .confirm_pull_action(provider, Some(PullAction::Merge(method)));
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("pull-close")
                            .debug_selector(move || format!("{}-pull-close", provider.key()))
                            .outline()
                            .small()
                            .label(locale.text("关闭 PR"))
                            .disabled(busy || closed)
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.presenter
                                    .confirm_pull_action(provider, Some(PullAction::Close));
                                cx.notify();
                            })),
                    ),
            )
            .when_some(pulls.confirmation, |el, action| {
                let operation = match action {
                    PullAction::Merge(method) => {
                        format!("{} ({})", locale.text(action.label()), method.key())
                    }
                    PullAction::Close => locale.text(action.label()).to_owned(),
                };
                el.child(
                    div()
                        .p_3()
                        .rounded(px(CONTROL_RADIUS))
                        .bg(rgb(colors.surface))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_3()
                        .child(locale.format(
                            "确认{operation} #{number}？",
                            &[
                                ("operation", operation),
                                ("number", detail.pull.number.clone()),
                            ],
                        ))
                        .child(
                            Button::new("pull-confirm")
                                .debug_selector(move || format!("{}-pull-confirm", provider.key()))
                                .primary()
                                .small()
                                .label(locale.text("确认"))
                                .disabled(busy)
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.presenter.act_on_pull_request(provider);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("pull-cancel")
                                .ghost()
                                .small()
                                .label(locale.text("取消"))
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.presenter.confirm_pull_action(provider, None);
                                    cx.notify();
                                })),
                        ),
                )
            })
            .child(
                div().flex().flex_wrap().items_center().gap_2().children(
                    [
                        PullRunKind::ResolveConflicts,
                        PullRunKind::Review,
                        PullRunKind::AddressReviews,
                        PullRunKind::FixCi,
                        PullRunKind::Stack,
                    ]
                    .into_iter()
                    .filter(|kind| *kind != PullRunKind::Stack || provider == IssueProvider::GitHub)
                    .enumerate()
                    .map(|(index, kind)| {
                        Button::new(SharedString::from(format!("pull-run-{index}")))
                            .debug_selector(move || format!("{}-pull-run-{index}", provider.key()))
                            .outline()
                            .small()
                            .icon(IconName::Bot)
                            .label(locale.text(kind.label()))
                            .disabled(
                                busy || closed
                                    || !detail.can_run(provider, kind)
                                    || model.active_run.is_some()
                                    || model.occupied_run_slots() >= 2,
                            )
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.open_issue_launch(
                                    provider,
                                    IssueLaunchKind::Pull(kind),
                                    window,
                                    cx,
                                );
                            }))
                    }),
                ),
            )
            .when(pulls.action_request.is_some(), |el| {
                el.child(Spinner::new())
            })
            .when_some(pulls.action_error.as_ref(), |el, error| {
                el.child(
                    div()
                        .text_color(rgb(colors.danger))
                        .child(error.render(locale).to_owned()),
                )
            })
            .when_some(pulls.action_success.as_ref(), |el, message| {
                el.child(
                    div()
                        .text_color(rgb(colors.success))
                        .child(message.render(locale).to_owned()),
                )
            })
            .into_any_element()
    }
}

fn pull_state(state: &str) -> &'static str {
    match state {
        "merged" => "已合并",
        "closed" => "已关闭",
        _ => "未关闭",
    }
}
