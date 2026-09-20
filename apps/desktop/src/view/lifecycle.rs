//! Window and application lifecycle.
//!
//! The main window owns the presenter and every task it runs, so window-level
//! commands are lifecycle decisions rather than plain window operations: macOS
//! keeps the session alive behind the status item, and every quit path asks
//! before interrupting running tasks.

use super::NexusView;
use gpui::{Context, PromptButton, PromptLevel, Window};
use gpui_kit as gpui;

impl NexusView {
    /// ⌘Q, the menu bar entry, the status item entry, and the Windows close
    /// request all end here.
    pub(super) fn quit_application(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let running = self.presenter.model().occupied_run_slots();
        if running == 0 {
            self.shutdown(cx);
            return;
        }
        // The status item can ask to quit while the window is hidden, and the
        // confirmation has to be readable before anything is torn down.
        #[cfg(target_os = "macos")]
        if crate::infrastructure::status_item::application_hidden() {
            self.show_window(window, cx);
        }
        let language = self.presenter.model().language;
        let message = language.format(
            "有 {count} 个任务正在运行，退出 Nexus Agent 会立即中断它们。",
            &[("count", running.to_string())],
        );
        let answer =
            window.prompt(
                PromptLevel::Critical,
                &message,
                Some(language.text(
                    "正在运行的任务会被停止；重新启动后它们会显示为已中断，历史记录仍可阅读。",
                )),
                &[
                    PromptButton::ok(language.text("退出")),
                    PromptButton::cancel(language.text("取消")),
                ],
                cx,
            );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() == Some(0) {
                let _ = this.update_in(cx, |view, _, cx| view.shutdown(cx));
            }
        })
        .detach();
    }

    /// Releases the embedded runner before the process exits, so agent
    /// processes are cancelled and reaped instead of outliving the app.
    fn shutdown(&mut self, cx: &mut Context<Self>) {
        self.presenter.shutdown();
        cx.quit();
    }

    /// Brings the main window back in front of the user.
    #[cfg(target_os = "macos")]
    pub(super) fn show_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Activating a hidden application does not order its windows back on
        // screen, so unhide first.
        crate::infrastructure::status_item::unhide_application();
        cx.activate(true);
        window.activate_window();
        window.refresh();
        self.sync_status_item_titles();
    }

    /// Restores the window only when the status item hid it. ⌘N can also be
    /// pressed while the window is visible, and that must not steal focus.
    #[cfg(target_os = "macos")]
    pub(super) fn show_window_if_hidden(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if crate::infrastructure::status_item::application_hidden() {
            self.show_window(window, cx);
        }
    }

    /// ⌘W, the menu bar entry, and a macOS close request: hide the application
    /// instead of closing the window, so background tasks keep running and the
    /// status item can restore the same session.
    #[cfg(target_os = "macos")]
    pub(super) fn hide_window(&mut self, cx: &mut Context<Self>) {
        cx.hide();
        self.sync_status_item_titles();
    }

    /// The status item entry toggles between hiding and restoring the window.
    #[cfg(target_os = "macos")]
    pub(super) fn toggle_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if crate::infrastructure::status_item::application_hidden() {
            self.show_window(window, cx);
        } else {
            self.hide_window(cx);
        }
    }

    #[cfg(target_os = "macos")]
    fn sync_status_item_titles(&self) {
        crate::infrastructure::status_item::sync_titles(self.presenter.model().language);
    }

    /// macOS never closes the main window: the close button, ⌘W, and the menu
    /// bar all hide the application, matching the status item.
    #[cfg(target_os = "macos")]
    pub(crate) fn should_close_window(&mut self, _: &mut Window, cx: &mut Context<Self>) -> bool {
        self.hide_window(cx);
        false
    }

    /// Windows follows the platform convention: Alt+F4 and the close button
    /// end the session, after confirming that running tasks will be stopped.
    #[cfg(target_os = "windows")]
    pub(crate) fn should_close_window(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.presenter.model().occupied_run_slots() == 0 {
            return true;
        }
        self.quit_application(window, cx);
        false
    }
}

/// macOS application menu. It is the only place the system picks up ⌘W and ⌘Q
/// while the window is on screen; both entries drive the same actions as the
/// status item.
#[cfg(target_os = "macos")]
pub(super) fn menu_bar(language: crate::i18n::Language) -> Vec<gpui::Menu> {
    use super::{HideWindow, NewTask, QuitApp};
    use gpui::MenuItem;

    vec![gpui::Menu::new("Nexus Agent").items([
        MenuItem::action(language.text("新建任务"), NewTask),
        MenuItem::separator(),
        MenuItem::action(language.text("隐藏主窗口"), HideWindow),
        MenuItem::separator(),
        MenuItem::action(language.text("退出 Nexus Agent"), QuitApp),
    ])]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presenter::tests::fixture;
    use crate::view::{NexusView, QuitApp, theme};
    use gpui::TestAppContext;

    #[gpui::test]
    fn quitting_while_tasks_run_confirms_before_stopping_them(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, _runner, _directory) = fixture();
        assert!(presenter.submit("Inspect the project", "claude"));
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));

        cx.dispatch_action(QuitApp);
        cx.run_until_parked();
        assert!(cx.has_pending_prompt());
        let (message, detail) = cx.pending_prompt().unwrap();
        assert_eq!(
            message,
            "有 1 个任务正在运行，退出 Nexus Agent 会立即中断它们。"
        );
        assert!(detail.starts_with("正在运行的任务会被停止"));

        cx.simulate_prompt_answer("取消");
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.presenter.model().active_run_count()),
            1
        );
    }

    #[gpui::test]
    fn quitting_without_running_tasks_needs_no_confirmation(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (presenter, _runner, _directory) = fixture();
        let (_view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));

        cx.dispatch_action(QuitApp);
        cx.run_until_parked();
        assert!(!cx.has_pending_prompt());
    }

    /// Windows closes the window according to the platform convention, so the
    /// confirmation has to appear before the tasks are interrupted.
    #[cfg(target_os = "windows")]
    #[gpui::test]
    fn closing_the_main_window_on_windows_confirms_while_tasks_run(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        cx.update(theme::configure_theme);
        let (mut presenter, _runner, _directory) = fixture();
        assert!(presenter.submit("Inspect the project", "claude"));
        let (view, cx) = cx.add_window_view(|window, cx| NexusView::new(presenter, window, cx));

        let should_close =
            view.update_in(cx, |view, window, cx| view.should_close_window(window, cx));
        cx.run_until_parked();
        assert!(!should_close);
        assert!(cx.has_pending_prompt());

        cx.simulate_prompt_answer("取消");
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.presenter.model().active_run_count()),
            1
        );
    }
}
