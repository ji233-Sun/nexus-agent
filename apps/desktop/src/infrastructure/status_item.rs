//! macOS menu bar status item.
//!
//! GPUI owns the `NSApplication` run loop but exposes no status item API, so
//! the AppKit objects live in `status_item.m`. This module keeps the Rust side
//! of that contract: menu labels come from the i18n catalog, and every command
//! re-enters GPUI as an action so the window behaves identically whether it is
//! driven from the status item, the menu bar, or a key binding.

use crate::i18n::Language;
use crate::view::{NewTask, QuitApp, ToggleWindow};
use gpui_kit::{Action, AnyWindowHandle, AppContext as _, AsyncApp};
use std::{cell::RefCell, ffi::CString, os::raw::c_char};

/// Status item commands, in menu order.
const TOGGLE_WINDOW: i32 = 0;
const NEW_TASK: i32 = 1;
const QUIT: i32 = 2;

/// Menu bar icon: the brand mark alone, sized for an 18 pt status item.
const ICON: &[u8] = include_bytes!("../../assets/brand/nexus-status-item.png");

unsafe extern "C" {
    fn nexus_status_item_install(
        command: extern "C" fn(i32),
        toggle_window_title: *const c_char,
        new_task_title: *const c_char,
        quit_title: *const c_char,
        icon: *const u8,
        icon_length: usize,
    );
    fn nexus_status_item_set_titles(
        toggle_window_title: *const c_char,
        new_task_title: *const c_char,
        quit_title: *const c_char,
    );
    fn nexus_status_item_application_hidden() -> i32;
    fn nexus_status_item_unhide_application();
}

struct Bridge {
    app: AsyncApp,
    window: AnyWindowHandle,
}

thread_local! {
    // Menu clicks arrive on the main thread while GPUI owns the run loop, so
    // the handle back into the app never crosses threads.
    static BRIDGE: RefCell<Option<Bridge>> = const { RefCell::new(None) };
}

/// Label for the show/hide entry: it names the action the entry performs.
fn toggle_title(language: Language, hidden: bool) -> &'static str {
    if hidden {
        language.text("显示主窗口")
    } else {
        language.text("隐藏主窗口")
    }
}

/// `true` while the application is hidden: on macOS the whole app is hidden so
/// agent processes keep running behind the status item.
pub(crate) fn application_hidden() -> bool {
    unsafe { nexus_status_item_application_hidden() != 0 }
}

/// Brings a hidden application back before GPUI activates and orders the
/// window front.
pub(crate) fn unhide_application() {
    unsafe { nexus_status_item_unhide_application() }
}

/// Refreshes every status item label for `language` and the current state.
pub(crate) fn sync_titles(language: Language) {
    let titles = titles(language);
    unsafe { nexus_status_item_set_titles(titles.0.as_ptr(), titles.1.as_ptr(), titles.2.as_ptr()) }
}

/// Installs the status item for the main window: every menu click re-enters
/// GPUI through `window`, so the status item drives the same code as ⌘W and ⌘Q.
pub(crate) fn install(app: AsyncApp, window: AnyWindowHandle, language: Language) {
    let titles = titles(language);
    BRIDGE.with(|bridge| *bridge.borrow_mut() = Some(Bridge { app, window }));
    unsafe {
        nexus_status_item_install(
            dispatch,
            titles.0.as_ptr(),
            titles.1.as_ptr(),
            titles.2.as_ptr(),
            ICON.as_ptr(),
            ICON.len(),
        )
    }
}

extern "C" fn dispatch(command: i32) {
    let action: Box<dyn Action> = match command {
        TOGGLE_WINDOW => Box::new(ToggleWindow),
        NEW_TASK => Box::new(NewTask),
        QUIT => Box::new(QuitApp),
        _ => return,
    };
    BRIDGE.with(|bridge| {
        let bridge = bridge.borrow();
        let Some(bridge) = bridge.as_ref() else {
            return;
        };
        let mut app = bridge.app.clone();
        // AppKit dispatches the click from the main thread; the window stays
        // alive for the whole session, so dispatching here never races a close.
        let _ = app.update_window(bridge.window, |_, window, cx| {
            window.dispatch_action(action, cx);
        });
    });
}

fn titles(language: Language) -> (CString, CString, CString) {
    let title = |text: &str| CString::new(text).expect("menu label without NUL");
    (
        title(toggle_title(language, application_hidden())),
        title(language.text("新建任务")),
        title(language.text("退出 Nexus Agent")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_title_names_the_action_the_entry_performs() {
        for language in [Language::Chinese, Language::English] {
            let hidden = toggle_title(language, true);
            let visible = toggle_title(language, false);
            assert_ne!(hidden, visible);
            assert_eq!(hidden, language.text("显示主窗口"));
            assert_eq!(visible, language.text("隐藏主窗口"));
        }
        assert_eq!(toggle_title(Language::English, true), "Show Main Window");
        assert_eq!(toggle_title(Language::English, false), "Hide Main Window");
    }
}
