use super::*;

#[gpui::test]
fn user_ask_panel_keeps_drafts_scoped_and_submits_each_answer_once(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, runner, _directory) = fixture();

    assert!(presenter.submit("other task", "claude"));
    let other_task = presenter.model().conversation.active_task.unwrap();
    let other_run = presenter.model().conversation.active_run.unwrap();
    runner.emit(Event::RunExited {
        run_id: other_run,
        status: RunStatus::Completed,
        exit_code: Some(0),
    });
    presenter.drain_events();
    presenter.new_task();
    assert!(presenter.submit("active task", "claude"));
    let active_task = presenter.model().conversation.active_task.unwrap();
    let run_id = presenter.model().conversation.active_run.unwrap();
    let request_id = Uuid::new_v4();
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    cx.simulate_resize(gpui::size(px(1040.), px(680.)));
    view.update_in(cx, |view, window, cx| {
        view.prompt_input
            .update(cx, |input, cx| input.set_value("普通消息草稿", window, cx));
    });
    runner.emit(Event::RunUserAskRequested {
        run_id,
        request_id,
        questions: user_ask_ui_questions(),
    });
    view.update_in(cx, |view, _, cx| {
        view.poll_events(Instant::now(), cx);
        cx.notify();
    });
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });

    let stack = cx.debug_bounds("user-ask-stack").unwrap();
    let composer = cx.debug_bounds("composer-surface").unwrap();
    assert!(stack.bottom() <= composer.top());
    assert!(stack.size.height <= px(680. * 0.36 + 1.));

    let option = format!("user-ask-option-{request_id}-target-workspace").leak();
    click_debug(cx, option);
    let next = format!("user-ask-next-{request_id}").leak();
    click_debug(cx, next);
    cx.run_until_parked();
    for option_id in ["tests", "clippy"] {
        let option = format!("user-ask-option-{request_id}-checks-{option_id}").leak();
        click_debug(cx, option);
    }
    click_debug(cx, next);
    cx.run_until_parked();
    let focused = format!("user-ask-option-{request_id}-scope-focused").leak();
    click_debug(cx, focused);
    view.update_in(cx, |view, window, cx| {
        view.user_ask_inputs
            .get(&(request_id, "scope".into()))
            .unwrap()
            .update(cx, |input, cx| input.focus(window, cx));
    });
    cx.simulate_input("整个工作区");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(runner.submitted_user_ask_answers().is_empty());
    view.update_in(cx, |view, _, cx| {
        view.select_user_ask_question(request_id, 3, cx)
    });
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    view.update_in(cx, |view, window, cx| {
        view.user_ask_inputs
            .get(&(request_id, "note".into()))
            .unwrap()
            .update(cx, |input, cx| input.focus(window, cx));
    });
    cx.simulate_input("保持精简");
    cx.run_until_parked();

    view.update_in(cx, |view, window, cx| view.toggle_settings(window, cx));
    view.update_in(cx, |view, window, cx| view.toggle_settings(window, cx));
    view.update_in(cx, |view, window, cx| {
        view.select_task(other_task, window, cx)
    });
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("user-ask-stack").is_none());
    view.update_in(cx, |view, window, cx| {
        view.select_task(active_task, window, cx)
    });
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("user-ask-stack").is_some());
    view.read_with(cx, |view, cx| {
        assert_eq!(view.prompt_input.read(cx).value(), "普通消息草稿");
        let request = &view.presenter.model().conversation.pending_user_asks[0];
        assert_eq!(
            request.drafts.get("scope"),
            Some(&UserAskAnswerValue::Text("整个工作区\n".into()))
        );
        assert_eq!(
            request.drafts.get("note"),
            Some(&UserAskAnswerValue::Text("保持精简".into()))
        );
    });

    let collapse = format!("user-ask-collapse-{request_id}").leak();
    click_debug(cx, collapse);
    cx.run_until_parked();
    assert!(
        cx.debug_bounds(&*format!("user-ask-question-{request_id}-note").leak())
            .is_none()
    );
    click_debug(cx, collapse);
    cx.run_until_parked();

    let submit = format!("user-ask-submit-{request_id}").leak();
    let submit_center = cx.debug_bounds(submit).unwrap().center();
    cx.simulate_click(submit_center, Default::default());
    cx.simulate_click(submit_center, Default::default());
    cx.run_until_parked();
    let submissions = runner.submitted_user_ask_answers();
    assert_eq!(submissions.len(), 1);
    assert_eq!(
        submissions[0]
            .iter()
            .map(|answer| (answer.question_id.as_str(), answer.value.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                "target",
                UserAskAnswerValue::Selected(vec!["workspace".into()])
            ),
            (
                "checks",
                UserAskAnswerValue::Selected(vec!["tests".into(), "clippy".into()])
            ),
            ("scope", UserAskAnswerValue::Text("整个工作区\n".into())),
            ("note", UserAskAnswerValue::Text("保持精简".into())),
        ]
    );

    runner.emit(Event::RunUserAskAnswerRejected {
        run_id,
        request_id,
        message: "原生请求暂不可用".into(),
    });
    view.update_in(cx, |view, _, cx| {
        view.poll_events(Instant::now(), cx);
        cx.notify();
    });
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    assert!(
        cx.debug_bounds(&*format!("user-ask-error-{request_id}").leak())
            .is_some()
    );
    view.update_in(cx, |view, _, cx| view.submit_user_ask(request_id, cx));
    assert_eq!(runner.submitted_user_ask_answers().len(), 2);

    runner.emit(Event::RunUserAskAnswerSent { run_id, request_id });
    view.update_in(cx, |view, _, cx| {
        view.poll_events(Instant::now(), cx);
        cx.notify();
    });
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    view.update_in(cx, |view, _, cx| view.submit_user_ask(request_id, cx));
    assert_eq!(runner.submitted_user_ask_answers().len(), 2);
    runner.emit(Event::RunUserAskFinished {
        run_id,
        request_id,
        status: nexus_domain::UserAskStatus::Answered,
        message: None,
    });
    view.update_in(cx, |view, _, cx| {
        view.poll_events(Instant::now(), cx);
        cx.notify();
    });
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("user-ask-stack").is_none());
    assert!(view.read_with(cx, |view, _| view.user_ask_inputs.is_empty()));
}

#[gpui::test]
fn user_ask_panel_preserves_native_question_ids_and_option_labels(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("native questions", "claude"));
    let run_id = presenter.model().conversation.active_run.unwrap();
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    cx.simulate_resize(gpui::size(px(1040.), px(680.)));
    // Claude keys answers by full question text, Codex by explicit ID, OMP by dialog ID.
    for (question_id, choice) in [("Which checks?", true), ("checks", true), ("ui_1", false)] {
        let request_id = Uuid::new_v4();
        runner.emit(Event::RunUserAskRequested {
            run_id,
            request_id,
            questions: vec![UserAskQuestion {
                id: question_id.into(),
                prompt: "Which checks?".into(),
                answer_mode: if choice {
                    UserAskAnswerMode::Choice {
                        multiple: false,
                        allow_custom: true,
                    }
                } else {
                    UserAskAnswerMode::Text
                },
                options: if choice {
                    vec![UserAskOption {
                        id: "Run tests".into(),
                        label: "Run tests".into(),
                        description: Some("Verify the change".into()),
                    }]
                } else {
                    vec![]
                },
            }],
        });
        view.update_in(cx, |view, _, cx| view.poll_events(Instant::now(), cx));
        cx.run_until_parked();
        let question_selector = format!("user-ask-question-{request_id}-{question_id}").leak();
        assert!(cx.debug_bounds(question_selector).is_some());
        let stack = cx.debug_bounds("user-ask-stack").unwrap();
        assert!(stack.bottom() <= cx.debug_bounds("composer-surface").unwrap().top());
        if choice {
            click_debug(
                cx,
                format!("user-ask-option-{request_id}-{question_id}-Run tests").leak(),
            );
        } else {
            view.update_in(cx, |view, window, cx| {
                view.user_ask_inputs
                    .get(&(request_id, question_id.into()))
                    .unwrap()
                    .update(cx, |input, cx| input.focus(window, cx));
            });
            cx.simulate_input("Custom answer");
        }
        cx.run_until_parked();
        let count = runner.submitted_user_ask_answers().len();
        assert!(view.read_with(cx, |view, _| view.presenter.can_submit_user_ask(request_id)));
        // Choice + custom text exceeds the panel cap at the minimum window height.
        // Scroll the panel, rather than clicking the clipped button's layout bounds.
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: gpui::point(stack.left() + px(4.), stack.center().y),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-400.))),
            ..Default::default()
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        let submit_selector = format!("user-ask-submit-{request_id}").leak();
        let submit_bounds = cx.debug_bounds(submit_selector).unwrap();
        assert!(
            submit_bounds.bottom() <= stack.bottom(),
            "submit={submit_bounds:?}, stack={stack:?}"
        );
        click_debug(cx, submit_selector);
        cx.run_until_parked();
        let submissions = runner.submitted_user_ask_answers();
        assert_eq!(submissions.len(), count + 1);
        assert_eq!(
            submissions.last().unwrap(),
            &vec![nexus_domain::UserAskAnswer {
                question_id: question_id.into(),
                value: if choice {
                    UserAskAnswerValue::Selected(vec!["Run tests".into()])
                } else {
                    UserAskAnswerValue::Text("Custom answer".into())
                },
            }]
        );
        runner.emit(Event::RunUserAskAnswerSent { run_id, request_id });
        runner.emit(Event::RunUserAskFinished {
            run_id,
            request_id,
            status: nexus_domain::UserAskStatus::Answered,
            message: None,
        });
        view.update_in(cx, |view, _, cx| view.poll_events(Instant::now(), cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("user-ask-stack").is_none());
    }
}

#[gpui::test]
fn user_ask_panel_clamps_long_content_above_the_composer(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, runner, _directory) = fixture();
    assert!(presenter.submit("long question", "claude"));
    let run_id = presenter.model().conversation.active_run.unwrap();
    let request_id = Uuid::new_v4();
    runner.emit(Event::RunUserAskRequested {
        run_id,
        request_id,
        questions: vec![UserAskQuestion {
            id: "long".into(),
            prompt: "这是一个用于验证最小窗口布局的很长问题。".repeat(18),
            answer_mode: UserAskAnswerMode::Choice {
                multiple: false,
                allow_custom: false,
            },
            options: vec![UserAskOption {
                id: "long-option".into(),
                label: "这个选项同样很长，需要在面板内完整换行并保持可滚动。".repeat(14),
                description: Some("补充说明不能越过面板边界或覆盖输入区。".repeat(12)),
            }],
        }],
    });
    presenter.drain_events();
    let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));
    for (width, height, theme, glass) in [
        (1040., 680., ThemePreference::Light, true),
        (1280., 800., ThemePreference::Dark, false),
    ] {
        cx.simulate_resize(gpui::size(px(width), px(height)));
        view.update_in(cx, |view, window, cx| {
            view.set_appearance(
                AppearanceSettings {
                    theme,
                    glass,
                    reduced_motion: true,
                },
                window,
                cx,
            );
        });
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
        let stack = cx.debug_bounds("user-ask-stack").unwrap();
        let composer = cx.debug_bounds("composer-surface").unwrap();
        let stop = cx.debug_bounds("composer-cancel").unwrap();
        let question = cx
            .debug_bounds(&*format!("user-ask-question-{request_id}-long").leak())
            .unwrap();
        assert!(stack.left() >= px(0.) && stack.right() <= px(width));
        assert!(stack.size.height <= px(height * 0.36 + 1.));
        assert!(stack.bottom() <= composer.top());
        assert!(composer.bottom() <= px(height));
        assert!(stop.bottom() <= composer.bottom());
        assert!(question.left() >= stack.left() && question.right() <= stack.right());
    }
}

#[gpui::test]
fn approval_dialog_renders_options_returns_the_choice_and_closes_on_resolution(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let (mut presenter, runner, _directory) = fixture();
    // Keep the dialog's slide-in animation from moving click targets between frames.
    assert!(presenter.set_appearance(AppearanceSettings {
        reduced_motion: true,
        ..Default::default()
    }));
    assert!(presenter.submit("approval task", "claude"));
    let run_id = presenter.model().conversation.active_run.unwrap();
    let (root, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| NexusView::new(presenter, window, cx));
        gpui_kit::component::Root::new(view, window, cx)
    });
    let view = root.read_with(cx, |root, _| {
        root.view().clone().downcast::<NexusView>().unwrap()
    });
    cx.simulate_resize(gpui::size(px(1040.), px(680.)));
    let request_id = Uuid::new_v4();
    runner.emit(nexus_protocol::Event::RunApprovalRequested {
        run_id,
        request: nexus_protocol::ApprovalRequest {
            request_id,
            title: "Bash".into(),
            details: "echo approved\n".repeat(100),
            options: vec!["Approve".into(), "Deny".into()],
        },
    });
    view.update_in(cx, |view, window, cx| {
        view.poll_events(Instant::now(), cx);
        view.sync_approval_dialog(window, cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("dialog-layer").is_some());
    let details = cx.debug_bounds("approval-details").unwrap();
    assert!(details.size.height <= px(680. * 0.45));
    assert!(details.left() >= px(0.) && details.right() <= px(1040.));
    let approve = cx.debug_bounds("approval-option-0").unwrap();
    assert!(approve.bottom() <= px(680.));
    view.update_in(cx, |view, window, cx| {
        assert!(!view.prompt_input.focus_handle(cx).is_focused(window));
    });
    cx.simulate_keystrokes("enter escape");
    assert!(view.read_with(cx, |view, _| {
        view.presenter
            .model()
            .conversation
            .responding_approval
            .is_none()
    }));
    click_debug(cx, "approval-option-0");
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |view, _| view
            .presenter
            .model()
            .conversation
            .responding_approval),
        Some(request_id)
    );
    runner.emit(nexus_protocol::Event::RunApprovalResolved { run_id, request_id });
    view.update_in(cx, |view, window, cx| {
        view.poll_events(Instant::now(), cx);
        view.sync_approval_dialog(window, cx);
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("dialog-layer").is_none());
}
