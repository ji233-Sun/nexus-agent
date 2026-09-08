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
            if draft.kind == WorkspaceKind::Worktree {
                row = row.child(
                    Button::new("worktree-configure")
                        .small()
                        .ghost()
                        .label(format!("{} → {}", draft.base, draft.branch))
                        .disabled(model.workspace_busy)
                        .on_click(cx.listener(Self::configure_workspace_dialog)),
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
            for workspace in model.workspaces.iter().filter(|workspace| workspace.managed) {
                let id = workspace.id;
                let cleanup_app = app.clone();
                let target = target.clone();
                let path = workspace.path.clone();
                let branch = workspace.branch.clone().unwrap_or_default();
                let state = match workspace.status {
                    WorkspaceStatus::Creating => "正在创建",
                    WorkspaceStatus::Ready => "可用",
                    WorkspaceStatus::Missing => "目录缺失或创建未完成",
                    WorkspaceStatus::Removed => "已清理",
                };
                list = list.child(div().flex().flex_col().gap_1()
                    .child(format!("{branch} · {}", locale.text(state)))
                    .child(div().text_size(px(12.)).child(path.clone()))
                    .child(Button::new(SharedString::from(format!("cleanup-{id}"))).small()
                        .label(locale.text("清理目录"))
                        .disabled(model.workspace_busy || model.active_run.is_some() || workspace.status != WorkspaceStatus::Ready)
                        .on_click(move |_, window, cx| {
                            let target = target.read(cx).value().to_string();
                            let answer = window.prompt(PromptLevel::Critical, locale.text("清理此 Worktree 目录？"),
                                Some(&format!("{path}\n{}\n{branch} → {target}", locale.text("保留聊天记录和任务分支；清理后不能继续运行此任务。存在待处理文件或未合入提交时会拒绝清理。"))),
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
}
