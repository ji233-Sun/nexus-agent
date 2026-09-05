use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, px, rgb, rgba};

pub(super) const CANVAS: u32 = 0x0b0c0f;
pub(super) const SURFACE: u32 = 0x14161b;
pub(super) const RECESSED: u32 = 0x1c1f26;
pub(super) const HOVER: u32 = 0x242a35;
pub(super) const SELECTED: u32 = 0x1c3156;
pub(super) const BORDER: u32 = 0x2a2e38;
pub(super) const TEXT: u32 = 0xf0f2f7;
pub(super) const TEXT_SECONDARY: u32 = 0xb9c0ce;
pub(super) const MUTED: u32 = 0x8892a5;
pub(super) const ACCENT: u32 = 0x619cff;
pub(super) const ACCENT_HOVER: u32 = 0x85b3ff;
pub(super) const LINK: u32 = 0x78acff;
pub(super) const SUCCESS: u32 = 0x619cff;
pub(super) const WARNING: u32 = 0xffbd66;
pub(super) const DANGER: u32 = 0xff6b70;
pub(super) const TOOL: u32 = 0xe5a65f;

pub(super) const CONTROL_HEIGHT: f32 = 32.;
pub(super) const COMPACT_CONTROL_HEIGHT: f32 = 28.;
pub(super) const HEADER_HEIGHT: f32 = 56.;

pub(super) const MONO_FONT: &str = if cfg!(target_os = "macos") {
    "SF Mono"
} else if cfg!(target_os = "windows") {
    "Consolas"
} else {
    "DejaVu Sans Mono"
};

pub(crate) fn configure_theme(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    let theme = Theme::global_mut(cx);
    theme.font_family = ".SystemUIFont".into();
    theme.font_size = px(14.);
    theme.mono_font_family = MONO_FONT.into();
    theme.mono_font_size = px(13.);
    theme.radius = px(6.);
    theme.radius_lg = px(12.);
    theme.shadow = true;

    theme.background = rgb(CANVAS).into();
    theme.foreground = rgb(TEXT).into();
    theme.border = rgb(BORDER).into();
    theme.input = rgba(0xffffff1c).into();
    theme.caret = rgb(TEXT).into();
    theme.ring = rgb(ACCENT).into();
    theme.selection = rgba(0x619cff40).into();
    theme.muted = rgb(RECESSED).into();
    theme.muted_foreground = rgb(MUTED).into();
    theme.accent = rgb(HOVER).into();
    theme.accent_foreground = rgb(TEXT).into();
    theme.primary = rgb(ACCENT).into();
    theme.primary_hover = rgb(ACCENT_HOVER).into();
    theme.primary_active = rgb(0x4788ee).into();
    theme.primary_foreground = rgb(CANVAS).into();
    theme.secondary = rgb(RECESSED).into();
    theme.secondary_hover = rgb(HOVER).into();
    theme.secondary_active = rgb(SELECTED).into();
    theme.secondary_foreground = rgb(TEXT_SECONDARY).into();
    theme.link = rgb(LINK).into();
    theme.link_hover = rgb(0x9cc2ff).into();
    theme.link_active = rgb(0x4788ee).into();
    theme.danger = rgb(DANGER).into();
    theme.warning = rgb(WARNING).into();
    theme.success = rgb(SUCCESS).into();
    theme.danger_hover = rgb(0xff8286).into();
    theme.danger_active = rgb(0xe4565c).into();
    theme.danger_foreground = rgb(TEXT).into();
    theme.popover = rgb(RECESSED).into();
    theme.popover_foreground = rgb(TEXT).into();
    theme.overlay = rgba(0x00000066).into();
    theme.title_bar = rgb(CANVAS).into();
    theme.title_bar_border = rgba(0xffffff00).into();
    theme.sidebar = rgb(SURFACE).into();
    theme.sidebar_foreground = rgb(TEXT).into();
    theme.sidebar_accent = rgb(SELECTED).into();
    theme.sidebar_accent_foreground = rgb(TEXT).into();
    theme.sidebar_border = rgb(BORDER).into();
    theme.list_active = rgb(HOVER).into();
    theme.list_active_border = rgba(0xffffff00).into();
    theme.list_hover = rgb(RECESSED).into();
    theme.button = rgb(RECESSED).into();
    theme.button_hover = rgb(HOVER).into();
    theme.button_active = rgb(SELECTED).into();
    theme.button_foreground = rgb(TEXT).into();
    theme.button_primary = theme.primary;
    theme.button_primary_hover = theme.primary_hover;
    theme.button_primary_active = theme.primary_active;
    theme.button_primary_foreground = theme.primary_foreground;
    theme.tokens = (&theme.colors).into();
    Theme::sync_base(cx);
}
