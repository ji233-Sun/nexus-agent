use super::*;

#[test]
fn attachments_survive_send_failure_queue_and_conversation_switch() {
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=").unwrap();
    let (mut presenter, runner, directory) = fixture();
    presenter.attach_pdf_capture("报告.pdf", 12, &png).unwrap();
    presenter.attach_pdf_capture("报告.pdf", 13, &png).unwrap();
    presenter.attach_pdf_capture("报告.pdf", 14, &png).unwrap();
    presenter.remove_attachment(1);
    let source = directory.path().join("说明.txt");
    fs::write(&source, "original contents").unwrap();
    let (conversation, root) = presenter.begin_attachment_import(1).unwrap();
    assert!(!presenter.submit("wait for import", "claude"));
    assert!(presenter.attach_pdf_capture("报告.pdf", 1, &png).is_err());
    let file = crate::infrastructure::attachments::import_attachment(&root, &source).unwrap();
    presenter.finish_attachment_import(conversation, vec![Ok(file.clone())]);
    let images = presenter.model.conversation.attachments.clone();
    assert_eq!(
        images.iter().map(|image| image.page).collect::<Vec<_>>(),
        vec![Some(12), Some(14), None]
    );
    runner.0.borrow_mut().fail_send = true;
    assert!(!presenter.submit("解释圈出的部分", "claude"));
    assert_eq!(presenter.model.conversation.attachments, images);
    runner.0.borrow_mut().fail_send = false;
    assert!(presenter.submit(" ", "claude"));
    assert_eq!(last_start(&runner).prompt, "请查看所附文件。");
    assert!(presenter.model.conversation.attachments.is_empty());
    assert_eq!(last_start(&runner).attachments, images);
    let run_id = presenter.model.conversation.active_run.unwrap();
    let task_id = presenter.model.conversation.selected_task.unwrap();
    runner.emit(Event::RunSessionStarted {
        run_id,
        session_id: "pdf-session".into(),
    });
    presenter.drain_events();
    presenter.attach_pdf_capture("报告.pdf", 13, &png).unwrap();
    assert!(presenter.restore_attachments(std::slice::from_ref(&file)));
    assert!(presenter.submit("比较下一页", "claude"));
    let queued = presenter.model.conversation.queued_messages[0].clone();
    assert_eq!(queued.attachments[0].page, Some(13));
    assert!(!presenter.steer_queued_message(queued.id));
    presenter.new_task();
    assert!(presenter.model.conversation.attachments.is_empty());
    runner.emit(Event::RunExited {
        run_id,
        status: RunStatus::Failed,
        exit_code: Some(1),
    });
    presenter.drain_events();
    presenter.select_task(task_id);
    assert_eq!(presenter.model.conversation.messages[0].attachments, images);
    assert!(presenter.send_queued_message(queued.id));
    assert_eq!(last_start(&runner).attachments[0].page, Some(13));
    assert_eq!(last_start(&runner).attachments[1], file);
    assert_eq!(
        last_start(&runner).session_id.as_deref(),
        Some("pdf-session")
    );
}

#[test]
fn attachment_import_reports_failures_and_returns_to_the_owning_conversation() {
    let (mut presenter, runner, directory) = fixture();
    assert!(presenter.submit("start", "claude"));
    let task_id = presenter.model.conversation.selected_task.unwrap();
    let (conversation, root) = presenter.begin_attachment_import(2).unwrap();
    assert!(presenter.begin_attachment_import(1).is_none());
    let source = directory.path().join("notes.txt");
    fs::write(&source, "notes").unwrap();
    let file = crate::infrastructure::attachments::import_attachment(&root, &source).unwrap();
    presenter.new_task();
    presenter.finish_attachment_import(
        conversation,
        vec![Ok(file.clone()), Err("missing file".into())],
    );
    assert!(presenter.model.conversation.attachments.is_empty());
    presenter.select_task(task_id);
    assert_eq!(presenter.model.conversation.attachments, vec![file.clone()]);
    assert!(!presenter.model.conversation.attachments_loading);
    assert!(presenter.model.conversation.attachment_error.is_some());
    assert!(!presenter.restore_attachments(&vec![file; nexus_domain::Attachment::MAX_COUNT]));
    assert!(last_start(&runner).attachments.is_empty());
}
