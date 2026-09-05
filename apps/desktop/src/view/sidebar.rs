use super::*;
use crate::model::history::ThreadSummary;
use gpui_kit::component::{
    button::ButtonCustomVariant, list::ListItem, scroll::ScrollableElement as _,
};

const SIDEBAR_ROW_HEIGHT: f32 = 36.;
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
        .rounded(px(8.))
        .text_size(px(15.))
        .child(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(10.))
                .when_some(icon, |row, icon| row.child(Icon::new(icon).size(px(18.))))
                .child(div().flex_1().min_w_0().truncate().child(title.into())),
        )
}

impl NexusView {
    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            .gap(px(16.))
            .children(model.projects.iter().map(|project| {
                let selected = selected_project_id == Some(project.id);
                let open = selected && !self.collapsed_projects.contains(&project.id);
                let project_id = project.id;
                let project = project.clone();
                let disclosure_project = project.clone();
                let tasks: Vec<_> = model
                    .tasks
                    .iter()
                    .filter(|task| matches_search(&task.title, &query))
                    .map(|task| {
                        let id = task.id;
                        let color = run_status_color(task.status);
                        navigation_row(id, task.title.clone(), None)
                            .selected(
                                model.selected_task == Some(id)
                                    && model.selected_codex_thread.is_none(),
                            )
                            .when_some(color, |row, color| {
                                row.suffix(move |_, _| {
                                    div()
                                        .absolute()
                                        .right(px(12.))
                                        .top(px(15.))
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
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        navigation_row(
                            project_id,
                            project.display_name.clone(),
                            Some(IconName::Folder),
                        )
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.select_project(project.clone());
                            app.collapsed_projects.remove(&project_id);
                            cx.notify();
                        }))
                        .suffix({
                            let app = cx.entity();
                            move |_, _| {
                                let app = app.clone();
                                let project = disclosure_project.clone();
                                Button::new((ElementId::from(project_id), "project-disclosure"))
                                    .absolute()
                                    .right(px(6.))
                                    .top(px(6.))
                                    .ghost()
                                    .small()
                                    .size(px(24.))
                                    .icon(if open {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .accessibility_label(if open {
                                        "收起项目任务"
                                    } else {
                                        "展开项目任务"
                                    })
                                    .on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        app.update(cx, |app, cx| {
                                            if !selected {
                                                app.select_project(project.clone());
                                                app.collapsed_projects.remove(&project_id);
                                            } else if !app.collapsed_projects.remove(&project_id) {
                                                app.collapsed_projects.insert(project_id);
                                            }
                                            cx.notify();
                                        });
                                    })
                            }
                        }),
                    )
                    .when(open, |group| {
                        group.child(
                            div()
                                .ml(px(24.))
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
                    })
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
                div().flex_none().px(px(16.)).pt(px(12.)).child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .pb_4()
                        .child(
                            div()
                                .px_2()
                                .py_2()
                                .text_size(px(20.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("Nexus Agent"),
                        )
                        .child(
                            Button::new("new-task")
                                .custom(
                                    ButtonCustomVariant::new(cx)
                                        .color(rgb(0x101010).into())
                                        .foreground(rgb(0xf0f0f0).into())
                                        .hover(rgb(0x252525).into())
                                        .active(rgb(0x303030).into()),
                                )
                                .small()
                                .w_full()
                                .h(px(SIDEBAR_ROW_HEIGHT))
                                .text_size(px(15.))
                                .accessibility_label("新建任务")
                                .child(
                                    div()
                                        .text_size(px(15.))
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
                                .text_size(px(15.))
                                .prefix(Icon::new(IconName::Search).small())
                                .cleanable(true),
                        ),
                ),
            )
            .child(
                div()
                    .id("sidebar-navigation")
                    .flex_1()
                    .min_h_0()
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
                                    .text_size(px(14.))
                                    .text_color(rgb(MUTED))
                                    .child("项目空间"),
                            )
                            .child(projects)
                            .child(
                                navigation_row("add-project", "添加本地项目", Some(IconName::Plus))
                                    .mt(px(8.))
                                    .on_click(cx.listener(Self::choose_project)),
                            )
                            .child(
                                div()
                                    .mt(px(24.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        navigation_row(
                                            "codex-history-disclosure",
                                            "Codex 最近会话",
                                            Some(IconName::FileText),
                                        )
                                        .suffix(move |_, _| {
                                            Icon::new(if history_open {
                                                IconName::ChevronDown
                                            } else {
                                                IconName::ChevronRight
                                            })
                                            .size(px(14.))
                                            .absolute()
                                            .right(px(12.))
                                            .top(px(11.))
                                        })
                                        .on_click(
                                            cx.listener(|app, _, _, cx| {
                                                app.codex_history_open = !app.codex_history_open;
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .when(history_open, |group| {
                                        group.child(
                                            div()
                                                .ml(px(24.))
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
                                        )
                                    }),
                            ),
                    )
                    .overflow_y_scrollbar(),
            )
            .child(
                div().flex_none().px(px(16.)).pb(px(12.)).child(
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
                                .text_size(px(15.))
                                .accessibility_label("环境与偏好")
                                .child(
                                    div()
                                        .text_size(px(15.))
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
