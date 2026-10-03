use super::*;

#[gpui::test]
async fn composer_pastes_attachments_without_replacing_prompt_text(cx: &mut gpui::TestAppContext) {
    use base64::Engine as _;
    cx.executor().allow_parking();
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (presenter, _, _directory) = fixture();
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    view.update_in(cx, |view, window, cx| {
        view.prompt_input
            .update(cx, |input, cx| input.set_value("保留问题", window, cx));
        view.focus_prompt(window, cx);
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("attach-files").is_some());
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a"
    } else {
        "ctrl-a"
    });
    let bytes = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=").unwrap();
    cx.update(|_, cx| {
        cx.write_to_clipboard(ClipboardItem::new_image(&gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            bytes,
        )))
    });
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-v"
    } else {
        "ctrl-v"
    });
    cx.condition(&view, |view, _| {
        !view.presenter.model().conversation.attachments_loading
    })
    .await;
    view.read_with(cx, |view, cx| {
        assert_eq!(view.prompt_input.read(cx).value(), "保留问题");
        assert_eq!(view.presenter.model().conversation.attachments.len(), 1);
        assert!(view.presenter.model().conversation.attachments[0].is_image());
    });
    cx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string("替换文字".into())));
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-v"
    } else {
        "ctrl-v"
    });
    view.read_with(cx, |view, cx| {
        assert_eq!(view.prompt_input.read(cx).value(), "替换文字");
        assert_eq!(view.presenter.model().conversation.attachments.len(), 1);
    });
}

#[gpui::test]
fn queued_message_steer_button_targets_the_message_and_waits_for_receipt(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("first", "claude"));
    assert!(presenter.submit("correction", "claude"));
    let run_id = presenter.model().conversation.active_run.unwrap();
    let message_id = presenter.model().conversation.queued_messages[0].id;
    let selector = format!("steer-queued-{message_id}").leak();
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    let button = cx
        .debug_bounds(selector)
        .expect("queued message must expose Steer")
        .center();
    cx.simulate_click(button, Default::default());
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.presenter.model().conversation.steering_message,
            Some(message_id)
        );
        assert_eq!(view.presenter.model().conversation.queued_messages.len(), 1);
    });
    runner.emit(Event::RunInputAccepted { run_id, message_id });
    view.update_in(cx, |view, _, cx| {
        view.presenter.drain_events();
        cx.notify();
    });
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds(selector).is_none());
    assert!(view.read_with(cx, |view, _| {
        view.presenter
            .model()
            .conversation
            .queued_messages
            .is_empty()
    }));
}
