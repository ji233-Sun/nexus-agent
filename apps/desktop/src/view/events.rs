use super::*;

impl NexusView {
    pub(super) fn start_event_pump(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                if this
                    .update_in(cx, |app, window, cx| {
                        let executable = app.presenter.model().conversation.executable.clone();
                        let untouched = app.executable_input.read(cx).value() == executable;
                        app.poll_events(Instant::now(), cx);
                        app.poll_voice_input(window, cx);
                        if untouched && app.presenter.model().conversation.executable != executable
                        {
                            app.sync_executable(window, cx);
                        }
                        app.sync_approval_dialog(window, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn sync_approval_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.presenter.model();
        let next = model.conversation.active_run.zip(
            model
                .conversation
                .pending_approvals
                .front()
                .map(|request| request.request_id),
        );
        if next == self.approval_dialog {
            return;
        }
        if self.approval_dialog.take().is_some() {
            window.close_dialog(cx);
        }
        let Some((run_id, request_id)) = next else {
            return;
        };
        self.approval_dialog = next;
        self.model_picker.open = false;
        let app = cx.entity().clone();
        window.open_dialog(cx, move |dialog, window, cx| {
            let model = app.read(cx).presenter.model();
            let locale = model.language;
            let Some(request) = model
                .conversation
                .pending_approvals
                .front()
                .filter(|request| request.request_id == request_id)
            else {
                return dialog;
            };
            let responding = model.conversation.responding_approval == Some(request_id);
            let title = model
                .conversation
                .tasks
                .iter()
                .find(|task| Some(task.id) == model.conversation.active_task)
                .map(|task| task.title.as_str())
                .unwrap_or_default();
            let mut buttons = div().flex().flex_wrap().gap_2();
            for (index, label) in request.options.iter().enumerate() {
                let app = app.clone();
                let label = match label.as_str() {
                    "Approve" => locale.text("允许本次").to_owned(),
                    "Deny" => locale.text("拒绝").to_owned(),
                    _ => label.clone(),
                };
                buttons = buttons.child(
                    Button::new(("approval-option", index))
                        .debug_selector(move || format!("approval-option-{index}"))
                        .label(label)
                        .disabled(responding)
                        .on_click(move |_, window, cx| {
                            app.update(cx, |app, cx| {
                                app.presenter
                                    .respond_approval(run_id, request_id, Some(index));
                                app.presenter.notify_remote_changed();
                                cx.notify();
                            });
                            window.refresh();
                        }),
                );
            }
            let stop_app = app.clone();
            buttons = buttons.child(
                Button::new("approval-stop")
                    .label(locale.text("停止任务"))
                    .ghost()
                    .on_click(move |_, window, cx| {
                        stop_app.update(cx, |app, cx| {
                            app.presenter.cancel();
                            app.presenter.notify_remote_changed();
                            cx.notify();
                        });
                        window.refresh();
                    }),
            );
            dialog
                .title(locale.text("需要授权"))
                .width(px(640.).min(window.viewport_size().width - px(48.)))
                .close_button(false)
                .overlay_closable(false)
                .keyboard(false)
                .on_ok(|_, _, _| false)
                .child(div().text_size(px(13.)).child(title.to_owned()))
                .child(div().child(request.title.clone()))
                .child(
                    div()
                        .id("approval-details")
                        .debug_selector(|| "approval-details".into())
                        .max_h(window.viewport_size().height * 0.45)
                        .overflow_y_scroll()
                        .text_size(px(13.))
                        .child(request.details.clone()),
                )
                .child(
                    div().text_size(px(12.)).child(
                        model
                            .conversation
                            .run_status
                            .render(model.language)
                            .to_owned(),
                    ),
                )
                .footer(buttons)
        });
    }

    pub(super) fn poll_events(&mut self, now: Instant, cx: &mut Context<Self>) {
        self.poll_pdf_events(cx);
        if self.presenter.drain_issue_events() {
            cx.notify();
        }
        if self.presenter.drain_installation_events() {
            cx.notify();
        }
        if self.presenter.drain_update_events() {
            cx.notify();
        }
        let follow_latest =
            self.timeline_scroll.max_offset().y + self.timeline_scroll.offset().y <= px(48.);
        if self.presenter.drain_events() {
            if follow_latest {
                self.timeline_scroll.scroll_to_bottom();
            }
            cx.notify();
        }
        if self.presenter.refresh_run_elapsed(now) {
            // Clock ticks are not new output: preserve scroll and skip remote broadcasts.
            self.timeline_pane.update(cx, |_, cx| cx.notify());
        }
        if self.presenter.install_update_when_idle() {
            cx.notify();
        }
        if matches!(
            self.presenter.model().updates.state,
            crate::model::updates::UpdateState::Restarting(_)
        ) {
            self.presenter.shutdown();
            cx.quit();
        }
    }
}
