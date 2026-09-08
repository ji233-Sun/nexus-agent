use super::*;
use crate::model::workspace::{WorkspaceKind, WorkspaceStatus};

impl NexusView {
    pub(super) fn render_workspace_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let locale = model.language;
        let draft = &model.workspace_draft;
        let creating = model.selected_task.is_none() && model.selected_workspace.is_none();
        let history = model.selected_codex_thread.is_some();
        let mut controls = div()
            .w_full()
            .max_w(px(CONTENT_WIDTH))
            .mx_auto()
            .mb_2()
            .flex()
            .flex_col()
            .gap_2()
            .text_size(px(12.));
        if model.selected_project.is_none() || history {
            return controls;
        }
        let mut row = div().flex().items_center().flex_wrap().gap_2();
        if creating {
            for (kind, label) in [
                (WorkspaceKind::Local, "本地"),
                (WorkspaceKind::Worktree, "Worktree"),
            ] {
                row = row.child(
                    Button::new(label)
                        .small()
                        .ghost()
                        .label(locale.text(label))
                        .selected(draft.kind == kind)
                        .disabled(
                            model.workspace_busy
                                || (kind == WorkspaceKind::Worktree && !model.project_is_git),
                        )
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.presenter.select_workspace_kind(kind);
                            cx.notify();
                        })),
                );
            }
        } else {
            let workspace = model.selected_workspace.as_ref();
            row = row.child(match workspace.map(|workspace| workspace.kind) {
                Some(WorkspaceKind::Worktree) => "Worktree",
                _ => locale.text("本地"),
            });
            row = row.child(model.workspace_branch.clone().unwrap_or_else(|| "—".into()));
            if workspace.is_some_and(|workspace| workspace.status != WorkspaceStatus::Ready) {
                row = row.child(locale.text("目录不可用；历史仍可阅读，请新建任务"));
            }
            if let Some(workspace) = workspace
                .filter(|workspace| workspace.managed && workspace.status == WorkspaceStatus::Ready)
            {
                let id = workspace.id;
                row = row.child(
                    Button::new("workspace-review")
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
            row = row.child(
                Button::new("worktree-configure")
                    .small()
                    .ghost()
                    .label(format!("{} → {}", draft.base, draft.branch))
                    .disabled(
                        model.workspace_busy
                            || model.selected_workspace.as_ref().is_some_and(|workspace| {
                                workspace.status != WorkspaceStatus::Missing
                            }),
                    )
                    .on_click(cx.listener(Self::configure_workspace_dialog)),
            );
        }
        if let Some(path) = model.working_directory().map(str::to_owned) {
            row = row.child(
                Button::new("workspace-copy-path")
                    .small()
                    .ghost()
                    .label(locale.text("复制目录"))
                    .tooltip(path.clone())
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(path.clone()))
                    }),
            );
        }
        row = row.child(
            Button::new("workspace-manager")
                .small()
                .ghost()
                .label(locale.text("管理 Worktree"))
                .on_click(cx.listener(Self::open_workspace_manager)),
        );
        if model.workspace_retry {
            row = row.child(
                Button::new("workspace-retry")
                    .small()
                    .label(locale.text("重试任务"))
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.presenter.retry_workspace_start();
                        cx.notify();
                    })),
            );
        }
        controls = controls.child(row);
        if creating && draft.kind == WorkspaceKind::Worktree {
            controls =
                controls.child(locale.text("首次发送时创建目录；依赖和 .env 不会自动复制。"));
            if model.project_dirty {
                controls = controls.child(
                    div()
                        .text_color(rgb(palette(cx).warning))
                        .child(locale.text("当前目录有未提交修改，这些修改不会带入新任务。")),
                );
            }
        }
        controls
    }

    fn configure_workspace_dialog(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let draft = &self.presenter.model().workspace_draft;
        let base = cx.new(|cx| InputState::new(window, cx).default_value(draft.base.clone()));
        let branch = cx.new(|cx| InputState::new(window, cx).default_value(draft.branch.clone()));
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let locale = app.read(cx).presenter.model().language;
            let save_app = app.clone();
            let base_input = base.clone();
            let branch_input = branch.clone();
            dialog
                .title(locale.text("创建 Worktree"))
                .child(locale.text("创建基准（分支、标签或提交，默认 HEAD）"))
                .child(Input::new(&base))
                .child(locale.text("任务分支（必须是尚不存在的新分支）"))
                .child(Input::new(&branch))
                .footer(
                    Button::new("workspace-config-save")
                        .primary()
                        .label(locale.text("保存"))
                        .on_click(move |_, window, cx| {
                            let base = base_input.read(cx).value().to_string();
                            let branch = branch_input.read(cx).value().to_string();
                            save_app.update(cx, |app, cx| {
                                app.presenter.configure_workspace(base, branch);
                                cx.notify();
                            });
                            window.close_dialog(cx);
                        }),
                )
        });
    }

    fn open_workspace_manager(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.presenter.reload_workspaces();
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
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let model = app.read(cx).presenter.model();
            let locale = model.language;
            let mut list = div().id("workspace-list").max_h(window.viewport_size().height * 0.5)
                .overflow_y_scroll().flex().flex_col().gap_3();
            for workspace in model.workspaces.iter().filter(|workspace| workspace.managed || (workspace.external && workspace.task_id.is_some())) {
                let id = workspace.id;
                let cleanup_app = app.clone();
                let target = target.clone();
                let path = workspace.path.clone();
                let branch = workspace.branch.clone().unwrap_or_default();
                let review_app = app.clone();
                let init_app = app.clone();
                let cancel_app = app.clone();
                let restore_app = app.clone();
                let owned = workspace.managed;
                let state = match workspace.status {
                    WorkspaceStatus::Creating => "正在创建",
                    WorkspaceStatus::Ready => "可用",
                    WorkspaceStatus::Missing => "目录缺失或创建未完成",
                    WorkspaceStatus::Removed => "已清理",
                };
                list = list.child(div().flex().flex_col().gap_1()
                    .child(format!("{branch} · {}", locale.text(state)))
                    .child(div().text_size(px(12.)).child(path.clone()))
                    .child(div().flex().gap_2()
                        .child(Button::new(SharedString::from(format!("review-{id}"))).small().label(locale.text("查看变更"))
                            .disabled(model.workspace_busy || workspace.status != WorkspaceStatus::Ready)
                            .on_click(move |_, window, cx| { window.close_dialog(cx); review_app.update(cx, |app, cx| app.open_workspace_review(id, window, cx)); }))
                        .child(Button::new(SharedString::from(format!("init-{id}"))).small().label(locale.text("初始化 / 重试"))
                            .disabled(model.workspace_busy || workspace.status != WorkspaceStatus::Ready || workspace.task_id.is_some_and(|task| model.task_running(task)))
                            .on_click(move |_, window, cx| { window.close_dialog(cx); init_app.update(cx, |app, cx| app.open_workspace_initialization(id, window, cx)); })))
                    .when_some(workspace.initialization.clone(), |element, log| {
                        element.child(div().flex().flex_col().gap_1()
                            .child(format!("$ {}", log.command))
                            .child(if log.running { locale.text("正在初始化") } else if log.success { locale.text("初始化成功") } else { locale.text("初始化失败或中断") })
                            .child(div().id(SharedString::from(format!("init-log-{id}"))).max_h(px(160.)).overflow_y_scroll().text_size(px(12.)).child(log.output))
                            .when(log.running, |element| element.child(Button::new(SharedString::from(format!("cancel-init-{id}"))).small().label(locale.text("停止初始化"))
                                .on_click(move |_, _, cx| { cancel_app.update(cx, |app, cx| { app.presenter.cancel_workspace_initialization(); cx.notify(); }); }))))
                    })
                    .when(owned && workspace.status == WorkspaceStatus::Missing, |element| element.child(Button::new(SharedString::from(format!("restore-workspace-{id}"))).small().label(locale.text("恢复缺失目录"))
                        .disabled(model.workspace_busy).on_click(move |_, _, cx| { restore_app.update(cx, |app, cx| { app.presenter.restore_workspace_directory(id); cx.notify(); }); })))
                    .child(Button::new(SharedString::from(format!("cleanup-{id}"))).small()
                        .label(locale.text(if owned { "清理目录" } else { "解除关联" }))
                        .disabled(model.workspace_busy || workspace.task_id.is_some_and(|task| model.task_running(task)) || !matches!(workspace.status, WorkspaceStatus::Ready | WorkspaceStatus::Missing))
                        .on_click(move |_, window, cx| {
                            let target = target.read(cx).value().to_string();
                            let answer = window.prompt(PromptLevel::Critical, locale.text(if owned { "清理此 Worktree 目录？" } else { "解除此外部目录与任务的关联？" }),
                                Some(&format!("{path}\n{}\n{branch} → {target}", locale.text(if owned { "保留聊天记录和任务分支；清理后不能继续运行此任务。存在待处理文件或未合入提交时会拒绝清理。" } else { "仅解除关联，保留外部目录的全部文件和分支。此任务历史仍可阅读。" }))),
                                &[PromptButton::ok(locale.text("清理目录")), PromptButton::cancel(locale.text("取消"))], cx);
                            let app = cleanup_app.clone();
                            cx.spawn(async move |cx| {
                                if answer.await.ok() == Some(0) {
                                    app.update(cx, |app, cx| { app.presenter.cleanup_workspace(id, target); cx.notify(); });
                                }
                            }).detach();
                        })));
            }
            dialog.title(locale.text("管理 Worktree"))
                .width(px(720.).min(window.viewport_size().width - px(48.)))
                .child(locale.text("检查成果已合入的本地目标分支"))
                .child(Input::new(&target))
                .child(list)
                .child(div().text_size(px(12.)).child(model.status_text().to_owned()))
        });
    }

    fn open_workspace_initialization(
        &mut self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self
            .presenter
            .model()
            .workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .cloned()
        else {
            return;
        };
        let command = cx.new(|cx| {
            InputState::new(window, cx).default_value(
                workspace
                    .initialization
                    .as_ref()
                    .map(|log| log.command.clone())
                    .unwrap_or_default(),
            )
        });
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let locale = app.read(cx).presenter.model().language;
            let command_input = command.clone();
            let run_app = app.clone();
            dialog
                .title(locale.text("项目初始化"))
                .child(workspace.path.clone())
                .child(locale.text("在此任务目录执行以下命令；依赖和 .env 不会自动复制。"))
                .child(Input::new(&command))
                .footer(
                    Button::new("run-workspace-init")
                        .primary()
                        .label(locale.text("执行初始化命令"))
                        .on_click(move |_, window, cx| {
                            let script = command_input.read(cx).value().to_string();
                            run_app.update(cx, |app, cx| {
                                if app.presenter.initialize_workspace(id, script) {
                                    window.close_dialog(cx);
                                    app.open_workspace_manager(
                                        &gpui::ClickEvent::default(),
                                        window,
                                        cx,
                                    );
                                }
                                cx.notify();
                            });
                        }),
                )
        });
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
