use super::*;

impl NexusView {
    pub(super) fn render_runtime_log(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.presenter.model();
        let locale = model.language;
        let colors = palette(cx);
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(rgb(colors.muted))
                    .child(locale.text("仅保留本次启动的运行日志，重启后清空。最新记录在前。")),
            )
            .when(model.runtime_log.is_empty(), |element| {
                element.child(locale.text("暂无运行日志。"))
            })
            .children(
                model
                    .runtime_log
                    .iter()
                    .enumerate()
                    .rev()
                    .map(|(index, entry)| {
                        div()
                            .debug_selector(move || format!("runtime-log-entry-{index}"))
                            .min_w_0()
                            .p_4()
                            .rounded(px(CARD_RADIUS))
                            .border_1()
                            .border_color(rgb(colors.border))
                            .bg(rgb(colors.elevated))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .font_family(mono_font(cx))
                                    .text_size(px(12.))
                                    .text_color(rgb(colors.muted))
                                    .child(
                                        entry.timestamp.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(rgb(colors.text))
                                    .child(entry.message.render(locale).to_owned()),
                            )
                    }),
            )
    }

    pub(super) fn render_archived_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.presenter.model().language;
        let colors = palette(cx);
        let model = self.presenter.model();
        let active_run = model.active_run_count() > 0;
        let archived_count = model.archived_tasks.len();
        let archived_rows = if model.archived_tasks.is_empty() {
            vec![settings_row(
                colors,
                locale.text("暂无归档对话"),
                locale.text("从工作区侧栏的对话菜单中可以归档对话。"),
                div().text_size(px(12.)).child(locale.text("空")),
            )]
        } else {
            model
                .archived_tasks
                .iter()
                .map(|task| {
                    let task_id = task.id;
                    let app = cx.entity().clone();
                    let restore_app = app.clone();
                    let project_name = model
                        .projects
                        .iter()
                        .find(|project| Some(project.id) == task.project_id)
                        .map(|project| project.display_name.clone())
                        .unwrap_or_else(|| {
                            locale
                                .text(if task.project_id.is_none() {
                                    "未关联项目"
                                } else {
                                    "未知项目"
                                })
                                .into()
                        });
                    settings_row(
                        colors,
                        task.title.clone(),
                        project_name,
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new((ElementId::from(task_id), "restore-archived"))
                                    .debug_selector(move || format!("restore-archived-{task_id}"))
                                    .outline()
                                    .small()
                                    .h(px(CONTROL_HEIGHT))
                                    .icon(IconName::Undo)
                                    .label(locale.text("取消归档"))
                                    .disabled(active_run)
                                    .on_click(move |_, _, cx| {
                                        restore_app
                                            .update(cx, |app, cx| app.restore_task(task_id, cx));
                                    }),
                            )
                            .child(
                                Button::new((ElementId::from(task_id), "delete-archived"))
                                    .debug_selector(move || format!("delete-archived-{task_id}"))
                                    .danger()
                                    .outline()
                                    .small()
                                    .size(px(CONTROL_HEIGHT))
                                    .icon(IconName::Delete)
                                    .tooltip(locale.text("永久删除"))
                                    .disabled(active_run)
                                    .on_click(move |_, window, cx| {
                                        app.update(cx, |app, cx| {
                                            app.confirm_delete_task(task_id, window, cx)
                                        });
                                    }),
                            ),
                    )
                })
                .collect()
        };

        div()
            .flex()
            .flex_col()
            .gap_8()
            .child(settings_group(
                colors,
                locale.text("归档管理"),
                [settings_row(
                    colors,
                    locale.text("清空归档"),
                    locale.format(
                        "当前共有 {archived_count} 个归档对话。此操作会永久删除其全部记录。",
                        &[("archived_count", (archived_count).to_string())],
                    ),
                    Button::new("delete-all-archived")
                        .debug_selector(|| "delete-all-archived".into())
                        .danger()
                        .outline()
                        .small()
                        .h(px(CONTROL_HEIGHT))
                        .icon(IconName::Delete)
                        .label(locale.text("清空全部"))
                        .disabled(active_run || archived_count == 0)
                        .on_click(cx.listener(Self::confirm_delete_archived_tasks)),
                )],
            ))
            .child(settings_group(colors, locale.text("已归档"), archived_rows))
    }
}
