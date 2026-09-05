use super::*;
use gpui::{HitboxBehavior, Pixels, ScrollWheelEvent, canvas};

const WHEEL_DURATION: Duration = Duration::from_millis(140);

struct WheelMotion {
    from: Pixels,
    target: Pixels,
    last_position: Pixels,
    started: Instant,
}

impl WheelMotion {
    fn position(&self, now: Instant) -> Pixels {
        let progress =
            (now.duration_since(self.started).as_secs_f32() / WHEEL_DURATION.as_secs_f32()).min(1.);
        self.from + (self.target - self.from) * (1. - (1. - progress).powi(3))
    }
}

#[derive(Clone, Copy)]
pub(super) enum PaneKind {
    Sidebar,
    Timeline,
    Settings,
}

pub(super) struct WorkspacePane {
    owner: gpui::WeakEntity<NexusView>,
    kind: PaneKind,
    wheel_motion: Option<WheelMotion>,
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
            wheel_motion: None,
            #[cfg(test)]
            render_count: 0,
        }
    }

    fn scroll_handle(&self, cx: &gpui::App) -> Option<(ScrollHandle, bool)> {
        self.owner.upgrade().map(|owner| {
            let owner = owner.read(cx);
            let handle = match self.kind {
                PaneKind::Sidebar => &owner.sidebar_scroll,
                PaneKind::Timeline => &owner.timeline_scroll,
                PaneKind::Settings => &owner.settings_scroll,
            };
            (handle.clone(), owner.reduced_motion || cx.reduce_motion())
        })
    }

    fn queue_wheel(&mut self, handle: &ScrollHandle, delta: Pixels, now: Instant) {
        let current = handle.offset().y;
        let target = self
            .wheel_motion
            .as_ref()
            .filter(|motion| (motion.target > current) == (delta > px(0.)))
            .map_or(current, |motion| motion.target);
        self.wheel_motion = Some(WheelMotion {
            from: current,
            target: (target + delta).clamp(-handle.max_offset().y, px(0.)),
            last_position: current,
            started: now,
        });
    }

    fn advance_wheel(&mut self, handle: &ScrollHandle, now: Instant) -> bool {
        let Some(motion) = self.wheel_motion.as_mut() else {
            return false;
        };
        let mut offset = handle.offset();
        // A scrollbar drag or programmatic jump takes precedence over wheel animation.
        if offset.y != motion.last_position {
            self.wheel_motion = None;
            return false;
        }
        offset.y = motion.position(now).clamp(-handle.max_offset().y, px(0.));
        motion.last_position = offset.y;
        handle.set_offset(offset);
        if now.duration_since(motion.started) >= WHEEL_DURATION || offset.y == motion.target {
            self.wheel_motion = None;
            false
        } else {
            true
        }
    }
}

impl Render for WorkspacePane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.render_count += 1;
        }
        let scroll = self.scroll_handle(cx);
        if let Some((handle, reduced_motion)) = &scroll {
            if *reduced_motion {
                self.wheel_motion = None;
            } else if self.advance_wheel(handle, Instant::now()) {
                window.request_animation_frame();
            }
        }
        let content = self
            .owner
            .update(cx, |owner, cx| match self.kind {
                PaneKind::Sidebar => owner.render_sidebar(window, cx).into_any_element(),
                PaneKind::Timeline => owner.render_timeline(window, cx).into_any_element(),
                PaneKind::Settings => owner.render_settings(cx).into_any_element(),
            })
            .ok();
        div()
            .id("pane-boundary")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .children(content)
            .when_some(scroll, |pane, (handle, reduced_motion)| {
                let line_height = window.line_height();
                let hit_handle = handle.clone();
                let listener = cx.listener(move |pane, event: &ScrollWheelEvent, _, cx| {
                    let delta = event.delta.pixel_delta(line_height).y;
                    if event.delta.precise() {
                        // Precise input already includes the OS trackpad momentum.
                        // Keep every vertical pixel instead of applying a second axis lock.
                        pane.wheel_motion = None;
                        if delta != px(0.) {
                            let mut offset = handle.offset();
                            let next_y = (offset.y + delta).clamp(-handle.max_offset().y, px(0.));
                            if next_y != offset.y {
                                offset.y = next_y;
                                handle.set_offset(offset);
                                cx.notify();
                            }
                            cx.stop_propagation();
                        }
                        return;
                    }
                    if reduced_motion {
                        pane.wheel_motion = None;
                        return;
                    }
                    if delta != px(0.) {
                        pane.queue_wheel(&handle, delta, Instant::now());
                        cx.notify();
                        cx.stop_propagation();
                    }
                });
                pane.child(
                    canvas(
                        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                        move |_, hitbox, window, _| {
                            // Child scroll masks run first, preserving horizontal code scrolling.
                            // Then handle pane input before native axis filtering or wheel jumps.
                            window.on_mouse_event(
                                move |event: &ScrollWheelEvent, phase, window, cx| {
                                    if phase.capture()
                                        && hitbox.should_handle_scroll(window)
                                        && hit_handle.bounds().contains(&event.position)
                                    {
                                        listener(event, window, cx);
                                    }
                                },
                            );
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
            })
    }
}
