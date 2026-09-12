use super::*;
use crate::infrastructure::workspace_opener::{self, OpenTarget};
use crate::model::workspace::{WorkspaceKind, WorkspaceStatus};

fn environment_row(
    id: &'static str,
    icon: impl IntoElement,
    label: impl Into<SharedString>,
    colors: Palette,
) -> Button {
    Button::new(id)
        .debug_selector(move || id.into())
        .ghost()
        .small()
        .w_full()
        .h(px(38.))
        .flex_none()
        .px_2()
        .rounded(px(CONTROL_RADIUS))
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(13.))
        .text_color(rgb(colors.text_secondary))
        .cursor_pointer()
        .child(div().w(px(18.)).flex_none().child(icon))
        .child(div().flex_1().min_w_0().truncate().child(label.into()))
}

impl NexusView {
    pub(super) fn working_directory_opener(
        &self,
        id: &'static str,
        button: Button,
        cx: &mut Context<Self>,
    ) -> AnimatedDropdown {
        let model = self.presenter.model();
        let locale = model.language;
        let directory = model.working_directory();
        let app = cx.entity();
        let button = button
            .accessibility_label(locale.text("打开方式"))
            .disabled(directory.is_none())
            .tooltip(match directory {
                Some(directory) => format!("{}\n{directory}", locale.text("打开方式")),
                None => locale.text("尚未选择工作目录。").to_owned(),
            });
        AnimatedDropdown::new(
            // Switching conversations must also discard an already-open menu.
            SharedString::from(format!(
                "{id}-{}-{}",
                model.conversation.id,
                directory.unwrap_or_default()
            )),
            button,
            self.reduced_motion,
            move |menu, _, _| {
                OpenTarget::ALL
                    .into_iter()
                    .filter(|target| target.supported(std::env::consts::OS))
                    .fold(menu.min_w(px(180.)), |menu, target| {
                        let app = app.clone();
                        menu.item(
                            PopupMenuItem::new(target.label(locale))
                                .icon(match target {
                                    OpenTarget::FileManager => IconName::Folder,
                                    OpenTarget::VsCode => IconName::FileText,
                                    OpenTarget::Ghostty => IconName::SquareTerminal,
                                })
                                .on_click(move |_, window, cx| {
                                    app.update(cx, |app, cx| {
                                        app.open_working_directory(target, window, cx)
                                    });
                                }),
                        )
                    })
            },
        )
    }

    fn open_working_directory(
        &mut self,
        target: OpenTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Resolve at selection time, never from a directory captured by the menu.
        let directory = self
            .presenter
            .model()
            .working_directory()
            .map(str::to_owned);
        cx.spawn_in(window, async move |this, cx| {
            if let Err(error) = workspace_opener::open(target, directory).await {
                let _ = this.update_in(cx, |app, window, cx| {
                    let locale = app.presenter.model().language;
                    drop(window.prompt(
                        PromptLevel::Warning,
                        locale.text("无法打开工作目录"),
                        Some(error.render(locale)),
                        &[PromptButton::ok(locale.text("确定"))],
                        cx,
                    ));
                });
            }
        })
        .detach();
    }

    pub(super) fn sync_commit_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.presenter.model();
        self.commit_inputs.retain(|id, _| {
            model
                .all_conversations()
                .any(|conversation| conversation.id == *id)
        });
        if !model.changes_sidebar_open || !model.commit_editor_open {
            return;
        }
        let id = model.conversation.id;
        let message = model.commit_message.clone();
        let placeholder = model.language.text("填写提交说明，或根据所选变更生成…");
        if let Some(input) = self.commit_inputs.get(&id) {
            if input.read(cx).value().as_str() != message {
                input.update(cx, |input, cx| input.set_value(message, window, cx));
            }
        } else {
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(3, 5)
                    .default_value(message)
                    .placeholder(placeholder)
            });
            cx.subscribe(&input, move |app, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change)
                    && app.presenter.model().conversation.id == id
                {
                    app.presenter
                        .set_commit_message(input.read(cx).value().to_string());
                    cx.notify();
                }
            })
            .detach();
            self.commit_inputs.insert(id, input);
        }
    }

    pub(super) fn render_changes_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let locale = model.language;
        let colors = palette(cx);
        let review = model.workspace_review.as_ref();
        let generating = model.commit_message_request.is_some();
        let running = model.working_directory().is_some_and(|cwd| {
            model.all_conversations().any(|conversation| {
                conversation.active_run.is_some()
                    && conversation
                        .active_checkout
                        .as_ref()
                        .is_some_and(|checkout| Path::new(cwd).starts_with(checkout))
            })
        });
        let busy = model.workspace_busy || generating || running;
        let has_changes = review.is_some_and(|review| !review.dirty_paths.is_empty());
        let kind = model
            .selected_workspace
            .as_ref()
            .map_or(model.workspace_draft.kind, |workspace| workspace.kind);
        let branch = review
            .and_then(|review| review.branch.as_ref())
            .or(model.workspace_branch.as_ref())
            .cloned()
            .unwrap_or_else(|| if review.is_some() { "HEAD" } else { "—" }.into());
        let branch_tooltip = match review {
            Some(review) => format!("{branch}\n{}", review.head),
            None => branch.clone(),
        };
        let count = review.map_or(0, |review| review.dirty_paths.len());
        let summary = if model.workspace_busy && review.is_none() {
            locale.text("正在读取变更…").to_owned()
        } else if review.is_none() {
            "—".into()
        } else if count == 0 {
            locale.text("无变更").to_owned()
        } else {
            locale.format("{count} 个文件", &[("count", count.to_string())])
        };
        let expanded = model.changes_files_expanded;
        let mut card = div()
            .id("environment-card")
            .debug_selector(|| "environment-card".into())
            .flex_none()
            .w_full()
            .p_2()
            .rounded(px(16.))
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.canvas))
            .flex()
            .flex_col()
            .text_size(px(13.))
            .child(
                div()
                    .h(px(36.))
                    .px_2()
                    .mb_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(12.))
                    .text_color(rgb(colors.muted))
                    .child(div().flex_1().child(locale.text("环境")))
                    .child(
                        Button::new("refresh-conversation-changes")
                            .debug_selector(|| "refresh-conversation-changes".into())
                            .ghost()
                            .small()
                            .size(px(26.))
                            .p_0()
                            .icon(IconName::RotateCw)
                            .tooltip(locale.text("刷新变更"))
                            .accessibility_label(locale.text("刷新变更"))
                            .disabled(model.workspace_busy || generating)
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.presenter.review_conversation_changes();
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("close-changes-sidebar")
                            .debug_selector(|| "close-changes-sidebar".into())
                            .ghost()
                            .small()
                            .size(px(26.))
                            .p_0()
                            .icon(IconName::PanelRightClose)
                            .tooltip(locale.text("收起环境面板"))
                            .accessibility_label(locale.text("收起环境面板"))
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.presenter.toggle_changes_sidebar();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                environment_row(
                    "environment-changes",
                    Icon::new(IconName::FileText),
                    locale.text("变更"),
                    colors,
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(colors.muted))
                        .child(summary),
                )
                .when_some(review.filter(|_| has_changes), |row, review| {
                    row.child(
                        div()
                            .flex()
                            .gap_1()
                            .text_size(px(12.))
                            .child(
                                div()
                                    .text_color(rgb(colors.success))
                                    .child(format!("+{}", review.additions)),
                            )
                            .child(
                                div()
                                    .text_color(rgb(colors.danger))
                                    .child(format!("−{}", review.deletions)),
                            ),
                    )
                })
                .child(
                    Icon::new(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(px(13.))
                    .text_color(rgb(colors.muted)),
                )
                .on_click(cx.listener(|app, _, _, cx| {
                    app.presenter.toggle_changes_files();
                    cx.notify();
                })),
            );
        if expanded {
            let mut files = div()
                .id("conversation-changed-files")
                .debug_selector(|| "conversation-changed-files".into())
                .max_h(px(240.))
                .overflow_y_scroll()
                .ml_2()
                .pl_3()
                .mr_2()
                .my_2()
                .border_l_1()
                .border_color(rgb(colors.border))
                .flex()
                .flex_col()
                .gap_1();
            if let Some(review) = review {
                for path in &review.dirty_paths {
                    let name = path.clone();
                    let tooltip = path.clone();
                    files = files.child(
                        div()
                            .id(SharedString::from(format!("change-row-{path}")))
                            .min_w_0()
                            .w_full()
                            .overflow_hidden()
                            .py_1()
                            .text_size(px(12.))
                            .tooltip(move |window, cx| {
                                Tooltip::new(tooltip.clone()).build(window, cx)
                            })
                            .child(
                                Checkbox::new(SharedString::from(format!("sidebar-change-{path}")))
                                    .debug_selector({
                                        let path = path.clone();
                                        move || format!("review-file-{path}")
                                    })
                                    .label(path.clone())
                                    .checked(model.selected_changes.contains(path))
                                    .disabled(model.workspace_busy)
                                    .on_click(cx.listener(move |app, checked, _, cx| {
                                        app.presenter.select_changed_file(name.clone(), *checked);
                                        cx.notify();
                                    })),
                            ),
                    );
                }
                if review.dirty_paths.is_empty() {
                    files = files.child(
                        div()
                            .py_1()
                            .text_size(px(12.))
                            .text_color(rgb(colors.muted))
                            .child(locale.text("没有待提交的变更")),
                    );
                }
                let id = review.workspace_id;
                files = files.child(
                    Button::new("show-workspace-diff")
                        .debug_selector(|| "show-workspace-diff".into())
                        .ghost()
                        .small()
                        .mt_1()
                        .label(locale.text("查看完整差异"))
                        .icon(IconName::ArrowRight)
                        .disabled(model.workspace_busy)
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.open_workspace_review(id, window, cx)
                        })),
                );
            } else {
                files = files.child(
                    div()
                        .py_1()
                        .text_size(px(12.))
                        .text_color(rgb(colors.muted))
                        .child(locale.text(if model.workspace_busy {
                            "正在读取变更…"
                        } else {
                            "刷新以查看项目变更"
                        })),
                );
            }
            card = card.child(files);
        }
        card = card
            .child(
                environment_row(
                    "environment-directory",
                    gpui::svg()
                        .data(match kind {
                            WorkspaceKind::Local => {
                                include_bytes!("../../assets/icons/monitor.svg").as_slice()
                            }
                            WorkspaceKind::Worktree => {
                                include_bytes!("../../assets/icons/git-fork.svg").as_slice()
                            }
                        })
                        .size(px(16.))
                        .flex_none()
                        .text_color(rgb(colors.text_secondary)),
                    locale.text(match kind {
                        WorkspaceKind::Local => "本地",
                        WorkspaceKind::Worktree => "Worktree",
                    }),
                    colors,
                )
                .child(
                    Icon::new(IconName::Folder)
                        .size(px(14.))
                        .text_color(rgb(colors.muted)),
                )
                .child(locale.text("打开方式"))
                .map(|button| {
                    self.working_directory_opener("environment-open-directory", button, cx)
                }),
            )
            .child(
                environment_row(
                    "environment-branch",
                    gpui::svg()
                        .data(include_bytes!("../../assets/icons/git-branch.svg").as_slice())
                        .size(px(16.))
                        .flex_none()
                        .text_color(rgb(colors.text_secondary)),
                    branch.clone(),
                    colors,
                )
                .tooltip(branch_tooltip)
                .child(
                    Icon::new(IconName::Copy)
                        .size(px(13.))
                        .text_color(rgb(colors.muted)),
                )
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(branch.clone()))
                }),
            )
            .child(
                environment_row(
                    "open-commit-editor",
                    Icon::new(IconName::CircleCheck),
                    locale.text(if generating {
                        "正在生成提交说明…"
                    } else {
                        "提交变更…"
                    }),
                    colors,
                )
                .disabled(!has_changes || (busy && !model.commit_editor_open))
                .when(!has_changes || busy, |row| {
                    row.text_color(rgb(colors.muted)).cursor_default()
                })
                .when(has_changes, |row| {
                    row.child(
                        Icon::new(if model.commit_editor_open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size(px(13.))
                        .text_color(rgb(colors.muted)),
                    )
                })
                .on_click(cx.listener(move |app, _, window, cx| {
                    if has_changes && (!busy || app.presenter.model().commit_editor_open) {
                        app.presenter.toggle_commit_editor();
                        app.sync_commit_input(window, cx);
                        if app.presenter.model().commit_editor_open
                            && let Some(input) = app
                                .commit_inputs
                                .get(&app.presenter.model().conversation.id)
                        {
                            input.update(cx, |input, cx| input.focus(window, cx));
                        }
                        cx.notify();
                    }
                })),
            );
        if model.commit_editor_open && has_changes {
            card = card.child(
                div()
                    .id("commit-editor")
                    .debug_selector(|| "commit-editor".into())
                    .mx_2()
                    .mt_2()
                    .pt_2()
                    .pb_2()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(px(12.))
                            .child(div().text_color(rgb(colors.muted)).child(locale.format(
                                "已选择 {selected}/{total} 个文件",
                                &[
                                    ("selected", model.selected_changes.len().to_string()),
                                    ("total", count.to_string()),
                                ],
                            )))
                            .child(
                                Button::new("generate-commit-message")
                                    .debug_selector(|| "generate-commit-message".into())
                                    .ghost()
                                    .small()
                                    .size(px(28.))
                                    .loading(generating)
                                    .when(generating, |button| button.icon(IconName::LoaderCircle))
                                    .when(!generating, |button| {
                                        button.child(
                                            gpui::svg()
                                                .data(
                                                    include_bytes!(
                                                        "../../assets/icons/sparkles.svg"
                                                    )
                                                    .as_slice(),
                                                )
                                                .size(px(16.))
                                                .text_color(rgb(colors.text_secondary)),
                                        )
                                    })
                                    .accessibility_label(locale.text("生成提交说明"))
                                    .tooltip(locale.text(if generating {
                                        "正在生成…"
                                    } else {
                                        "生成提交说明"
                                    }))
                                    .disabled(busy || model.selected_changes.is_empty())
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.presenter.generate_workspace_commit_message();
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when_some(
                        self.commit_inputs.get(&model.conversation.id),
                        |element, input| {
                            element.child(
                                Textarea::new(input)
                                    .bg(rgb(colors.surface))
                                    .border_color(rgb(colors.border))
                                    .disabled(model.workspace_busy),
                            )
                        },
                    )
                    .child(
                        Button::new("commit-workspace-files")
                            .debug_selector(|| "commit-workspace-files".into())
                            .primary()
                            .small()
                            .h(px(32.))
                            .w_full()
                            .label(locale.text("提交所选文件"))
                            .disabled(
                                busy || model.selected_changes.is_empty()
                                    || model.commit_message.trim().is_empty(),
                            )
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.confirm_workspace_commit(window, cx)
                            })),
                    ),
            );
        }
        if running && has_changes {
            card = card.child(
                div()
                    .px_2()
                    .py_2()
                    .text_size(px(11.))
                    .text_color(rgb(colors.muted))
                    .child(locale.text("任务运行结束后可生成说明和提交。")),
            );
        }
        if let Some(status) = &model.changes_status {
            card = card.child(
                div()
                    .debug_selector(|| "environment-status".into())
                    .px_2()
                    .py_2()
                    .text_size(px(12.))
                    .text_color(rgb(colors.muted))
                    .child(status.render(locale).to_owned()),
            );
        }
        div()
            .id("conversation-right-sidebar")
            .debug_selector(|| "conversation-right-sidebar".into())
            .w(px(304.))
            .flex_none()
            .min_h_0()
            .my_2()
            .mr_2()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(card)
    }

    fn confirm_workspace_commit(&self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.presenter.model();
        let Some(review) = model.workspace_review.clone() else {
            return;
        };
        let owner = model.conversation.id;
        let locale = model.language;
        let files = model.selected_changes.iter().cloned().collect::<Vec<_>>();
        let message = model.commit_message.clone();
        let detail = format!(
            "{}\n\n{}\n\n{}",
            review.branch.as_deref().unwrap_or("HEAD"),
            message,
            files.join("\n")
        );
        let answer = window.prompt(
            PromptLevel::Info,
            locale.text("确认将所选文件提交到当前分支？"),
            Some(&detail),
            &[
                PromptButton::ok(locale.text("提交")),
                PromptButton::cancel(locale.text("取消")),
            ],
            cx,
        );
        let app = cx.entity();
        cx.spawn(async move |_, cx| {
            if answer.await.ok() == Some(0) {
                app.update(cx, |app, cx| {
                    if app.presenter.model().conversation.id == owner {
                        app.presenter.commit_workspace_files(review, files, message);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    pub(super) fn render_workspace_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let colors = palette(cx);
        let locale = model.language;
        let draft = &model.workspace_draft;
        let creating = model.selected_task.is_none() && model.selected_workspace.is_none();
        let mut row = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .text_size(px(12.));
        if model.selected_project.is_none() {
            return row;
        }
        if creating {
            let app = cx.entity();
            let button = Button::new("workspace-mode")
                .debug_selector(|| "workspace-mode".into())
                .small()
                .ghost()
                .h(px(COMPACT_CONTROL_HEIGHT))
                .child(
                    gpui::svg()
                        .data(match draft.kind {
                            WorkspaceKind::Local => {
                                include_bytes!("../../assets/icons/monitor.svg").as_slice()
                            }
                            WorkspaceKind::Worktree => {
                                include_bytes!("../../assets/icons/git-fork.svg").as_slice()
                            }
                        })
                        .size(px(14.))
                        .text_color(rgb(colors.text))
                        .flex_none(),
                )
                .child(locale.text(match draft.kind {
                    WorkspaceKind::Local => "本地",
                    WorkspaceKind::Worktree => "Worktree",
                }))
                .accessibility_label(locale.text("工作区模式"))
                .disabled(model.workspace_busy);
            row = row.child(AnimatedDropdown::new(
                "workspace-mode",
                button,
                self.reduced_motion,
                move |menu, _, cx| {
                    let model = app.read(cx).presenter.model();
                    let selected = model.workspace_draft.kind;
                    let busy = model.workspace_busy;
                    let is_git = model.project_is_git;
                    [
                        (WorkspaceKind::Local, "本地"),
                        (WorkspaceKind::Worktree, "Worktree"),
                    ]
                    .into_iter()
                    .fold(menu.min_w(px(140.)), |menu, (kind, label)| {
                        let app = app.clone();
                        menu.item(
                            PopupMenuItem::new(locale.text(label))
                                .checked(selected == kind)
                                .disabled(busy || (kind == WorkspaceKind::Worktree && !is_git))
                                .on_click(move |_, _, cx| {
                                    app.update(cx, |app, cx| {
                                        app.presenter.select_workspace_kind(kind);
                                        cx.notify();
                                    });
                                }),
                        )
                    })
                },
            ));
        } else {
            let workspace = model.selected_workspace.as_ref();
            row = row.child(
                locale.text(match workspace.map(|workspace| workspace.kind) {
                    Some(WorkspaceKind::Worktree) => "Worktree",
                    _ => "本地",
                }),
            );
            if model.selected_task.is_some() {
                let branch = model.workspace_branch.clone().unwrap_or_else(|| "—".into());
                row = row.child(
                    div()
                        .id("workspace-current-branch")
                        .debug_selector(|| "workspace-current-branch".into())
                        .max_w(px(180.))
                        .truncate()
                        .tooltip({
                            let branch = branch.clone();
                            move |window, cx| Tooltip::new(branch.clone()).build(window, cx)
                        })
                        .child(branch),
                );
            }
            if workspace.is_some_and(|workspace| {
                workspace.managed && workspace.status == WorkspaceStatus::Ready
            }) {
                row = row.child(
                    Button::new("workspace-review")
                        .debug_selector(|| "workspace-review".into())
                        .small()
                        .label(locale.text("查看变更"))
                        .disabled(model.workspace_busy)
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.presenter.toggle_changes_sidebar();
                            cx.notify();
                        })),
                );
            }
        }
        if draft.kind == WorkspaceKind::Worktree
            && (creating || (model.workspace_retry && model.selected_task.is_none()))
        {
            let app = cx.entity();
            let button = Button::new("workspace-base")
                .debug_selector(|| "workspace-base".into())
                .small()
                .ghost()
                .h(px(COMPACT_CONTROL_HEIGHT))
                .max_w(px(200.))
                .child(
                    gpui::svg()
                        .data(include_bytes!("../../assets/icons/git-branch.svg").as_slice())
                        .size(px(14.))
                        .text_color(rgb(colors.text))
                        .flex_none(),
                )
                .child(div().min_w_0().truncate().child(if draft.base.is_empty() {
                    locale.text("选择来源分支").to_owned()
                } else {
                    draft.base.clone()
                }))
                .accessibility_label(locale.text("来源分支"))
                .tooltip(locale.format(
                    "从 {branch} 创建 Worktree",
                    &[("branch", draft.base.clone())],
                ))
                .disabled(
                    model.workspace_busy
                        || model
                            .selected_workspace
                            .as_ref()
                            .is_some_and(|workspace| workspace.status != WorkspaceStatus::Missing),
                );
            row = row.child(AnimatedDropdown::new(
                "workspace-base",
                button,
                self.reduced_motion,
                move |menu, _, cx| {
                    let presenter = &app.read(cx).presenter;
                    let selected = presenter.model().workspace_draft.base.clone();
                    let menu = menu.min_w(px(180.)).max_w(px(320.)).scrollable(true);
                    match presenter.workspace_base_branches() {
                        Ok(branches) if !branches.is_empty() => {
                            branches.into_iter().fold(menu, |menu, branch| {
                                let app = app.clone();
                                menu.item(
                                    PopupMenuItem::new(branch.clone())
                                        .checked(branch == selected)
                                        .on_click(move |_, _, cx| {
                                            app.update(cx, |app, cx| {
                                                app.presenter.select_workspace_base(branch.clone());
                                                cx.notify();
                                            });
                                        }),
                                )
                            })
                        }
                        Ok(_) => menu
                            .item(PopupMenuItem::new(locale.text("没有可用的分支")).disabled(true)),
                        Err(_) => menu.item(
                            PopupMenuItem::new(locale.text("无法读取来源分支")).disabled(true),
                        ),
                    }
                },
            ));
        }
        if model.workspace_retry {
            row = row.child(
                Button::new("workspace-retry")
                    .debug_selector(|| "workspace-retry".into())
                    .small()
                    .label(locale.text("重试任务"))
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.presenter.retry_workspace_start();
                        cx.notify();
                    })),
            );
        }
        row
    }

    pub(super) fn render_workspace_hints(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let locale = model.language;
        let creating =
            model.selected_task.is_none() && model.workspace_draft.kind == WorkspaceKind::Worktree;
        let unavailable = model
            .selected_workspace
            .as_ref()
            .is_some_and(|workspace| workspace.status != WorkspaceStatus::Ready);
        let hints = div();
        if model.selected_project.is_none() || creating || !unavailable {
            return hints;
        }
        hints
            .w_full()
            .max_w(px(CONTENT_WIDTH))
            .mx_auto()
            .mb_2()
            .px_3()
            .text_size(px(12.))
            .child(locale.text("目录不可用；历史仍可阅读，请新建任务"))
    }
}
