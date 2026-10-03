mod layout;
mod navigation;
mod scrolling;
mod settings;

use super::*;
use crate::infrastructure::storage::{NewTaskRun, Storage};
use gpui::{Bounds, Pixels, ScrollDelta, ScrollWheelEvent, TestAppContext, point};
use nexus_protocol::Event;
use std::path::Path;

fn scroll_test_view(cx: &mut TestAppContext) -> (Entity<NexusView>, &mut gpui::VisualTestContext) {
    cx.update(gpui_kit::init);
    cx.update(theme::configure_theme);
    let directory = tempfile::tempdir().unwrap();
    let mut storage = Storage::open(Path::new(":memory:")).unwrap();
    let project = storage.open_project(directory.path()).unwrap();
    let long_title = "A deliberately long task title that must stay inside compact navigation";
    for index in 0..30 {
        storage
            .create_task_run(NewTaskRun {
                attachments: &[],
                workspace_id: None,
                permission_mode: nexus_domain::PermissionMode::AutoEdit,
                task_id: None,
                project_id: Some(project.id),
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
    // Keep background CLI discovery from changing idle render counts.
    for provider in IssueProvider::ALL {
        presenter.set_issues_enabled(provider, false);
    }
    presenter.select_project(project);
    presenter.select_task(presenter.model().conversation.tasks[0].id);
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

fn painted_scrollbar_thumbs(
    cx: &mut gpui::VisualTestContext,
    viewport: Bounds<Pixels>,
) -> Vec<Bounds<Pixels>> {
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
            .collect()
    })
}

// Measures CPU input/layout/paint work; the test platform does not present GPU frames.
