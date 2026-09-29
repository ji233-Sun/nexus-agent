use super::*;

impl NexusView {
    pub(super) fn choose_project(
        &mut self,
        _: &gpui::ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let Some(folder) = rfd::AsyncFileDialog::new().pick_folder().await else {
                return;
            };
            let _ = this.update(cx, |view, cx| {
                view.presenter.open_project(folder.path());
                view.presenter.notify_remote_changed();
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn new_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The status item can start a task while the window is hidden.
        #[cfg(target_os = "macos")]
        self.show_window_if_hidden(window, cx);
        self.presenter.new_task();
        self.settings_open = false;
        self.expanded_messages.clear();
        self.focus_prompt(window, cx);
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    pub(super) fn new_projectless_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.presenter.new_projectless_task();
        self.settings_open = false;
        self.expanded_messages.clear();
        self.focus_prompt(window, cx);
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    pub(super) fn select_project(&mut self, project: Project) {
        self.presenter.select_project(project);
        self.expanded_messages.clear();
        self.timeline_scroll.scroll_to_bottom();
        self.presenter.notify_remote_changed();
    }

    pub(super) fn confirm_delete_project(
        &mut self,
        project_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.presenter.can_delete_project(project_id) {
            return;
        }
        let model = self.presenter.model();
        let Some(project) = model
            .projects
            .iter()
            .find(|project| project.id == project_id)
        else {
            return;
        };
        let locale = model.language;
        let message = locale.format(
            "删除项目“{title}”？",
            &[("title", project.display_name.clone())],
        );
        let answer = window.prompt(
            PromptLevel::Critical,
            &message,
            Some(locale.text(
                "此操作会永久删除 Nexus 中该项目的全部对话（含归档）、消息、运行和工作区记录，且无法撤销。磁盘上的项目目录、Worktree、文件和 Git 分支不会删除。",
            )),
            &[
                PromptButton::ok(locale.text("删除项目")),
                PromptButton::cancel(locale.text("取消")),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() == Some(0) {
                let _ = this.update_in(cx, |app, window, cx| {
                    app.delete_project(project_id, window, cx)
                });
            }
        })
        .detach();
    }

    pub(super) fn delete_project(
        &mut self,
        project_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self
            .presenter
            .model()
            .conversation
            .selected_project
            .as_ref()
            .is_some_and(|project| project.id == project_id);
        if self.presenter.delete_project(project_id) {
            self.collapsed_projects.remove(&project_id);
            if selected {
                self.expanded_messages.clear();
                self.timeline_scroll.scroll_to_bottom();
                self.focus_handle.focus(window, cx);
            }
        }
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    pub(super) fn select_task(
        &mut self,
        task_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.presenter.select_task(task_id);
        self.expanded_messages.clear();
        self.timeline_scroll.scroll_to_bottom();
        self.sync_executable(window, cx);
        self.sync_provider_profile_form(
            self.presenter
                .model()
                .selected_provider_profile()
                .map(|profile| profile.id),
            window,
            cx,
        );
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    pub(super) fn archive_task(
        &mut self,
        task_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self.presenter.model().conversation.selected_task == Some(task_id);
        if self.presenter.archive_task(task_id) && selected {
            self.expanded_messages.clear();
            self.timeline_scroll.scroll_to_bottom();
            self.focus_prompt(window, cx);
        }
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    pub(super) fn restore_task(&mut self, task_id: Uuid, cx: &mut Context<Self>) {
        self.presenter.restore_task(task_id);
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    pub(super) fn confirm_delete_task(
        &mut self,
        task_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let locale = self.presenter.model().language;
        let Some(title) = self
            .presenter
            .model()
            .conversation
            .tasks
            .iter()
            .chain(&self.presenter.model().archived_tasks)
            .find(|task| task.id == task_id)
            .map(|task| task.title.clone())
        else {
            return;
        };
        let message = locale.format("永久删除“{title}”？", &[("title", (title).to_string())]);
        let answer = window.prompt(
            PromptLevel::Critical,
            &message,
            Some(locale.text("此操作会删除该对话的全部消息和运行记录，且无法撤销。")),
            &[
                PromptButton::ok(locale.text("永久删除")),
                PromptButton::cancel(locale.text("取消")),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() == Some(0) {
                let _ = this.update_in(cx, |app, window, cx| app.delete_task(task_id, window, cx));
            }
        })
        .detach();
    }

    pub(super) fn delete_task(
        &mut self,
        task_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self.presenter.model().conversation.selected_task == Some(task_id);
        if self.presenter.delete_task(task_id) && selected {
            self.expanded_messages.clear();
            self.timeline_scroll.scroll_to_bottom();
            if self.settings_open {
                self.focus_handle.focus(window, cx);
            } else {
                self.focus_prompt(window, cx);
            }
        }
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    pub(super) fn confirm_delete_archived_tasks(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let locale = self.presenter.model().language;
        let count = self.presenter.model().archived_tasks.len();
        if count == 0 || self.presenter.model().conversation.active_run.is_some() {
            return;
        }
        let message = locale.format(
            "永久删除 {count} 个归档对话？",
            &[("count", (count).to_string())],
        );
        let detail = locale.format(
            "此操作会删除这 {count} 个对话的全部消息和运行记录，且无法撤销。",
            &[("count", (count).to_string())],
        );
        let answer = window.prompt(
            PromptLevel::Critical,
            &message,
            Some(&detail),
            &[
                PromptButton::ok(locale.text("全部删除")),
                PromptButton::cancel(locale.text("取消")),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() == Some(0) {
                let _ = this.update(cx, |app, cx| app.delete_archived_tasks(cx));
            }
        })
        .detach();
    }

    pub(super) fn delete_archived_tasks(&mut self, cx: &mut Context<Self>) {
        self.presenter.delete_archived_tasks();
        self.presenter.notify_remote_changed();
        cx.notify();
    }

    pub(super) fn render_project_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let locale = model.language;
        let colors = palette(cx);
        let query = self.project_search_input.read(cx).value();
        let selected = model
            .conversation
            .selected_project
            .as_ref()
            .map(|project| project.id);
        let projects: Vec<_> = model
            .projects
            .iter()
            .filter(|project| matches_search(&project.display_name, &query))
            .collect();
        div()
            .debug_selector(|| "project-picker-surface".into())
            .w(px(280.))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .debug_selector(|| "project-picker-search".into())
                    .flex_none()
                    .child(
                        Input::new(&self.project_search_input)
                            .small()
                            .min_h(px(CONTROL_HEIGHT))
                            .text_size(px(13.)),
                    ),
            )
            .child(
                div()
                    .id("project-picker-list")
                    .max_h(px(240.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        navigation_row(
                            colors,
                            "project-picker-none",
                            locale.text("未关联项目"),
                            None,
                        )
                        .debug_selector(|| "project-picker-none".into())
                        .selected(selected.is_none())
                        .on_click(cx.listener(
                            move |app, _, window, cx| {
                                app.project_picker_open = false;
                                if selected.is_some() {
                                    app.new_projectless_task(window, cx);
                                }
                                app.focus_prompt(window, cx);
                                cx.notify();
                            },
                        )),
                    )
                    .when(projects.is_empty(), |list| {
                        list.child(
                            div()
                                .p_2()
                                .text_color(rgb(colors.muted))
                                .child(locale.text("没有匹配的项目")),
                        )
                    })
                    .children(projects.into_iter().map(|project| {
                        let project = project.clone();
                        let id = project.id;
                        navigation_row(
                            colors,
                            ElementId::from(id),
                            project.display_name.clone(),
                            None,
                        )
                        .debug_selector(move || format!("project-picker-{id}"))
                        .flex_none()
                        .selected(selected == Some(id))
                        .when(selected == Some(id), |row| {
                            row.suffix(|_, _| Icon::new(IconName::Check).size(px(14.)))
                        })
                        .on_click(cx.listener(
                            move |app, _, window, cx| {
                                app.project_picker_open = false;
                                if selected != Some(id) {
                                    app.select_project(project.clone());
                                }
                                app.focus_prompt(window, cx);
                                cx.notify();
                            },
                        ))
                    })),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .pt_3()
                    .child(
                        navigation_row(
                            colors,
                            "project-picker-new",
                            locale.text("新建项目"),
                            Some(IconName::Plus),
                        )
                        .debug_selector(|| "project-picker-new".into())
                        .on_click(cx.listener(|app, event, window, cx| {
                            app.project_picker_open = false;
                            app.choose_project(event, window, cx);
                            cx.notify();
                        })),
                    ),
            )
    }
}
