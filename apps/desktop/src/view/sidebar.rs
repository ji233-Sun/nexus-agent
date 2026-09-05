use super::*;
use crate::model::history::ThreadSummary;
use gpui_kit::component::{list::ListItem, scroll::ScrollableElement as _};

const SIDEBAR_ROW_HEIGHT: f32 = CONTROL_HEIGHT;
pub(super) const HISTORY_PAGE_SIZE: usize = 10;

fn visible_history<'a>(
    threads: &'a [ThreadSummary],
    query: &str,
    limit: usize,
) -> (Vec<&'a ThreadSummary>, bool) {
    let mut matching = threads
        .iter()
        .filter(|thread| matches_search(&format!("{} {}", thread.title, thread.detail()), query));
    let visible = matching.by_ref().take(limit).collect();
    let has_more = matching.next().is_some();
    (visible, has_more)
}

fn navigation_row(
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
    icon: Option<IconName>,
) -> ListItem {
    ListItem::new(id)
        .w_full()
        .h(px(SIDEBAR_ROW_HEIGHT))
        .px(px(10.))
        .pr(px(32.))
        .py_0()
        .rounded(px(CONTROL_RADIUS))
        .text_size(px(13.))
        .child(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .when_some(icon, |row, icon| {
                    row.child(Icon::new(icon).size(px(16.)).text_color(rgb(MUTED)))
                })
                .child(div().flex_1().min_w_0().truncate().child(title.into())),
        )
}

impl NexusView {
    pub(super) fn render_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let model = self.presenter.model();
        let query = self.search_input.read(cx).value();
        let selected_project_id = model.selected_project.as_ref().map(|project| project.id);
        let history_status = if model.codex_history_loading {
            "正在读取本机会话…".to_owned()
        } else if let Some(error) = &model.codex_history_error {
            format!("历史不可用：{error}")
        } else if self.presenter.history_available() {
            format!("{} 条本机会话 · 只读浏览", model.codex_threads.len())
        } else {
            "等待检测 Codex CLI".to_owned()
        };
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
                let can_create_task = model.active_run.is_none();
                let tasks: Vec<_> = model
                    .tasks
                    .iter()
                    .filter(|task| matches_search(&task.title, &query))
                    .map(|task| {
                        let id = task.id;
                        let color = run_status_color(task.status);
                        navigation_row(id, task.title.clone(), None)
                            .debug_selector(move || format!("sidebar-task-{id}"))
                            .selected(
                                model.selected_task == Some(id)
                                    && model.selected_codex_thread.is_none(),
                            )
                            .when_some(color, |row, color| {
                                row.suffix(move |_, _| {
                                    div()
                                        .absolute()
                                        .right(px(12.))
                                        .top(px((SIDEBAR_ROW_HEIGHT - 6.) / 2.))
                                        .size(px(6.))
                                        .rounded_full()
                                        .bg(color)
                                })
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
                            project_id,
                            project.display_name.clone(),
                            Some(IconName::Folder),
                        )
                        .group("sidebar-project")
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
                                                    .text_color(rgb(TEXT)),
                                            )
                                            .accessibility_label("在此项目中新建对话")
                                            .tooltip("新建对话")
                                            .disabled(!can_create_task)
                                            .on_click(move |_, window, cx| {
                                                cx.stop_propagation();
                                                app.update(cx, |app, cx| {
                                                    if app.presenter.model().active_run.is_some() {
                                                        return;
                                                    }
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
                            .gap(px(2.))
                            .when(tasks.is_empty(), |list| {
                                list.child(
                                    navigation_row(
                                        (ElementId::from(project_id), "empty"),
                                        if query.trim().is_empty() {
                                            "开始任务后，记录会出现在这里"
                                        } else {
                                            "没有匹配的任务"
                                        },
                                        None,
                                    )
                                    .disabled(true),
                                )
                            })
                            .children(tasks),
                    )
            }));
        let (visible_threads, has_more_history) = visible_history(
            &model.codex_threads,
            &query,
            self.codex_history_visible_count,
        );
        let history: Vec<_> = visible_threads
            .into_iter()
            .map(|thread| {
                let id = thread.id.clone();
                navigation_row(
                    SharedString::from(format!("codex-{id}")),
                    thread.title.clone(),
                    None,
                )
                .selected(model.selected_codex_thread.as_deref() == Some(thread.id.as_str()))
                .on_click(cx.listener(move |app, _, _, cx| app.select_codex_thread(id.clone(), cx)))
            })
            .collect();
        let history_open = self.codex_history_open;
        let history_progress = disclosure_progress(
            "history-reveal",
            history_open,
            self.reduced_motion,
            window,
            cx,
        );
        div()
            .id("workspace-sidebar")
            .w(px(300.))
            .h_full()
            .flex_none()
            .pt(px(if cfg!(target_os = "macos") { 36. } else { 0. }))
            .bg(rgb(SURFACE))
            .border_r_1()
            .border_color(rgb(BORDER))
            .flex()
            .flex_col()
            .child(
                div().flex_none().px(px(12.)).pt(px(8.)).child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .pb_4()
                        .child(
                            div()
                                .px_2()
                                .py_2()
                                .text_size(px(14.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("Nexus Agent"),
                        )
                        .child(
                            Button::new("new-task")
                                .outline()
                                .small()
                                .w_full()
                                .h(px(SIDEBAR_ROW_HEIGHT))
                                .text_size(px(13.))
                                .accessibility_label("新建任务")
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
                                                .child(Icon::new(IconName::Plus))
                                                .child("新建任务"),
                                        )
                                        .child(div().text_xs().opacity(0.65).child(
                                            if cfg!(target_os = "macos") {
                                                "⌘ N"
                                            } else {
                                                "Ctrl N"
                                            },
                                        )),
                                )
                                .tooltip("在当前项目中开始新任务")
                                .disabled(
                                    model.selected_project.is_none() || model.active_run.is_some(),
                                )
                                .on_click(
                                    cx.listener(|app, _, window, cx| app.new_task(window, cx)),
                                ),
                        )
                        .child(
                            Input::new(&self.search_input)
                                .small()
                                .min_h(px(SIDEBAR_ROW_HEIGHT))
                                .text_size(px(13.))
                                .prefix(Icon::new(IconName::Search).small())
                                .cleanable(true),
                        ),
                ),
            )
            .child(
                div()
                    .id("sidebar-navigation")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .lock_scroll_axis()
                    .track_scroll(&self.sidebar_scroll)
                    .child(
                        div()
                            .px(px(12.))
                            .pb(px(16.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .px(px(10.))
                                    .pb(px(10.))
                                    .text_size(px(12.))
                                    .text_color(rgb(MUTED))
                                    .child("项目空间"),
                            )
                            .child(projects)
                            .child(
                                navigation_row("add-project", "添加本地项目", Some(IconName::Plus))
                                    .debug_selector(|| "add-project".into())
                                    .mt(px(8.))
                                    .on_click(cx.listener(Self::choose_project)),
                            )
                            .child(
                                gpui_kit::base::Collapsible::new()
                                    .open(history_open)
                                    .reveal("history-content", history_progress)
                                    .mt(px(24.))
                                    .flex()
                                    .flex_col()
                                    .child(
                                        navigation_row(
                                            "codex-history-disclosure",
                                            "Codex 最近会话",
                                            Some(IconName::FileText),
                                        )
                                        .suffix(move |_, _| {
                                            Icon::new(IconName::ChevronRight)
                                                .rotate(gpui::radians(
                                                    std::f32::consts::FRAC_PI_2 * history_progress,
                                                ))
                                                .size(px(14.))
                                                .absolute()
                                                .right(px(12.))
                                                .top(px((SIDEBAR_ROW_HEIGHT - 14.) / 2.))
                                        })
                                        .on_click(
                                            cx.listener(|app, _, _, cx| {
                                                app.codex_history_open = !app.codex_history_open;
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .content(
                                        div()
                                            .pt(px(4.))
                                            .opacity(history_progress)
                                            .w_full()
                                            .pl(px(24.))
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.))
                                            .when(history.is_empty(), |list| {
                                                list.child(
                                                    navigation_row(
                                                        "history-empty",
                                                        if query.trim().is_empty() {
                                                            "暂无可显示的会话"
                                                        } else {
                                                            "没有匹配的历史会话"
                                                        },
                                                        None,
                                                    )
                                                    .disabled(true),
                                                )
                                            })
                                            .children(history)
                                            .when(has_more_history, |list| {
                                                list.child(
                                                    navigation_row(
                                                        "codex-history-read-more",
                                                        "Read More",
                                                        Some(IconName::ChevronDown),
                                                    )
                                                    .text_color(rgb(MUTED))
                                                    .on_click(cx.listener(|app, _, _, cx| {
                                                        app.codex_history_visible_count +=
                                                            HISTORY_PAGE_SIZE;
                                                        cx.notify();
                                                    })),
                                                )
                                            }),
                                    ),
                            ),
                    )
                    .vertical_scrollbar(&self.sidebar_scroll),
            )
            .child(
                div().flex_none().px(px(12.)).pb(px(12.)).child(
                    div()
                        .w_full()
                        .pt_3()
                        .border_t_1()
                        .border_color(rgb(BORDER))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(px(12.))
                                        .text_color(rgb(MUTED))
                                        .line_clamp(2)
                                        .child(history_status),
                                )
                                .child(
                                    Button::new("refresh-codex-history")
                                        .ghost()
                                        .small()
                                        .size(px(COMPACT_CONTROL_HEIGHT))
                                        .icon(IconName::RotateCw)
                                        .tooltip("刷新本机 Codex 历史")
                                        .disabled(
                                            !self.presenter.history_available()
                                                || model.codex_history_loading,
                                        )
                                        .on_click(cx.listener(Self::refresh_codex_history)),
                                ),
                        )
                        .child(
                            Button::new("sidebar-environment")
                                .ghost()
                                .small()
                                .w_full()
                                .h(px(SIDEBAR_ROW_HEIGHT))
                                .text_size(px(13.))
                                .accessibility_label("环境与偏好")
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
                                                .child("环境与偏好"),
                                        )
                                        .child(status_dot(
                                            model
                                                .selected_probe()
                                                .map(|probe| {
                                                    if probe.available && probe.authenticated {
                                                        rgb(SUCCESS).into()
                                                    } else {
                                                        rgb(WARNING).into()
                                                    }
                                                })
                                                .unwrap_or_else(|| rgb(MUTED).into()),
                                        )),
                                )
                                .on_click(cx.listener(|app, _, window, cx| {
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
    use std::path::Path;

    fn scroll_test_view(
        cx: &mut TestAppContext,
    ) -> (Entity<NexusView>, &mut gpui::VisualTestContext) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let directory = tempfile::tempdir().unwrap();
        let mut storage = Storage::open(Path::new(":memory:")).unwrap();
        let project = storage.open_project(directory.path()).unwrap();
        for index in 0..30 {
            storage
                .create_task_run(NewTaskRun {
                    project_id: project.id,
                    title: if index % 2 == 0 {
                        "Hi"
                    } else {
                        "Scroll regression"
                    },
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
            view.reduced_motion = true;
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
        cx.simulate_click(
            point(
                bounds[short_index].right() - px(8.),
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
    fn project_disclosure_animates_layout_reversibly_and_can_skip_motion(cx: &mut TestAppContext) {
        let (view, cx) = scroll_test_view(cx);
        let project_id = view.read_with(cx, |view, _| {
            view.presenter.model().selected_project.as_ref().unwrap().id
        });
        let project_selector = format!("sidebar-project-{project_id}").leak();
        let project_bounds = cx.debug_bounds(project_selector).unwrap();
        let trigger = point(project_bounds.left() + px(60.), project_bounds.center().y);
        let selected_task = view.read_with(cx, |view, _| view.presenter.model().selected_task);
        view.update(cx, |view, cx| {
            view.reduced_motion = false;
            cx.notify();
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
        view.update(cx, |view, cx| {
            view.reduced_motion = true;
            cx.notify();
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
        view.update(cx, |view, cx| {
            view.settings_open = true;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let settings_scroll = view.read_with(cx, |view, _| view.settings_scroll.clone());
        let overlap = point(
            settings_scroll.bounds().left() + px(30.),
            scroll.bounds().top() + px(40.),
        );
        assert!(settings_scroll.bounds().contains(&overlap));
        assert!(scroll.bounds().contains(&overlap));
        let before = scroll.offset();
        cx.simulate_event(ScrollWheelEvent {
            position: overlap,
            delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
            ..Default::default()
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert_eq!(scroll.offset(), before);
        assert_eq!(sidebar_scroll.offset().y, px(-80.));
        assert_eq!(
            settings_scroll.offset().y,
            (-settings_scroll.max_offset().y).max(px(-40.))
        );

        view.update(cx, |view, cx| {
            view.settings_open = false;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
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
    fn trackpad_preserves_small_diagonal_deltas_and_momentum(cx: &mut TestAppContext) {
        let (view, cx) = scroll_test_view(cx);
        let handles = view.read_with(cx, |view, _| {
            [view.sidebar_scroll.clone(), view.timeline_scroll.clone()]
        });
        for reduced_motion in [true, false] {
            for scroll in &handles {
                scroll.set_offset(point(px(0.), px(-100.)));
                view.update(cx, |view, cx| {
                    view.reduced_motion = reduced_motion;
                    cx.notify();
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
        scroll.set_offset(point(px(0.), px(0.)));
        view.update(cx, |view, cx| {
            view.reduced_motion = false;
            cx.notify();
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

    fn threads(count: usize) -> Vec<ThreadSummary> {
        (0..count)
            .map(|index| ThreadSummary {
                id: index.to_string(),
                title: format!("Session {index}"),
                cwd: "/workspace/project".into(),
                source: "cli".into(),
                updated_at: 0,
                archived: false,
            })
            .collect()
    }

    #[test]
    fn history_expands_in_tens_until_all_sessions_are_visible() {
        let threads = threads(58);
        let mut limit = HISTORY_PAGE_SIZE;
        for expected_count in [10, 20, 30, 40, 50, 58] {
            let (visible, has_more) = visible_history(&threads, "", limit);
            assert_eq!(
                visible,
                threads[..expected_count].iter().collect::<Vec<_>>()
            );
            assert_eq!(has_more, expected_count < threads.len());
            limit += HISTORY_PAGE_SIZE;
        }
    }

    #[test]
    fn history_hides_read_more_when_results_fit_on_one_page() {
        for count in [0, 1, 9, 10] {
            let threads = threads(count);
            let (visible, has_more) = visible_history(&threads, "", HISTORY_PAGE_SIZE);
            assert_eq!(visible.len(), count);
            assert!(!has_more);
        }
    }

    #[test]
    fn history_search_filters_all_sessions_before_pagination() {
        let threads = threads(58);
        let (visible, has_more) = visible_history(&threads, "Session 4", HISTORY_PAGE_SIZE);
        assert_eq!(visible.len(), 10);
        assert_eq!(visible[0].id, "4");
        assert_eq!(visible[9].id, "48");
        assert!(has_more);
        let (visible, has_more) = visible_history(&threads, "Session 4", HISTORY_PAGE_SIZE * 2);
        assert_eq!(visible.len(), 11);
        assert_eq!(visible[10].id, "49");
        assert!(!has_more);
        let (visible, has_more) = visible_history(&threads, "missing", HISTORY_PAGE_SIZE);
        assert!(visible.is_empty());
        assert!(!has_more);
    }
}
