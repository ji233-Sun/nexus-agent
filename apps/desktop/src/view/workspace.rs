use super::*;
use crate::model::workspace::{WorkspaceKind, WorkspaceStatus};

impl NexusView {
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
        if model.selected_project.is_none() || model.selected_codex_thread.is_some() {
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
            if let Some(workspace) = workspace
                .filter(|workspace| workspace.managed && workspace.status == WorkspaceStatus::Ready)
            {
                let id = workspace.id;
                row = row.child(
                    Button::new("workspace-review")
                        .debug_selector(|| "workspace-review".into())
                        .small()
                        .label(locale.text("查看变更"))
                        .disabled(model.workspace_busy)
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.open_workspace_review(id, window, cx)
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
        if model.selected_project.is_none()
            || model.selected_codex_thread.is_some()
            || creating
            || !unavailable
        {
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

    pub(super) fn open_workspace_review(
        &mut self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.presenter.review_workspace(id) {
            return;
        }
        let target_default = self
            .presenter
            .model()
            .selected_project
            .as_ref()
            .and_then(|project| {
                crate::infrastructure::git::current_branch(Path::new(&project.canonical_path))
            })
            .unwrap_or_default();
        let target = cx.new(|cx| InputState::new(window, cx).default_value(target_default));
        let message = cx.new(|cx| InputState::new(window, cx).default_value("完成任务成果"));
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let model = app.read(cx).presenter.model();
            let locale = model.language;
            let managed = model.workspaces.iter().any(|workspace| workspace.id == id && workspace.managed);
            let mut content = div().id("workspace-review-content").debug_selector(|| "workspace-review-content".into()).max_h(window.viewport_size().height * 0.56).overflow_y_scroll().flex().flex_col().gap_3();
            let refresh_app = app.clone();
            let mut actions = div().flex().flex_wrap().gap_2().child(Button::new("refresh-workspace-review").small().label(locale.text("刷新变更"))
                .disabled(model.workspace_busy).on_click(move |_, _, cx| { refresh_app.update(cx, |app, cx| { app.presenter.review_workspace(id); cx.notify(); }); }));
            if let Some(review) = model.workspace_review.as_ref().filter(|review| review.workspace_id == id) {
                content = content.child(format!("{} · {}", review.branch.as_deref().unwrap_or("HEAD"), review.head));
                for path in &review.dirty_paths {
                    let change_app = app.clone();
                    let name = path.clone();
                    content = content.child(Checkbox::new(SharedString::from(format!("change-{path}"))).debug_selector({ let path = path.clone(); move || format!("review-file-{path}") }).label(path.clone()).checked(model.selected_changes.contains(path))
                        .disabled(model.workspace_busy || !managed).on_click(move |checked, _, cx| { change_app.update(cx, |app, cx| { app.presenter.select_changed_file(name.clone(), *checked); cx.notify(); }); }));
                }
                for (label, diff) in [("相对创建基准的已提交变更", &review.committed), ("暂存变更", &review.staged), ("未暂存变更", &review.unstaged)] {
                    content = content.child(diff_block(locale.text(label), diff));
                }
                for (name, text) in &review.untracked { content = content.child(diff_block(&format!("{} · {name}", locale.text("未跟踪文件")), text)); }
                let commit_app = app.clone();
                let message_input = message.clone();
                let commit_review = review.clone();
                let files = model.selected_changes.iter().cloned().collect::<Vec<_>>();
                content = content.child(locale.text("提交说明")).child(Input::new(&message));
                actions = actions.child(Button::new("commit-workspace-files").debug_selector(|| "commit-workspace-files".into()).small().label(locale.text("提交所选文件"))
                    .disabled(model.workspace_busy || !managed || files.is_empty())
                    .on_click(move |_, window, cx| {
                        let description = message_input.read(cx).value().to_string();
                        let detail = format!("{}\n\n{}", description, files.join("\n"));
                        let answer = window.prompt(PromptLevel::Info, locale.text("确认将所选文件提交到任务分支？"), Some(&detail),
                            &[PromptButton::ok(locale.text("提交")), PromptButton::cancel(locale.text("取消"))], cx);
                        let app = commit_app.clone(); let files = files.clone(); let review = commit_review.clone();
                        cx.spawn(async move |cx| { if answer.await.ok() == Some(0) { app.update(cx, |app, cx| { app.presenter.commit_workspace_files(review, files, description); cx.notify(); }); } }).detach();
                    }));
                if let Some(workspace) = model.workspaces.iter().find(|workspace| workspace.id == id)
                    && let Some(state) = &workspace.merge {
                    content = content.child(format!("{} → {}\n{}", review.branch.as_deref().unwrap_or_default(), state.target_branch, state.target_path))
                        .child(format!("{}: {}", locale.text("冲突文件"), review.conflicts.join(", ")))
                        .child(locale.text("请在目标目录编辑冲突文件并 git add，再刷新此处继续；也可以中止合并。"))
                        .child(diff_block(locale.text("当前冲突解决内容"), &review.resolution_diff))
                        .child(diff_block(locale.text("已暂存的冲突解决内容"), &review.resolution_staged));
                    for (abort, label) in [(false, "继续合并"), (true, "中止合并")] {
                        let finish_app = app.clone(); let review = review.clone();
                        actions = actions.child(Button::new(label).small().label(locale.text(label)).disabled(model.workspace_busy)
                            .on_click(move |_, window, cx| {
                                let answer = window.prompt(PromptLevel::Critical, locale.text(label), Some(locale.text("继续会提交当前冲突解决内容；中止会恢复到本次合并开始前的状态。")),
                                    &[PromptButton::ok(locale.text(label)), PromptButton::cancel(locale.text("取消"))], cx);
                                let app = finish_app.clone(); let review = review.clone();
                                cx.spawn(async move |cx| { if answer.await.ok() == Some(0) { app.update(cx, |app, cx| { app.presenter.finish_workspace_merge(review, abort); cx.notify(); }); } }).detach();
                            }));
                    }
                } else {
                    let preview_app = app.clone(); let target_input = target.clone();
                    content = content.child(locale.text("合入本地目标分支")).child(Input::new(&target))
                        .child(review.target_branches.join(" · "));
                    actions = actions.child(Button::new("preview-workspace-merge").small().label(locale.text("预览合入"))
                        .disabled(model.workspace_busy || !managed || !review.dirty_paths.is_empty())
                        .on_click(move |_, _, cx| { let branch = target_input.read(cx).value().to_string(); preview_app.update(cx, |app, cx| { app.presenter.preview_workspace_merge(id, branch); cx.notify(); }); }));
                }
            }
            if let Some(plan) = model.merge_plan.as_ref().filter(|plan| plan.workspace_id == id) {
                content = content.child(format!("{} → {}\n{}", plan.source_branch, plan.state.target_branch, plan.state.target_path))
                    .child(diff_block(locale.text("合入预览"), &plan.diff));
                let merge_app = app.clone(); let plan = plan.clone();
                actions = actions.child(Button::new("confirm-workspace-merge").primary().small().label(locale.text("确认合入本地分支"))
                    .disabled(model.workspace_busy)
                    .on_click(move |_, _, cx| { let plan = plan.clone(); merge_app.update(cx, |app, cx| { app.presenter.merge_workspace(plan); cx.notify(); }); }));
            }
            dialog.title(locale.text("任务变更与成果接收")).width(px(900.).min(window.viewport_size().width - px(48.)))
                .child(content).child(div().text_size(px(12.)).child(model.status_text().to_owned())).footer(actions)
        });
        cx.notify();
    }
}

fn diff_block(title: &str, content: &str) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(title.to_owned())
        .child(
            div()
                .text_size(px(12.))
                .font_family("monospace")
                .whitespace_normal()
                .child(if content.is_empty() {
                    "—".to_owned()
                } else {
                    content.to_owned()
                }),
        )
}
