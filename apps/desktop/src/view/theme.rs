use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, px, rgb, rgba};

pub(super) const CANVAS: u32 = 0x111216;
pub(super) const SURFACE: u32 = 0x191b21;
pub(super) const RECESSED: u32 = 0x20232b;
pub(super) const HOVER: u32 = 0x292d38;
pub(super) const SELECTED: u32 = 0x303246;
pub(super) const BORDER: u32 = 0x30343f;
pub(super) const TEXT: u32 = 0xf2f3f7;
pub(super) const TEXT_SECONDARY: u32 = 0xb8becc;
pub(super) const MUTED: u32 = 0x858da0;
pub(super) const ACCENT: u32 = 0xa99bff;
pub(super) const ACCENT_HOVER: u32 = 0xbeb3ff;
pub(super) const LINK: u32 = 0x7aa9ff;
pub(super) const SUCCESS: u32 = 0x51d88a;
pub(super) const WARNING: u32 = 0xffbd66;
pub(super) const DANGER: u32 = 0xff6b70;
pub(super) const TOOL: u32 = 0xe5a65f;

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
    theme.radius = px(8.);
    theme.radius_lg = px(12.);
    theme.shadow = true;

    theme.background = rgb(CANVAS).into();
    theme.foreground = rgb(TEXT).into();
    theme.border = rgb(BORDER).into();
    theme.input = rgba(0xffffff1c).into();
    theme.caret = rgb(TEXT).into();
    theme.ring = rgb(ACCENT).into();
    theme.selection = rgba(0x9b7cff4a).into();
    theme.muted = rgb(RECESSED).into();
    theme.muted_foreground = rgb(MUTED).into();
    theme.accent = rgb(HOVER).into();
    theme.accent_foreground = rgb(TEXT).into();
    theme.primary = rgb(ACCENT).into();
    theme.primary_hover = rgb(ACCENT_HOVER).into();
    theme.primary_active = rgb(0x8968f2).into();
    theme.primary_foreground = rgb(CANVAS).into();
    theme.secondary = rgb(RECESSED).into();
    theme.secondary_hover = rgb(HOVER).into();
    theme.secondary_active = rgb(SELECTED).into();
    theme.secondary_foreground = rgb(TEXT_SECONDARY).into();
    theme.link = rgb(LINK).into();
    theme.link_hover = rgb(0x245aa6).into();
    theme.link_active = rgb(0x1e4c8e).into();
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
