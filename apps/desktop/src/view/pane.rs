use super::*;

#[derive(Clone, Copy)]
pub(super) enum PaneKind {
    Sidebar,
    Timeline,
    Settings,
}

pub(super) struct WorkspacePane {
    owner: gpui::WeakEntity<NexusView>,
    kind: PaneKind,
    #[cfg(test)]
    pub(super) render_count: usize,
}

impl WorkspacePane {
    pub(super) fn new(
        owner: gpui::WeakEntity<NexusView>,
        kind: PaneKind,
        cx: &mut Context<Self>,
    ) -> Self {
        if let Some(owner) = owner.upgrade() {
            cx.observe(&owner, |_, _, cx| cx.notify()).detach();
        }
        Self {
            owner,
            kind,
            #[cfg(test)]
            render_count: 0,
        }
    }
}

impl Render for WorkspacePane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.render_count += 1;
        }
        let content = self
            .owner
            .update(cx, |owner, cx| match self.kind {
                PaneKind::Sidebar => owner.render_sidebar(cx).into_any_element(),
                PaneKind::Timeline => owner.render_timeline(window, cx).into_any_element(),
                PaneKind::Settings => owner.render_settings(cx).into_any_element(),
            })
            .ok();
        div()
            .id("pane-boundary")
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .children(content)
    }
}
