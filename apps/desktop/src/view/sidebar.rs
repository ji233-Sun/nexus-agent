use super::*;
use gpui_kit::component::{list::ListItem, scroll::Scrollbar, spinner::Spinner};

const SIDEBAR_ROW_HEIGHT: f32 = 40.;

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
    pub(super) fn render_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let model = self.presenter.model();
        let query = self.search_input.read(cx).value();
        let selected_project_id = model.selected_project.as_ref().map(|project| project.id);
        let projects = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .children(model.projects.iter().map(|project| {
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
                let tasks: Vec<_> = model
                    .tasks
                    .iter()
                    .filter(|task| matches_search(&task.title, &query))
                    .map(|task| {
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
                            .selected(model.selected_task == Some(id) && !model.cnb.opened)
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
                                                .debug_selector(move || {
                                                    format!("task-actions-{id}")
                                                })
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
                                                        PopupMenuItem::new(
                                                            locale.text("归档此对话"),
                                                        )
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
                                                                    app.confirm_delete_task(
                                                                        id, window, cx,
                                                                    )
                                                                });
                                                            }),
                                                    )
                                            },
                                        )
                                        .show_caret(false),
                                    )
                            })
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.select_task(id, window, cx)
                            }))
                    })
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
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .debug_selector(move || format!("sidebar-project-{project_id}"))
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
                                let project = new_task_project.clone();
                                div()
                                    .absolute()
                                    .right(px(2.))
                                    .top(px((SIDEBAR_ROW_HEIGHT - COMPACT_CONTROL_HEIGHT) / 2.))
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
                            .when(
                                selected && model.cnb.enabled && model.cnb.repository.is_some(),
                                |list| {
                                    list.child(
                                        div().relative().ml(-px(24.)).child(
                                            navigation_row(
                                                colors,
                                                (ElementId::from(project_id), "cnb"),
                                                "CNB",
                                                None,
                                            )
                                            .pl(px(34.))
                                            .suffix(move |_, _| {
                                                div()
                                                    .debug_selector(|| "sidebar-cnb-icon".into())
                                                    .absolute()
                                                    .left(px(10.))
                                                    .top(px(12.))
                                                    .child(cnb::cnb_icon(16., colors.accent))
                                            })
                                            .debug_selector(|| "sidebar-cnb".into())
                                            .selected(model.cnb.opened)
                                            .on_click(
                                                cx.listener(|app, _, window, cx| {
                                                    app.presenter.open_cnb();
                                                    app.focus_handle.focus(window, cx);
                                                    cx.notify();
                                                }),
                                            ),
                                        ),
                                    )
                                },
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
                                .tooltip(locale.text("在当前项目中开始新任务"))
                                .disabled(
                                    model.selected_project.is_none(),
                                )
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
mod tests {
    use super::*;
    use crate::infrastructure::storage::{NewTaskRun, Storage};
    use gpui::{ScrollDelta, ScrollWheelEvent, TestAppContext, point};
    use nexus_protocol::Event;
    use std::path::Path;

    #[gpui::test]
    fn software_update_settings_show_release_notes_and_installation_progress(
        cx: &mut TestAppContext,
    ) {
        use crate::{
            model::updates::{UpdateChannel, UpdateState},
            presenter::tests::{pending_update, update_package},
        };
        let (view, cx) = scroll_test_view(cx);
        cx.simulate_resize(gpui::size(px(1040.), px(680.)));
        view.update_in(cx, |view, window, cx| view.toggle_settings(window, cx));
        cx.run_until_parked();
        view.update_in(cx, |view, _, cx| {
            view.settings_scroll.scroll_to_bottom();
            cx.notify();
        });
        cx.run_until_parked();
        let channel_button = cx.debug_bounds("update-channel-nightly").unwrap().center();
        cx.simulate_click(channel_button, Default::default());
        assert_eq!(
            view.read_with(cx, |view, _| view.presenter.model().updates.channel),
            UpdateChannel::Nightly
        );
        let sender = view.update_in(cx, |view, _, cx| {
            let sender = pending_update(&mut view.presenter);
            cx.notify();
            sender
        });
        cx.run_until_parked();
        let other_channel = cx.debug_bounds("update-channel-release").unwrap().center();
        cx.simulate_click(other_channel, Default::default());
        assert_eq!(
            view.read_with(cx, |view, _| view.presenter.model().updates.channel),
            UpdateChannel::Nightly
        );
        sender
            .send(UpdateState::Available(update_package()))
            .unwrap();
        view.update_in(cx, |view, window, cx| {
            view.poll_events(Instant::now(), cx);
            view.settings_scroll.scroll_to_bottom();
            view.set_language(Language::English, window, cx);
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("download-update").is_some());
        assert!(cx.debug_bounds("release-notes").is_some());
        assert!(cx.debug_bounds("release-notes-link").is_some());
        let sender = view.update_in(cx, |view, _, cx| {
            let sender = pending_update(&mut view.presenter);
            sender
                .send(UpdateState::Installing(update_package()))
                .unwrap();
            view.poll_events(Instant::now(), cx);
            sender
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("download-update").is_none());
        assert!(cx.debug_bounds("release-notes").is_some());
        view.update_in(cx, |view, window, cx| view.toggle_settings(window, cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("update-ready-badge").is_some());
        drop(sender);
    }

    fn scroll_test_view(
        cx: &mut TestAppContext,
    ) -> (Entity<NexusView>, &mut gpui::VisualTestContext) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let directory = tempfile::tempdir().unwrap();
        let mut storage = Storage::open(Path::new(":memory:")).unwrap();
        let project = storage.open_project(directory.path()).unwrap();
        let long_title = "A deliberately long task title that must stay inside compact navigation";
        for index in 0..30 {
            storage
                .create_task_run(NewTaskRun {
                    workspace_id: None,
                    permission_mode: nexus_domain::PermissionMode::AutoEdit,
                    task_id: None,
                    project_id: project.id,
                    title: if index % 2 == 0 { "Hi" } else { long_title },
                    prompt: &"A long message for scrolling.\n\n".repeat(100),
                    harness: HarnessKind::Claude,
                    executable: "claude",
                    model: None,
                    effort: ThinkingEffort::Low,
                    harness_version: None,
                })
                .unwrap();
        }
        let mut presenter = Presenter::new(storage, Err(anyhow::anyhow!("test")), None);
        presenter.select_project(project);
        presenter.select_task(presenter.model().tasks[0].id);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = NexusView::new(presenter, window, cx);
            view.set_appearance(
                AppearanceSettings {
                    reduced_motion: true,
                    ..view.presenter.model().appearance
                },
                window,
                cx,
            );
            view
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        (view, cx)
    }

    #[gpui::test]
    fn task_rows_fill_the_same_width_and_accept_clicks_past_short_titles(cx: &mut TestAppContext) {
        let (view, cx) = scroll_test_view(cx);
        let tasks = view.read_with(cx, |view, _| view.presenter.model().tasks[..2].to_vec());
        let bounds = tasks
            .iter()
            .map(|task| {
                let selector = format!("sidebar-task-{}", task.id).leak();
                cx.debug_bounds(selector).unwrap()
            })
            .collect::<Vec<_>>();
        assert_ne!(tasks[0].title.len(), tasks[1].title.len());
        assert_eq!(bounds[0].left(), bounds[1].left());
        assert_eq!(bounds[0].size.width, bounds[1].size.width);
        assert_eq!(
            bounds[0].right(),
            cx.debug_bounds("add-project").unwrap().right()
        );
        let short_index = tasks.iter().position(|task| task.title == "Hi").unwrap();
        view.update(cx, |view, cx| {
            view.presenter.select_task(tasks[1 - short_index].id);
            cx.notify();
        });
        cx.run_until_parked();
        let action_selector = format!("task-actions-{}", tasks[short_index].id).leak();
        let action_bounds = cx.debug_bounds(action_selector).unwrap();
        cx.simulate_click(
            point(
                action_bounds.left() - px(8.),
                bounds[short_index].center().y,
            ),
            Default::default(),
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.presenter.model().selected_task),
            Some(tasks[short_index].id),
        );
    }

    #[gpui::test]
    fn supported_window_sizes_keep_workspace_regions_and_header_context_separate(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = scroll_test_view(cx);
        for size in [
            gpui::size(px(1040.), px(680.)),
            gpui::size(px(1280.), px(800.)),
        ] {
            cx.simulate_resize(size);
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            });

            let page = cx.debug_bounds("workspace-page").unwrap();
            let sidebar = cx.debug_bounds("workspace-sidebar").unwrap();
            let header = cx.debug_bounds("workspace-header").unwrap();
            let timeline = view.read_with(cx, |view, _| view.timeline_scroll.bounds());
            let composer = cx.debug_bounds("composer-surface").unwrap();
            let project = cx.debug_bounds("workspace-header-project").unwrap();
            let task = cx.debug_bounds("workspace-header-task").unwrap();
            let status = cx.debug_bounds("workspace-header-status").unwrap();

            for bounds in [sidebar, header, timeline, composer, project, task, status] {
                assert!(
                    bounds.left() >= page.left()
                        && bounds.right() <= page.right()
                        && bounds.top() >= page.top()
                        && bounds.bottom() <= page.bottom(),
                    "{bounds:?} must stay inside {page:?} at {size:?}"
                );
            }
            assert!(sidebar.right() <= header.left());
            assert!(header.bottom() <= timeline.top());
            assert!(timeline.bottom() <= composer.top());
            assert!(project.right() <= task.left());
            assert!(task.right() <= status.left());
            assert!(composer.left() >= timeline.left());
            assert!(composer.right() <= timeline.right());
        }
    }

    #[gpui::test]
    fn empty_workspace_surfaces_project_and_agent_readiness(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let directory = tempfile::tempdir().unwrap();
        let storage = Storage::open(Path::new(":memory:")).unwrap();
        let presenter = Presenter::new(storage, Err(anyhow::anyhow!("runner unavailable")), None);
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("workspace-empty-no-project").is_some());
        assert!(cx.debug_bounds("workspace-empty-agent-status").is_none());
        assert!(cx.debug_bounds("workspace-header-task").is_none());
        assert!(cx.debug_bounds("workspace-header-settled-status").is_some());
        assert!(cx.debug_bounds("workspace-header-pending-status").is_none());

        for size in [
            gpui::size(px(1040.), px(680.)),
            gpui::size(px(1280.), px(800.)),
        ] {
            cx.simulate_resize(size);
            cx.run_until_parked();
            let welcome = cx.debug_bounds("workspace-empty-no-project").unwrap();
            let choose_project = cx.debug_bounds("welcome-choose-project").unwrap();
            let timeline = view.read_with(cx, |view, _| view.timeline_scroll.bounds());
            assert!(choose_project.left() >= welcome.left());
            assert!(choose_project.right() <= welcome.right());
            assert!(choose_project.top() >= timeline.top());
            assert!(choose_project.bottom() <= timeline.bottom());
        }

        view.update(cx, |view, cx| {
            view.presenter.open_project(directory.path());
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("workspace-empty-no-project").is_none());
        assert!(cx.debug_bounds("workspace-empty-agent-status").is_some());
        assert!(cx.debug_bounds("workspace-empty-status").is_some());
        assert!(cx.debug_bounds("workspace-header-task").is_some());
        assert!(cx.debug_bounds("workspace-header-settled-status").is_some());
    }

    #[gpui::test]
    fn header_marks_an_active_run_as_background_after_selecting_another_task(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, runner, _directory) = crate::presenter::tests::fixture();
        assert!(presenter.submit("inactive conversation", "claude"));
        let inactive_task = presenter.model().active_task.unwrap();
        runner.emit(Event::RunExited {
            run_id: presenter.model().active_run.unwrap(),
            status: RunStatus::Completed,
            exit_code: Some(0),
        });
        presenter.drain_events();
        presenter.new_task();
        assert!(presenter.submit("background conversation", "claude"));
        let active_task = presenter.model().active_task.unwrap();
        presenter.select_task(inactive_task);
        assert_ne!(presenter.model().selected_task, Some(active_task));

        let (_view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });

        assert!(
            cx.debug_bounds("workspace-header-background-status")
                .is_some()
        );
        assert!(cx.debug_bounds("workspace-header-task").is_some());
    }

    #[gpui::test]
    fn task_menu_archives_and_settings_restores_the_conversation(cx: &mut TestAppContext) {
        let (view, cx) = scroll_test_view(cx);
        let task_id = view.read_with(cx, |view, _| view.presenter.model().selected_task.unwrap());
        let task_selector = format!("sidebar-task-{task_id}").leak();
        let actions_selector = format!("task-actions-{task_id}").leak();
        let actions = cx.debug_bounds(actions_selector).unwrap().center();

        cx.simulate_click(actions, Default::default());
        cx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("animated-menu-surface").is_some());
        cx.simulate_keystrokes("down enter");
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        view.read_with(cx, |view, _| {
            assert!(view.presenter.model().selected_task.is_none());
            assert_eq!(view.presenter.model().archived_tasks[0].id, task_id);
        });
        assert!(cx.debug_bounds(task_selector).is_none());

        let settings = cx.debug_bounds("open-settings").unwrap().center();
        cx.simulate_click(settings, Default::default());
        cx.run_until_parked();
        let archived = cx.debug_bounds("settings-nav-archived").unwrap().center();
        cx.simulate_click(archived, Default::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        let restore_selector = format!("restore-archived-{task_id}").leak();
        let delete_selector = format!("delete-archived-{task_id}").leak();
        assert!(cx.debug_bounds(delete_selector).is_some());
        assert!(cx.debug_bounds("delete-all-archived").is_some());
        let restore = cx.debug_bounds(restore_selector).unwrap().center();
        cx.simulate_click(restore, Default::default());
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(view.presenter.model().archived_tasks.is_empty());
            assert!(
                view.presenter
                    .model()
                    .tasks
                    .iter()
                    .any(|task| task.id == task_id)
            );
        });
    }

    #[gpui::test]
    fn permanent_task_deletion_requires_explicit_confirmation(cx: &mut TestAppContext) {
        let (view, cx) = scroll_test_view(cx);
        let (task_id, task_title) = view.read_with(cx, |view, _| {
            let task_id = view.presenter.model().selected_task.unwrap();
            let task_title = view
                .presenter
                .model()
                .tasks
                .iter()
                .find(|task| task.id == task_id)
                .unwrap()
                .title
                .clone();
            (task_id, task_title)
        });

        let actions_selector = format!("task-actions-{task_id}").leak();
        let actions = cx.debug_bounds(actions_selector).unwrap().center();
        cx.simulate_click(actions, Default::default());
        cx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        cx.simulate_keystrokes("down down enter");
        assert!(cx.has_pending_prompt());
        let (message, detail) = cx.pending_prompt().unwrap();
        assert_eq!(message, format!("永久删除“{task_title}”？"));
        assert!(detail.contains("无法撤销"));
        assert!(view.read_with(cx, |view, _| {
            view.presenter
                .model()
                .tasks
                .iter()
                .any(|task| task.id == task_id)
        }));

        cx.simulate_prompt_answer("取消");
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| {
            view.presenter
                .model()
                .tasks
                .iter()
                .any(|task| task.id == task_id)
        }));

        view.update_in(cx, |view, window, cx| {
            view.archive_task(task_id, window, cx);
        });
        let settings = cx.debug_bounds("open-settings").unwrap().center();
        cx.simulate_click(settings, Default::default());
        cx.run_until_parked();
        let archived = cx.debug_bounds("settings-nav-archived").unwrap().center();
        cx.simulate_click(archived, Default::default());
        cx.run_until_parked();

        let delete_selector = format!("delete-archived-{task_id}").leak();
        let delete = cx.debug_bounds(delete_selector).unwrap().center();
        cx.simulate_click(delete, Default::default());
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("永久删除");
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| {
            view.presenter.model().archived_tasks.is_empty()
        }));
    }

    #[gpui::test]
    fn clearing_archived_tasks_requires_counted_confirmation(cx: &mut TestAppContext) {
        let (view, cx) = scroll_test_view(cx);
        let task_ids = view.read_with(cx, |view, _| {
            view.presenter
                .model()
                .tasks
                .iter()
                .take(2)
                .map(|task| task.id)
                .collect::<Vec<_>>()
        });
        view.update_in(cx, |view, window, cx| {
            for task_id in task_ids {
                view.archive_task(task_id, window, cx);
            }
        });
        let settings = cx.debug_bounds("open-settings").unwrap().center();
        cx.simulate_click(settings, Default::default());
        cx.run_until_parked();
        let archived = cx.debug_bounds("settings-nav-archived").unwrap().center();
        cx.simulate_click(archived, Default::default());
        cx.run_until_parked();

        let clear = cx.debug_bounds("delete-all-archived").unwrap().center();
        cx.simulate_click(clear, Default::default());
        assert!(cx.has_pending_prompt());
        let (message, detail) = cx.pending_prompt().unwrap();
        assert_eq!(message, "永久删除 2 个归档对话？");
        assert!(detail.contains("这 2 个对话"));
        assert_eq!(
            view.read_with(cx, |view, _| view.presenter.model().archived_tasks.len()),
            2
        );

        cx.simulate_prompt_answer("取消");
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.presenter.model().archived_tasks.len()),
            2
        );

        let clear = cx.debug_bounds("delete-all-archived").unwrap().center();
        cx.simulate_click(clear, Default::default());
        cx.simulate_prompt_answer("全部删除");
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| {
            view.presenter.model().archived_tasks.is_empty()
        }));
    }

    #[gpui::test]
    fn managing_another_task_preserves_the_current_timeline_state(cx: &mut TestAppContext) {
        let (view, cx) = scroll_test_view(cx);
        let (selected_task, other_tasks) = view.read_with(cx, |view, _| {
            let selected_task = view.presenter.model().selected_task.unwrap();
            let other_tasks = view
                .presenter
                .model()
                .tasks
                .iter()
                .filter(|task| task.id != selected_task)
                .take(2)
                .map(|task| task.id)
                .collect::<Vec<_>>();
            (selected_task, other_tasks)
        });
        let expanded_message = ElementId::from("expanded-message");
        view.update_in(cx, |view, window, cx| {
            view.timeline_scroll.set_offset(point(px(0.), px(-120.)));
            view.expanded_messages.insert(expanded_message.clone());
            view.search_input
                .update(cx, |input, cx| input.focus(window, cx));

            view.archive_task(other_tasks[0], window, cx);
            assert_eq!(view.presenter.model().selected_task, Some(selected_task));
            assert_eq!(view.timeline_scroll.offset().y, px(-120.));
            assert!(view.expanded_messages.contains(&expanded_message));
            assert!(
                view.search_input
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );

            view.delete_task(other_tasks[1], window, cx);
            assert_eq!(view.presenter.model().selected_task, Some(selected_task));
            assert_eq!(view.timeline_scroll.offset().y, px(-120.));
            assert!(view.expanded_messages.contains(&expanded_message));
            assert!(
                view.search_input
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
        });
    }

    #[gpui::test]
    fn project_disclosure_animates_layout_reversibly_and_can_skip_motion(cx: &mut TestAppContext) {
        let (view, cx) = scroll_test_view(cx);
        let project_id = view.read_with(cx, |view, _| {
            view.presenter.model().selected_project.as_ref().unwrap().id
        });
        let project_selector = format!("sidebar-project-{project_id}").leak();
        let project_bounds = cx.debug_bounds(project_selector).unwrap();
        let trigger = point(project_bounds.left() + px(60.), project_bounds.center().y);
        let selected_task = view.read_with(cx, |view, _| view.presenter.model().selected_task);
        view.update_in(cx, |view, window, cx| {
            view.set_appearance(
                AppearanceSettings {
                    reduced_motion: false,
                    ..view.presenter.model().appearance
                },
                window,
                cx,
            );
        });
        let frame = |cx: &mut gpui::VisualTestContext, millis| {
            cx.executor().advance_clock(Duration::from_millis(millis));
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
                let _ = window.draw(cx);
            });
        };
        frame(cx, 0);
        let expanded_y = cx.debug_bounds("add-project").unwrap().origin.y;
        cx.simulate_click(trigger, Default::default());
        assert!(view.read_with(cx, |view, _| view.collapsed_projects.contains(&project_id)));
        frame(cx, 0);
        assert_eq!(cx.debug_bounds("add-project").unwrap().origin.y, expanded_y);
        frame(cx, 20);
        let closing_y = cx.debug_bounds("add-project").unwrap().origin.y;
        assert!(closing_y < expanded_y);
        cx.simulate_click(trigger, Default::default());
        frame(cx, 0);
        assert_eq!(cx.debug_bounds("add-project").unwrap().origin.y, closing_y);
        frame(cx, 200);
        assert_eq!(cx.debug_bounds("add-project").unwrap().origin.y, expanded_y);
        cx.simulate_click(trigger, Default::default());
        frame(cx, 0);
        frame(cx, 200);
        assert!(cx.debug_bounds("add-project").unwrap().origin.y < closing_y);
        view.update_in(cx, |view, window, cx| {
            view.set_appearance(
                AppearanceSettings {
                    reduced_motion: true,
                    ..view.presenter.model().appearance
                },
                window,
                cx,
            );
        });
        cx.simulate_click(trigger, Default::default());
        frame(cx, 0);
        assert_eq!(cx.debug_bounds("add-project").unwrap().origin.y, expanded_y);
        assert_eq!(
            view.read_with(cx, |view, _| view.presenter.model().selected_task),
            selected_task,
        );
    }

    #[gpui::test]
    fn project_new_chat_opens_its_project_without_toggling_the_row(cx: &mut TestAppContext) {
        let (view, cx) = scroll_test_view(cx);
        let project_id = view.read_with(cx, |view, _| {
            assert!(view.presenter.model().selected_task.is_some());
            view.presenter.model().selected_project.as_ref().unwrap().id
        });
        let button_selector = format!("project-new-task-{project_id}").leak();
        let project_selector = format!("sidebar-project-{project_id}").leak();
        assert!(cx.debug_bounds(button_selector).is_none());
        let project_bounds = cx.debug_bounds(project_selector).unwrap();
        cx.simulate_mouse_move(project_bounds.center(), None, Default::default());
        assert!(cx.debug_bounds(button_selector).is_some());
        cx.simulate_mouse_move(point(px(400.), px(100.)), None, Default::default());
        assert!(cx.debug_bounds(button_selector).is_none());
        view.update(cx, |view, cx| {
            view.collapsed_projects.insert(project_id);
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.simulate_mouse_move(project_bounds.center(), None, Default::default());
        let button = cx.debug_bounds(button_selector).unwrap().center();
        cx.simulate_click(button, Default::default());
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.presenter.model().selected_project.as_ref().unwrap().id,
                project_id
            );
            assert!(view.presenter.model().selected_task.is_none());
            assert!(view.presenter.model().messages.is_empty());
            assert!(!view.collapsed_projects.contains(&project_id));
        });

        let other_project = tempfile::tempdir().unwrap();
        view.update(cx, |view, cx| {
            view.presenter.open_project(other_project.path());
            assert_ne!(
                view.presenter.model().selected_project.as_ref().unwrap().id,
                project_id
            );
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let project_bounds = cx.debug_bounds(project_selector).unwrap();
        cx.simulate_mouse_move(project_bounds.center(), None, Default::default());
        let button = cx.debug_bounds(button_selector).unwrap().center();
        cx.simulate_click(button, Default::default());
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.presenter.model().selected_project.as_ref().unwrap().id,
                project_id
            );
            assert!(view.presenter.model().selected_task.is_none());
            assert!(!view.collapsed_projects.contains(&project_id));
        });
    }

    // Measures CPU input/layout/paint work; the test platform does not present GPU frames.
    #[gpui::test]
    #[ignore]
    fn scroll_frame_cost(cx: &mut TestAppContext) {
        let (view, cx) = scroll_test_view(cx);
        for (name, handle, pane) in view.read_with(cx, |view, _| {
            [
                (
                    "sidebar",
                    view.sidebar_scroll.clone(),
                    view.sidebar_pane.clone(),
                ),
                (
                    "timeline",
                    view.timeline_scroll.clone(),
                    view.timeline_pane.clone(),
                ),
            ]
        }) {
            assert!(handle.max_offset().y > px(60.));
            handle.set_offset(point(px(0.), px(0.)));
            pane.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            let mut samples = Vec::new();
            let before = pane.read_with(cx, |pane, _| pane.render_count);
            for frame in 0..140 {
                let delta = px(if frame % 40 < 20 { -3. } else { 3. });
                let previous = handle.offset().y;
                let started = Instant::now();
                cx.simulate_event(ScrollWheelEvent {
                    position: handle.bounds().center(),
                    delta: ScrollDelta::Pixels(point(px(0.), delta)),
                    touch_phase: gpui::TouchPhase::Moved,
                    ..Default::default()
                });
                if frame >= 20 {
                    samples.push(started.elapsed().as_secs_f64() * 1000.);
                }
                assert_eq!(handle.offset().y, previous + delta);
            }
            let renders = pane.read_with(cx, |pane, _| pane.render_count) - before;
            assert!(renders >= 140);
            samples.sort_by(f64::total_cmp);
            eprintln!(
                "{name}: CPU ms/event median={:.2}, p95={:.2}; pane renders={}",
                samples[samples.len() / 2],
                samples[samples.len() * 95 / 100],
                renders,
            );
        }
    }

    #[gpui::test]
    fn sidebar_scrollbar_tracks_scrolling_and_dragging(cx: &mut TestAppContext) {
        let (view, cx) = scroll_test_view(cx);
        cx.update(|window, cx| {
            gpui_kit::component::Theme::set_scrollbar_mode(
                gpui_kit::component::scroll::ScrollbarMode::Always,
                cx,
            );
            window.refresh();
            let _ = window.draw(cx);
        });
        let scroll = view.read_with(cx, |view, _| view.sidebar_scroll.clone());
        let viewport = scroll.bounds();
        let device_pixel = cx.update(|window, _| px(1. / window.scale_factor()));
        let content_height = scroll.bounds_for_item(0).unwrap().size.height;
        let max_offset = (content_height - viewport.size.height).max(px(0.));
        assert_eq!(
            scroll.max_offset().y,
            max_offset,
            "only navigation content may contribute to the scroll range"
        );
        let painted_thumbs = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, _| {
                window
                    .painted_quads()
                    .into_iter()
                    .map(|quad| quad.bounds.map(|value| px(value.0 / window.scale_factor())))
                    .filter(|bounds| {
                        bounds.left() >= viewport.right() - px(16.)
                            && bounds.right() <= viewport.right()
                            && bounds.size.width > px(0.)
                            && bounds.size.width < px(16.)
                            && bounds.size.height > px(32.)
                    })
                    .collect::<Vec<_>>()
            })
        };
        let thumb_bounds = |cx: &mut gpui::VisualTestContext| {
            let thumbs = painted_thumbs(cx);
            assert_eq!(thumbs.len(), 1, "expected one painted sidebar thumb");
            thumbs[0]
        };
        let initial_thumb = thumb_bounds(cx);
        assert!((initial_thumb.top() - viewport.top()).abs() < px(8.));
        for delta in [
            ScrollDelta::Pixels(point(px(0.), px(-80.))),
            ScrollDelta::Lines(point(0., -3.)),
        ] {
            let before = thumb_bounds(cx);
            cx.simulate_event(ScrollWheelEvent {
                position: viewport.center(),
                delta,
                ..Default::default()
            });
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let after = thumb_bounds(cx);
            assert!(
                after.top() > before.top(),
                "thumb must follow downward scrolling"
            );
            // Painting snaps each edge separately, so translation can change the
            // painted height by one device pixel without changing the layout.
            assert!(
                (after.size.height - initial_thumb.size.height).abs() <= device_pixel,
                "thumb height must stay constant within one device pixel: initial={initial_thumb:?}, after={after:?}"
            );
            assert_eq!(scroll.bounds(), viewport);
        }

        let thumb = thumb_bounds(cx);
        let before_drag = scroll.offset().y;
        let destination = thumb.center() + point(px(0.), px(40.));
        cx.simulate_mouse_down(thumb.center(), gpui::MouseButton::Left, Default::default());
        cx.simulate_mouse_move(destination, gpui::MouseButton::Left, Default::default());
        cx.simulate_mouse_up(destination, gpui::MouseButton::Left, Default::default());
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            scroll.offset().y < before_drag,
            "dragging must scroll the sidebar"
        );
        assert!(thumb_bounds(cx).top() > thumb.top());

        scroll.scroll_to_bottom();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        let bottom_thumb = thumb_bounds(cx);
        assert!(bottom_thumb.top() > initial_thumb.top());
        assert!((bottom_thumb.bottom() - viewport.bottom()).abs() < px(8.));
        assert_eq!(scroll.offset().y, -max_offset);
        assert_eq!(
            scroll.bounds_for_item(0).unwrap().bottom() + scroll.offset().y,
            viewport.bottom(),
            "the last content edge must meet the viewport bottom without blank overscroll"
        );

        for (destination_y, expected_offset, delta_y) in [
            (viewport.top() - px(500.), px(0.), 10_000.),
            (viewport.bottom() + px(500.), -max_offset, -10_000.),
        ] {
            let thumb = thumb_bounds(cx);
            let destination = point(thumb.center().x, destination_y);
            cx.simulate_mouse_down(thumb.center(), gpui::MouseButton::Left, Default::default());
            cx.simulate_mouse_move(destination, gpui::MouseButton::Left, Default::default());
            cx.simulate_mouse_up(destination, gpui::MouseButton::Left, Default::default());
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert_eq!(scroll.offset().y, expected_offset);
            for delta in [
                ScrollDelta::Pixels(point(px(0.), px(delta_y))),
                ScrollDelta::Lines(point(0., delta_y)),
            ] {
                cx.simulate_event(ScrollWheelEvent {
                    position: viewport.center(),
                    delta,
                    ..Default::default()
                });
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                assert_eq!(scroll.offset().y, expected_offset);
                assert_eq!(scroll.max_offset().y, max_offset);
                let thumb = thumb_bounds(cx);
                assert!(thumb.top() >= viewport.top());
                assert!(thumb.bottom() <= viewport.bottom());
            }
        }

        view.update(cx, |view, cx| {
            let project_id = view.presenter.model().selected_project.as_ref().unwrap().id;
            view.collapsed_projects.insert(project_id);
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(scroll.bounds_for_item(0).unwrap().size.height < viewport.size.height);
        assert_eq!(scroll.max_offset().y, px(0.));
        assert_eq!(scroll.offset().y, px(0.));
        assert!(painted_thumbs(cx).is_empty());
        for delta_y in [-10_000., 10_000.] {
            cx.simulate_event(ScrollWheelEvent {
                position: viewport.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(delta_y))),
                ..Default::default()
            });
            assert_eq!(scroll.offset().y, px(0.));
        }
    }

    #[gpui::test]
    fn scroll_regions_keep_offsets_and_rendering_independent(cx: &mut TestAppContext) {
        let (view, cx) = scroll_test_view(cx);
        let scroll = view.read_with(cx, |view, _| view.timeline_scroll.clone());
        assert!(scroll.max_offset().y > px(100.));
        scroll.set_offset(point(px(0.), px(-100.)));
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let (sidebar_scroll, timeline_pane, sidebar_pane) = view.read_with(cx, |view, _| {
            (
                view.sidebar_scroll.clone(),
                view.timeline_pane.clone(),
                view.sidebar_pane.clone(),
            )
        });
        let timeline_renders = timeline_pane.read_with(cx, |pane, _| pane.render_count);
        let sidebar_renders = sidebar_pane.read_with(cx, |pane, _| pane.render_count);
        let before = scroll.offset();
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(100.), px(350.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-80.))),
            ..Default::default()
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert_eq!(scroll.offset(), before);
        assert_eq!(sidebar_scroll.offset().y, px(-80.));
        assert_eq!(
            timeline_pane.read_with(cx, |pane, _| pane.render_count),
            timeline_renders
        );
        assert!(sidebar_pane.read_with(cx, |pane, _| pane.render_count) > sidebar_renders);

        cx.simulate_event(ScrollWheelEvent {
            position: scroll.bounds().center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
            ..Default::default()
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert_eq!(scroll.offset().y, before.y - px(40.));
        assert_eq!(sidebar_scroll.offset().y, px(-80.));
        let workspace_bounds = cx.debug_bounds("workspace-page").unwrap();
        let settings_button = cx.debug_bounds("sidebar-settings").unwrap().center();
        assert!(workspace_bounds.contains(&settings_button));
        cx.simulate_click(settings_button, Default::default());
        assert!(view.read_with(cx, |view, _| view.settings_open));
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        let providers_tab = cx.debug_bounds("settings-nav-providers").unwrap().center();
        cx.simulate_click(providers_tab, Default::default());
        cx.run_until_parked();
        let settings_scroll = view.read_with(cx, |view, _| view.settings_scroll.clone());
        assert_eq!(cx.debug_bounds("settings-page").unwrap(), workspace_bounds);
        assert!(cx.debug_bounds("workspace-page").is_none());
        assert!(cx.debug_bounds("sidebar-settings").is_none());
        assert!(settings_scroll.max_offset().y > px(40.));
        let back_button = cx.debug_bounds("back-to-workspace").unwrap();
        let navigation = cx.debug_bounds("settings-navigation").unwrap();
        let breadcrumb = cx.debug_bounds("settings-breadcrumb").unwrap();
        assert!(navigation.right() <= settings_scroll.bounds().left());
        let timeline_renders = timeline_pane.read_with(cx, |pane, _| pane.render_count);
        let sidebar_renders = sidebar_pane.read_with(cx, |pane, _| pane.render_count);
        let before = scroll.offset();
        cx.simulate_event(ScrollWheelEvent {
            position: settings_scroll.bounds().center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
            ..Default::default()
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert_eq!(scroll.offset(), before);
        assert_eq!(sidebar_scroll.offset().y, px(-80.));
        assert_eq!(settings_scroll.offset().y, px(-40.));
        assert_eq!(cx.debug_bounds("back-to-workspace").unwrap(), back_button);
        assert_eq!(cx.debug_bounds("settings-navigation").unwrap(), navigation);
        assert_eq!(cx.debug_bounds("settings-breadcrumb").unwrap(), breadcrumb);
        assert_eq!(
            timeline_pane.read_with(cx, |pane, _| pane.render_count),
            timeline_renders
        );
        assert_eq!(
            sidebar_pane.read_with(cx, |pane, _| pane.render_count),
            sidebar_renders
        );

        cx.simulate_event(ScrollWheelEvent {
            position: navigation.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
            ..Default::default()
        });
        cx.simulate_click(providers_tab, Default::default());
        cx.run_until_parked();
        assert_eq!(settings_scroll.offset().y, px(-40.));
        let general_tab = cx.debug_bounds("settings-nav-general").unwrap().center();
        cx.simulate_click(general_tab, Default::default());
        cx.run_until_parked();
        assert_eq!(settings_scroll.offset().y, px(0.));
        assert!(cx.debug_bounds("settings-content-general").is_some());
        assert!(cx.debug_bounds("settings-content-providers").is_none());
        cx.simulate_click(providers_tab, Default::default());
        cx.run_until_parked();
        assert_eq!(settings_scroll.offset().y, px(0.));

        cx.simulate_click(back_button.center(), Default::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("settings-page").is_none());
        assert_eq!(cx.debug_bounds("workspace-page").unwrap(), workspace_bounds);
        assert_eq!(scroll.offset(), before);
        assert_eq!(sidebar_scroll.offset().y, px(-80.));
        let before = scroll.offset();
        cx.simulate_event(ScrollWheelEvent {
            position: scroll.bounds().center(),
            delta: ScrollDelta::Pixels(point(px(-80.), px(0.))),
            touch_phase: gpui::TouchPhase::Started,
            ..Default::default()
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert_eq!(scroll.offset(), before);
    }

    #[gpui::test]
    fn appearance_controls_and_system_changes_preserve_workspace_state(cx: &mut TestAppContext) {
        let (view, cx) = scroll_test_view(cx);
        let assert_preview_corners = |cx: &mut gpui::VisualTestContext| {
            for selector in [
                "appearance-theme-preview-system",
                "appearance-theme-preview-light",
                "appearance-theme-preview-dark",
            ] {
                let bounds = cx.debug_bounds(selector).expect(selector);
                cx.update(|window, _| {
                    let bounds = bounds.scale(window.scale_factor());
                    let quads = window.painted_quads();
                    let border = quads.iter().find(|quad| quad.bounds == bounds).unwrap();
                    let inner = gpui::Bounds::from_corners(
                        bounds.origin + point(border.border_widths.left, border.border_widths.top),
                        bounds.bottom_right()
                            - point(border.border_widths.right, border.border_widths.bottom),
                    );
                    let left = quads
                        .iter()
                        .find(|quad| {
                            quad.bounds.origin == inner.origin
                                && quad.bounds.bottom() == inner.bottom()
                        })
                        .unwrap();
                    let right = quads
                        .iter()
                        .find(|quad| {
                            quad.bounds.top() == inner.top()
                                && quad.bounds.bottom_right() == inner.bottom_right()
                        })
                        .unwrap();
                    assert_eq!(left.bounds.right(), right.bounds.left());
                    let radii = border
                        .corner_radii
                        .map(|radius| *radius - border.border_widths.top);
                    assert_eq!(
                        left.corner_radii,
                        gpui::Corners {
                            top_left: radii.top_left,
                            bottom_left: radii.bottom_left,
                            ..Default::default()
                        },
                        "{selector}: left background must meet the rounded border without gaps"
                    );
                    assert_eq!(
                        right.corner_radii,
                        gpui::Corners {
                            top_right: radii.top_right,
                            bottom_right: radii.bottom_right,
                            ..Default::default()
                        },
                        "{selector}: right background must meet the rounded border without gaps"
                    );
                });
            }
        };
        let task = view.read_with(cx, |view, _| view.presenter.model().selected_task);
        view.update_in(cx, |view, window, cx| {
            view.timeline_scroll.set_offset(point(px(0.), px(-120.)));
            view.sidebar_scroll.set_offset(point(px(0.), px(-60.)));
            view.prompt_input
                .update(cx, |input, cx| input.set_value("Keep my draft", window, cx));
            view.toggle_settings(window, cx);
        });
        cx.run_until_parked();
        let appearance_tab = cx.debug_bounds("settings-nav-appearance").unwrap().center();
        cx.simulate_click(appearance_tab, Default::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("settings-content-appearance").is_some());
        assert!(cx.debug_bounds("settings-content-general").is_none());
        for size in [
            gpui::size(px(1040.), px(680.)),
            gpui::size(px(1280.), px(800.)),
        ] {
            cx.simulate_resize(size);
            cx.run_until_parked();
            assert_preview_corners(cx);
            let page = cx.debug_bounds("settings-page").unwrap();
            let navigation = cx.debug_bounds("settings-navigation").unwrap();
            let content = cx.debug_bounds("settings-content-appearance").unwrap();
            let settings_scroll = view.read_with(cx, |view, _| view.settings_scroll.clone());
            assert!(navigation.right() <= settings_scroll.bounds().left());
            assert_eq!(
                content.left(),
                cx.debug_bounds("settings-breadcrumb-label").unwrap().left()
            );
            assert!(content.right() <= page.right());
            assert_eq!(settings_scroll.max_offset().x, px(0.));
            assert_eq!(
                cx.debug_bounds("appearance-glass").unwrap().left(),
                cx.debug_bounds("reduce-motion").unwrap().left()
            );
            for (selector, theme) in [
                ("appearance-theme-dark", ThemePreference::Dark),
                ("appearance-theme-light", ThemePreference::Light),
                ("appearance-theme-system", ThemePreference::System),
            ] {
                let bounds = cx.debug_bounds(selector).unwrap();
                assert!(
                    cx.debug_bounds("settings-page")
                        .unwrap()
                        .contains(&bounds.center())
                );
                cx.simulate_mouse_move(bounds.center(), None, Default::default());
                cx.run_until_parked();
                assert_preview_corners(cx);
                cx.simulate_click(bounds.center(), Default::default());
                cx.run_until_parked();
                assert_preview_corners(cx);
                view.read_with(cx, |view, cx| {
                    assert_eq!(view.presenter.model().appearance.theme, theme);
                    assert_eq!(view.presenter.model().selected_task, task);
                    assert_eq!(view.prompt_input.read(cx).value(), "Keep my draft");
                    assert_eq!(view.timeline_scroll.offset().y, px(-120.));
                    assert_eq!(view.sidebar_scroll.offset().y, px(-60.));
                });
            }
            for selector in [
                "appearance-glass",
                "appearance-glass",
                "reduce-motion",
                "reduce-motion",
            ] {
                let bounds = cx.debug_bounds(selector).unwrap();
                cx.simulate_click(bounds.center(), Default::default());
                cx.run_until_parked();
                view.read_with(cx, |view, cx| {
                    assert_eq!(
                        cx.reduce_motion(),
                        view.presenter.model().appearance.reduced_motion
                    );
                    assert_eq!(view.timeline_scroll.offset().y, px(-120.));
                    assert_eq!(view.presenter.model().selected_task, task);
                    assert_eq!(view.prompt_input.read(cx).value(), "Keep my draft");
                });
            }
        }
        for system in [gpui::WindowAppearance::Dark, gpui::WindowAppearance::Light] {
            view.update_in(cx, |view, window, cx| {
                let appearance = ResolvedAppearance::resolve(
                    view.presenter.model().appearance,
                    system,
                    SystemAccessibility::default(),
                    true,
                    cfg!(target_os = "macos"),
                );
                apply_theme(appearance, cx);
                window.refresh();
                cx.notify();
            });
            cx.run_until_parked();
            assert_preview_corners(cx);
            cx.update(|_, cx| {
                assert_eq!(
                    cx.global::<ResolvedAppearance>().dark,
                    system == gpui::WindowAppearance::Dark,
                )
            });
        }
        let counts = view.read_with(cx, |view, cx| {
            (
                view.sidebar_pane.read(cx).render_count,
                view.timeline_pane.read(cx).render_count,
            )
        });
        cx.executor().advance_clock(Duration::from_millis(500));
        cx.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                counts,
                (
                    view.sidebar_pane.read(cx).render_count,
                    view.timeline_pane.read(cx).render_count
                )
            );
        });
    }

    #[gpui::test]
    fn settings_navigation_preserves_drafts_and_restores_visible_focus(cx: &mut TestAppContext) {
        let (view, cx) = scroll_test_view(cx);
        cx.simulate_resize(gpui::size(px(1040.), px(680.)));
        cx.run_until_parked();
        let selected_task = view.read_with(cx, |view, _| view.presenter.model().selected_task);
        view.update_in(cx, |view, window, cx| {
            view.prompt_input.update(cx, |input, cx| {
                input.set_value("Keep this draft", window, cx);
                input.focus(window, cx);
            });
        });
        cx.run_until_parked();
        let settings_button = cx.debug_bounds("open-settings").unwrap().center();
        cx.simulate_click(settings_button, Default::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("settings-content-general").is_some());
        let agent_tab = cx.debug_bounds("settings-nav-agent").unwrap().center();
        cx.simulate_click(agent_tab, Default::default());
        view.update_in(cx, |view, window, cx| {
            assert!(view.settings_open);
            assert_eq!(view.settings_section, SettingsSection::Agent);
            assert!(view.focus_handle.is_focused(window));
            assert!(
                !view
                    .prompt_input
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
            view.executable_input.update(cx, |input, cx| {
                input.set_value("custom-agent", window, cx);
                input.focus(window, cx);
            });
            view.provider_name_input.update(cx, |input, cx| {
                input.set_value("Keep this provider draft", window, cx);
            });
            view.provider_api_key_input.update(cx, |input, cx| {
                input.set_value("unsaved-test-key", window, cx);
            });
        });
        for (selector, section) in [
            ("settings-nav-providers", SettingsSection::Providers),
            ("settings-nav-remote", SettingsSection::Remote),
            ("settings-nav-appearance", SettingsSection::Appearance),
            ("settings-nav-general", SettingsSection::General),
            ("settings-nav-providers", SettingsSection::Providers),
        ] {
            cx.run_until_parked();
            let tab = cx.debug_bounds(selector).unwrap().center();
            cx.simulate_click(tab, Default::default());
            view.update_in(cx, |view, window, cx| {
                assert_eq!(view.settings_section, section);
                assert!(view.focus_handle.is_focused(window));
                assert_eq!(view.executable_input.read(cx).value(), "custom-agent");
                assert_eq!(
                    view.provider_name_input.read(cx).value(),
                    "Keep this provider draft"
                );
                assert_eq!(
                    view.provider_api_key_input.read(cx).value(),
                    "unsaved-test-key"
                );
                if section == SettingsSection::Providers {
                    view.provider_name_input
                        .update(cx, |input, cx| input.focus(window, cx));
                }
            });
            if section == SettingsSection::Providers {
                cx.run_until_parked();
                let harness = cx.debug_bounds("settings-provider-harness").unwrap();
                let profile = cx.debug_bounds("settings-provider-profile").unwrap();
                assert_eq!(harness.left(), profile.left());
                assert_eq!(harness.size, profile.size);
            }
            if section == SettingsSection::General {
                for (selector, language, prompt_placeholder, search_placeholder, group_title) in [
                    (
                        "language-en",
                        Language::English,
                        "Describe a goal for the agent…",
                        "Search tasks…",
                        "Default",
                    ),
                    (
                        "language-zh-CN",
                        Language::Chinese,
                        "描述一个目标，让 Agent 开始工作…",
                        "搜索任务…",
                        "默认",
                    ),
                ] {
                    cx.run_until_parked();
                    let language_button = cx.debug_bounds(selector).unwrap();
                    cx.simulate_click(language_button.center(), Default::default());
                    cx.run_until_parked();
                    view.read_with(cx, |view, cx| {
                        assert_eq!(view.presenter.model().language, language);
                        assert_eq!(
                            view.prompt_input.read(cx).presentation().placeholder(),
                            prompt_placeholder
                        );
                        assert_eq!(
                            view.search_input.read(cx).presentation().placeholder(),
                            search_placeholder
                        );
                        assert_eq!(
                            view.catalog_model_select_content.groups[0].title,
                            group_title
                        );
                        assert_eq!(view.prompt_input.read(cx).value(), "Keep this draft");
                        assert_eq!(view.executable_input.read(cx).value(), "custom-agent");
                        assert_eq!(
                            view.provider_name_input.read(cx).value(),
                            "Keep this provider draft"
                        );
                        assert_eq!(
                            view.provider_api_key_input.read(cx).value(),
                            "unsaved-test-key"
                        );
                        assert_eq!(view.presenter.model().selected_task, selected_task);
                    });
                }
            }
        }
        cx.run_until_parked();
        let settings_shortcut = if cfg!(target_os = "macos") {
            "cmd-,"
        } else {
            "ctrl-,"
        };
        cx.simulate_keystrokes(settings_shortcut);
        view.update_in(cx, |view, window, cx| {
            assert!(!view.settings_open);
            assert!(
                view.prompt_input
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
            assert_eq!(view.prompt_input.read(cx).value(), "Keep this draft");
            assert_eq!(view.executable_input.read(cx).value(), "custom-agent");
            assert_eq!(view.presenter.model().selected_task, selected_task);
        });
        cx.simulate_keystrokes(settings_shortcut);
        view.read_with(cx, |view, cx| {
            assert!(view.settings_open);
            assert_eq!(view.settings_section, SettingsSection::Providers);
            assert_eq!(
                view.provider_name_input.read(cx).value(),
                "Keep this provider draft"
            );
        });
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-k"
        } else {
            "ctrl-k"
        });
        view.update_in(cx, |view, window, cx| {
            assert!(!view.settings_open);
            assert!(
                view.search_input
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
            assert_eq!(view.presenter.model().selected_task, selected_task);
        });
        cx.simulate_keystrokes(settings_shortcut);
        assert!(view.read_with(cx, |view, _| view.settings_open));
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-n"
        } else {
            "ctrl-n"
        });
        view.update_in(cx, |view, window, cx| {
            assert!(!view.settings_open);
            assert!(view.presenter.model().selected_task.is_none());
            assert!(
                view.prompt_input
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
            assert_eq!(view.prompt_input.read(cx).value(), "Keep this draft");
        });
    }

    #[gpui::test]
    fn trackpad_preserves_small_diagonal_deltas_and_momentum(cx: &mut TestAppContext) {
        let (view, cx) = scroll_test_view(cx);
        let handles = view.read_with(cx, |view, _| {
            [view.sidebar_scroll.clone(), view.timeline_scroll.clone()]
        });
        for reduced_motion in [true, false] {
            for scroll in &handles {
                scroll.set_offset(point(px(0.), px(-100.)));
                view.update_in(cx, |view, window, cx| {
                    view.set_appearance(
                        AppearanceSettings {
                            reduced_motion,
                            ..view.presenter.model().appearance
                        },
                        window,
                        cx,
                    );
                });
                cx.run_until_parked();
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                for (dx, dy, phase) in [
                    (0.9, -0.5, gpui::TouchPhase::Started),
                    (0.1, -0.25, gpui::TouchPhase::Moved),
                    (0.1, 0.125, gpui::TouchPhase::Moved),
                    (0., -0.0625, gpui::TouchPhase::Ended),
                    (0., -0.03125, gpui::TouchPhase::Moved),
                    (3., 0., gpui::TouchPhase::Moved),
                ] {
                    let before = scroll.offset();
                    cx.simulate_event(ScrollWheelEvent {
                        position: scroll.bounds().center(),
                        delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
                        touch_phase: phase,
                        ..Default::default()
                    });
                    cx.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                    assert_eq!(scroll.offset(), point(before.x, before.y + px(dy)));
                }
            }
        }
    }

    #[gpui::test]
    fn wheel_smoothing_preserves_native_trackpad_input(cx: &mut TestAppContext) {
        let (view, cx) = scroll_test_view(cx);
        let scroll = view.read_with(cx, |view, _| view.sidebar_scroll.clone());
        let marker = view.read_with(cx, |view, _| {
            format!("sidebar-task-{}", view.presenter.model().tasks[3].id).leak()
        });
        let marker_y = cx.debug_bounds(marker).unwrap().top();
        let event = ScrollWheelEvent {
            position: scroll.bounds().center(),
            delta: ScrollDelta::Lines(point(0., -3.)),
            ..Default::default()
        };
        cx.simulate_event(event.clone());
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let native_target = scroll.offset().y;
        assert!(native_target < px(0.));
        assert_eq!(
            cx.debug_bounds(marker).unwrap().top(),
            marker_y + native_target
        );
        scroll.set_offset(point(px(0.), px(0.)));
        view.update_in(cx, |view, window, cx| {
            view.set_appearance(
                AppearanceSettings {
                    reduced_motion: false,
                    ..view.presenter.model().appearance
                },
                window,
                cx,
            );
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.simulate_event(event.clone());
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(scroll.offset().y > native_target);
        assert!(scroll.offset().y <= px(0.));
        std::thread::sleep(Duration::from_millis(160));
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        assert_eq!(scroll.offset().y, native_target);

        cx.simulate_event(event.clone());
        let before = scroll.offset().y;
        cx.simulate_event(ScrollWheelEvent {
            position: scroll.bounds().center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-7.5))),
            touch_phase: gpui::TouchPhase::Started,
            ..Default::default()
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert_eq!(scroll.offset().y, before - px(7.5));
        let after = scroll.offset();
        std::thread::sleep(Duration::from_millis(160));
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        assert_eq!(scroll.offset(), after);

        cx.simulate_event(event.clone());
        let reversal_from = scroll.offset().y;
        cx.simulate_event(ScrollWheelEvent {
            delta: ScrollDelta::Lines(point(0., 1.)),
            ..event.clone()
        });
        std::thread::sleep(Duration::from_millis(160));
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        assert_eq!(
            scroll.offset().y,
            (reversal_from - native_target / 3.).min(px(0.))
        );

        cx.simulate_event(event);
        let manual_offset = point(px(0.), -scroll.max_offset().y / 2.);
        scroll.set_offset(manual_offset);
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        assert_eq!(scroll.offset(), manual_offset);
    }
}
