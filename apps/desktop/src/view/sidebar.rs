use super::*;
use gpui_kit::component::{list::ListItem, scroll::Scrollbar, spinner::Spinner};

const SIDEBAR_ROW_HEIGHT: f32 = 40.;

#[derive(Clone)]
struct ProjectDrag {
    id: Uuid,
    index: usize,
    name: SharedString,
}

impl Render for ProjectDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = palette(cx);
        div()
            .w(px(SIDEBAR_WIDTH - 28.))
            .h(px(SIDEBAR_ROW_HEIGHT))
            .px(px(10.))
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(CONTROL_RADIUS))
            .bg(rgb(colors.elevated))
            .border_1()
            .border_color(rgb(colors.accent))
            .text_color(rgb(colors.text))
            .text_size(px(13.))
            .shadow_md()
            .child(
                Icon::new(IconName::Folder)
                    .size(px(16.))
                    .text_color(rgb(colors.muted)),
            )
            .child(div().flex_1().min_w_0().truncate().child(self.name.clone()))
    }
}

pub(super) fn navigation_row(
    colors: Palette,
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
    icon: Option<IconName>,
) -> ListItem {
    let title = title.into();
    let tooltip = title.clone();
    ListItem::new(id)
        .w_full()
        .h(px(SIDEBAR_ROW_HEIGHT))
        .px(px(10.))
        .pr(px(32.))
        .py_0()
        .rounded(px(CONTROL_RADIUS))
        .text_size(px(13.))
        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        .child(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .when_some(icon, |row, icon| {
                    row.child(Icon::new(icon).size(px(16.)).text_color(rgb(colors.muted)))
                })
                .child(div().flex_1().min_w_0().truncate().child(title)),
        )
}

impl NexusView {
    fn render_task_row(
        &self,
        task: &nexus_domain::TaskSummary,
        cx: &mut Context<Self>,
    ) -> ListItem {
        let model = self.presenter.model();
        let locale = model.language;
        let colors = palette(cx);
        let id = task.id;
        let app = cx.entity().clone();
        let can_manage = !model.task_running(task.id) && !model.workspace_busy;
        let reduced_motion = self.reduced_motion;
        let color = run_status_color(colors, task.status);
        let active = task.status.is_active();
        navigation_row(colors, id, task.title.clone(), None)
            .pr(px(62.))
            .group("sidebar-task")
            .debug_selector(move || format!("sidebar-task-{id}"))
            .selected(
                model.conversation.selected_task == Some(id) && model.opened_issues().is_none(),
            )
            .suffix(move |_, _| {
                let archive_app = app.clone();
                let delete_app = app.clone();
                div()
                    .absolute()
                    .right(px(2.))
                    .top(px((SIDEBAR_ROW_HEIGHT - COMPACT_CONTROL_HEIGHT) / 2.))
                    .h(px(COMPACT_CONTROL_HEIGHT))
                    .flex()
                    .items_center()
                    .gap_1()
                    .when_some(color, |actions, color| {
                        actions.child(if active {
                            Spinner::new()
                                .icon(IconName::LoaderCircle)
                                .with_size(px(14.))
                                .color(color)
                                .into_any_element()
                        } else {
                            Icon::new(IconName::CircleX)
                                .size(px(14.))
                                .text_color(color)
                                .into_any_element()
                        })
                    })
                    .child(
                        AnimatedDropdown::new(
                            (ElementId::from(id), "actions-menu"),
                            Button::new((ElementId::from(id), "actions-trigger"))
                                .debug_selector(move || format!("task-actions-{id}"))
                                .ghost()
                                .small()
                                .size(px(COMPACT_CONTROL_HEIGHT))
                                .p_0()
                                .icon(IconName::Ellipsis)
                                .accessibility_label(locale.text("对话操作"))
                                .tooltip(locale.text("对话操作"))
                                .disabled(!can_manage),
                            reduced_motion,
                            move |menu, _, _| {
                                let archive_app = archive_app.clone();
                                let delete_app = delete_app.clone();
                                menu.min_w(px(144.))
                                    .item(
                                        PopupMenuItem::new(locale.text("归档此对话"))
                                            .icon(IconName::Inbox)
                                            .on_click(move |_, window, cx| {
                                                archive_app.update(cx, |app, cx| {
                                                    app.archive_task(id, window, cx)
                                                });
                                            }),
                                    )
                                    .item(PopupMenuItem::separator())
                                    .item(
                                        PopupMenuItem::new(locale.text("删除对话"))
                                            .icon(IconName::Delete)
                                            .on_click(move |_, window, cx| {
                                                delete_app.update(cx, |app, cx| {
                                                    app.confirm_delete_task(id, window, cx)
                                                });
                                            }),
                                    )
                            },
                        )
                        .show_caret(false),
                    )
            })
            .on_click(cx.listener(move |app, _, window, cx| app.select_task(id, window, cx)))
    }

    pub(super) fn render_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let model = self.presenter.model();
        let query = self.search_input.read(cx).value();
        let selected_project_id = model
            .conversation
            .selected_project
            .as_ref()
            .map(|project| project.id);
        let project_rows = model.projects.iter().enumerate();
        let projects = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .children(project_rows.map(|(index, project)| {
                let selected = selected_project_id == Some(project.id);
                let open = selected && !self.collapsed_projects.contains(&project.id);
                let progress = disclosure_progress(
                    (ElementId::from(project.id), "project-reveal"),
                    open,
                    self.reduced_motion || !selected,
                    window,
                    cx,
                );
                let project_id = project.id;
                let project = project.clone();
                let new_task_project = project.clone();
                let can_delete = self.presenter.can_delete_project(project_id);
                let reduced_motion = self.reduced_motion;
                let tasks: Vec<_> = model
                    .conversation
                    .tasks
                    .iter()
                    .filter(|task| task.project_id == Some(project_id))
                    .filter(|task| matches_search(&task.title, &query))
                    .map(|task| self.render_task_row(task, cx))
                    .collect();
                gpui_kit::base::Collapsible::new()
                    .open(open)
                    .reveal((ElementId::from(project_id), "project-content"), progress)
                    .flex()
                    .flex_col()
                    .child(
                        navigation_row(
                            colors,
                            project_id,
                            project.display_name.clone(),
                            Some(IconName::Folder),
                        )
                        .group("sidebar-project")
                        .pr(px(70.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .debug_selector(move || format!("sidebar-project-{project_id}"))
                        .cursor_move()
                        .on_drag(
                            ProjectDrag {
                                id: project_id,
                                index,
                                name: project.display_name.clone().into(),
                            },
                            |drag, _, _, cx| cx.new(|_| drag.clone()),
                        )
                        .can_drop(move |value, _, _| {
                            value
                                .downcast_ref::<ProjectDrag>()
                                .is_some_and(|drag| drag.id != project_id)
                        })
                        .drag_over::<ProjectDrag>(move |style, drag, _, _| {
                            let style = style
                                .bg(rgb(colors.selected))
                                .border_color(rgb(colors.accent));
                            if drag.index > index {
                                style.border_t_2()
                            } else {
                                style.border_b_2()
                            }
                        })
                        .on_drop(cx.listener(move |app, drag: &ProjectDrag, _, cx| {
                            app.presenter.reorder_project(drag.id, project_id);
                            cx.notify();
                        }))
                        .on_click(cx.listener(move |app, _, _, cx| {
                            if !selected {
                                app.select_project(project.clone());
                                app.collapsed_projects.remove(&project_id);
                            } else if !app.collapsed_projects.remove(&project_id) {
                                app.collapsed_projects.insert(project_id);
                            }
                            cx.notify();
                        }))
                        .suffix({
                            let app = cx.entity();
                            move |_, _| {
                                let app = app.clone();
                                let delete_app = app.clone();
                                let project = new_task_project.clone();
                                div()
                                    .absolute()
                                    .right(px(2.))
                                    .top(px((SIDEBAR_ROW_HEIGHT - COMPACT_CONTROL_HEIGHT) / 2.))
                                    .flex()
                                    .items_center()
                                    .invisible()
                                    .group_hover("sidebar-project", |style| style.visible())
                                    .child(
                                        Button::new((ElementId::from(project_id), "new-task"))
                                            .debug_selector(move || {
                                                format!("project-new-task-{project_id}")
                                            })
                                            .ghost()
                                            .small()
                                            .size(px(COMPACT_CONTROL_HEIGHT))
                                            .p_0()
                                            .child(
                                                gpui::svg()
                                                    .data(
                                                        include_bytes!(
                                                            "../../assets/icons/new-chat.svg"
                                                        )
                                                        .as_slice(),
                                                    )
                                                    .size(px(16.))
                                                    .flex_none()
                                                    .text_color(rgb(colors.text)),
                                            )
                                            .accessibility_label(locale.text("在此项目中新建对话"))
                                            .tooltip(locale.text("新建对话"))
                                            .on_click(move |_, window, cx| {
                                                cx.stop_propagation();
                                                app.update(cx, |app, cx| {
                                                    if !selected {
                                                        app.select_project(project.clone());
                                                    }
                                                    app.collapsed_projects.remove(&project_id);
                                                    app.new_task(window, cx);
                                                });
                                            }),
                                    )
                                    .child(
                                        AnimatedDropdown::new(
                                            (ElementId::from(project_id), "actions-menu"),
                                            Button::new((ElementId::from(project_id), "actions"))
                                                .debug_selector(move || {
                                                    format!("project-actions-{project_id}")
                                                })
                                                .ghost()
                                                .small()
                                                .size(px(COMPACT_CONTROL_HEIGHT))
                                                .p_0()
                                                .icon(IconName::Ellipsis)
                                                .accessibility_label(locale.text("项目操作"))
                                                .tooltip(locale.text("项目操作"))
                                                .disabled(!can_delete),
                                            reduced_motion,
                                            move |menu, _, _| {
                                                let delete_app = delete_app.clone();
                                                menu.min_w(px(144.)).item(
                                                    PopupMenuItem::new(locale.text("删除项目"))
                                                        .icon(IconName::Delete)
                                                        .on_click(move |_, window, cx| {
                                                            delete_app.update(cx, |app, cx| {
                                                                app.confirm_delete_project(
                                                                    project_id, window, cx,
                                                                )
                                                            });
                                                        }),
                                                )
                                            },
                                        )
                                        .show_caret(false),
                                    )
                            }
                        }),
                    )
                    .content(
                        div()
                            .pt(px(4.))
                            .opacity(progress)
                            .w_full()
                            .pl(px(24.))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .children(
                                IssueProvider::ALL
                                    .into_iter()
                                    .filter(|provider| {
                                        selected
                                            && model.issues(*provider).enabled
                                            && model.issues(*provider).repository.is_some()
                                    })
                                    .map(|provider| {
                                        div().relative().ml(-px(24.)).child(
                                            navigation_row(
                                                colors,
                                                (ElementId::from(project_id), provider.key()),
                                                provider.name(),
                                                None,
                                            )
                                            .pl(px(34.))
                                            .suffix(move |_, _| {
                                                div()
                                                    .debug_selector(move || {
                                                        format!("sidebar-{}-icon", provider.key())
                                                    })
                                                    .absolute()
                                                    .left(px(10.))
                                                    .top(px(12.))
                                                    .child(issues::provider_icon(
                                                        provider,
                                                        16.,
                                                        colors.accent,
                                                    ))
                                            })
                                            .debug_selector(move || {
                                                format!("sidebar-{}", provider.key())
                                            })
                                            .selected(model.issues(provider).opened)
                                            .on_click(
                                                cx.listener(move |app, _, window, cx| {
                                                    app.presenter.open_issues(provider);
                                                    app.focus_handle.focus(window, cx);
                                                    cx.notify();
                                                }),
                                            ),
                                        )
                                    }),
                            )
                            .when(tasks.is_empty(), |list| {
                                list.child(
                                    navigation_row(
                                        colors,
                                        (ElementId::from(project_id), "empty"),
                                        if query.trim().is_empty() {
                                            locale.text("开始任务后，记录会出现在这里")
                                        } else {
                                            locale.text("没有匹配的任务")
                                        },
                                        None,
                                    )
                                    .disabled(true),
                                )
                            })
                            .children(tasks),
                    )
            }));
        div()
            .id("workspace-sidebar")
            .debug_selector(|| "workspace-sidebar".into())
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .pt(px(if cfg!(target_os = "macos") { 36. } else { 0. }))
            .flex()
            .flex_col()
            .child(
                div().flex_none().px(px(14.)).pt(px(8.)).child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .pb_6()
                        .child(
                            div()
                                .px_1()
                                .pt_2()
                                .pb_4()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_size(px(15.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(brand_mark(32.))
                                .child("Nexus Agent"),
                        )
                        .child(
                            Button::new("new-task")
                                .outline()
                                .small()
                                .w_full()
                                .h(px(38.))
                                .bg(rgb(colors.elevated))
                                .border_color(rgb(colors.border))
                                .text_size(px(13.))
                                .accessibility_label(locale.text("新建任务"))
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .w_full()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(Icon::new(IconName::Plus).text_color(rgb(colors.accent)))
                                                .font_weight(gpui::FontWeight::MEDIUM)
                                                .child(locale.text("新建任务")),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(rgb(colors.muted))
                                                .rounded(px(5.))
                                                .bg(rgb(colors.surface))
                                                .px(px(5.))
                                                .py(px(2.))
                                                .child(if cfg!(target_os = "macos") {
                                                    "⌘ N"
                                                } else {
                                                    "Ctrl N"
                                                }),
                                        ),
                                )
                                .tooltip(locale.text("新建任务"))
                                .on_click(
                                    cx.listener(|app, _, window, cx| app.new_task(window, cx)),
                                ),
                        )
                        .child(
                            div()
                                .rounded(px(CONTROL_RADIUS))
                                .bg(rgb(colors.recessed))
                                .px_2()
                                .child(
                                    Input::new(&self.search_input)
                                        .small()
                                        .appearance(false)
                                        .bordered(false)
                                        .min_h(px(SIDEBAR_ROW_HEIGHT))
                                        .text_size(px(12.))
                                        .prefix(Icon::new(IconName::Search).size(px(14.)).text_color(rgb(colors.muted)))
                                        .cleanable(true),
                                ),
                        ),
                ),
            )
            .child(
                div()
                    .id("sidebar-navigation")
                    .size_full()
                    .overflow_y_scroll()
                    .lock_scroll_axis()
                    .track_scroll(&self.sidebar_scroll)
                    .child(
                        div()
                            .px(px(14.))
                            .pb(px(16.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .px(px(10.))
                                    .pb(px(10.))
                                    .text_size(px(11.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(rgb(colors.muted))
                                    .child(locale.text("项目空间")),
                            )
                            .child(
                                div()
                                    .mb_3()
                                    .child(
                                        navigation_row(colors, "projectless-tasks", locale.text("未关联项目"), Some(IconName::Plus))
                                            .debug_selector(|| "sidebar-projectless".into())
                                            .selected(selected_project_id.is_none() && model.conversation.selected_task.is_none())
                                            .on_click(cx.listener(|app, _, window, cx| {
                                                app.new_projectless_task(window, cx);
                                            })),
                                    )
                                    .children(model.projectless_tasks.iter()
                                        .filter(|task| matches_search(&task.title, &query))
                                        .map(|task| self.render_task_row(task, cx))),
                            )
                            .child(projects)
                            .child(
                                navigation_row(
                                    colors,
                                    "add-project",
                                    locale.text("添加本地项目"),
                                    Some(IconName::Plus),
                                )
                                .debug_selector(|| "add-project".into())
                                .mt(px(8.))
                                .text_color(rgb(colors.muted))
                                .on_click(cx.listener(Self::choose_project)),
                            ),
                    )
                    .map(|navigation| {
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .child(navigation)
                            // Keep the scrollbar outside the content measured by the scroll handle.
                            .child(Scrollbar::vertical(&self.sidebar_scroll))
                    }),
            )
            .child(
                div().flex_none().px(px(14.)).pb(px(16.)).child(
                    div()
                        .w_full()
                        .pt_3()
                        .border_t(px(0.5))
                        .border_color(rgb(colors.border))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            Button::new("sidebar-settings")
                                .debug_selector(|| "sidebar-settings".into())
                                .ghost()
                                .small()
                                .w_full()
                                .h(px(SIDEBAR_ROW_HEIGHT))
                                .text_size(px(13.))
                                .accessibility_label(locale.text("设置"))
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .w_full()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(Icon::new(IconName::Settings2))
                                                .child(locale.text("设置"))
                                                .when(model.updates.state.package().is_some(), |element| {
                                                    element.child(div()
                                                        .debug_selector(|| "update-ready-badge".into())
                                                        .text_size(px(11.)).text_color(rgb(colors.accent))
                                                        .child(locale.text(if matches!(model.updates.state, crate::model::updates::UpdateState::Available(_)) {
                                                            "发现新版本"
                                                        } else if model.updates.state.is_installing() {
                                                            "正在安装更新"
                                                        } else if matches!(model.updates.state, crate::model::updates::UpdateState::Ready { .. }) {
                                                            "等待安装"
                                                        } else {
                                                            "正在下载更新"
                                                        })))
                                                }),
                                        )
                                        .child(status_dot(
                                            model
                                                .selected_probe()
                                                .map(|probe| {
                                                    if probe.available && probe.authenticated {
                                                        rgb(colors.success).into()
                                                    } else {
                                                        rgb(colors.warning).into()
                                                    }
                                                })
                                                .unwrap_or_else(|| rgb(colors.muted).into()),
                                        )),
                                )
                                .on_click(cx.listener(|app, _, window, cx| {
                                    if app.presenter.model().updates.state.package().is_some() {
                                        app.settings_section = SettingsSection::General;
                                    }
                                    app.toggle_settings(window, cx)
                                })),
                        ),
                ),
            )
    }
}

#[cfg(test)]
mod tests;
