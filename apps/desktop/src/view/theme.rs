use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, px, rgb, rgba};

// T3 Code's neutral dark surfaces and compact controls, adapted to GPUI Kit.
// Reference: https://github.com/pingdotgg/t3code/blob/main/apps/web/src/index.css
pub(super) const CANVAS: u32 = 0x0a0a0a;
pub(super) const SURFACE: u32 = 0x111111;
pub(super) const RECESSED: u32 = 0x191919;
pub(super) const HOVER: u32 = 0x202020;
pub(super) const SELECTED: u32 = 0x242424;
pub(super) const BORDER: u32 = 0x242424;
pub(super) const TEXT: u32 = 0xf5f5f5;
pub(super) const TEXT_SECONDARY: u32 = 0xa3a3a3;
pub(super) const MUTED: u32 = 0x828282;
pub(super) const ACCENT: u32 = 0x346bf1;
pub(super) const ACCENT_HOVER: u32 = 0x477af5;
pub(super) const LINK: u32 = 0x8aafff;
pub(super) const SUCCESS: u32 = 0x34d399;
pub(super) const WARNING: u32 = 0xfbbf24;
pub(super) const DANGER: u32 = 0xf87171;
pub(super) const TOOL: u32 = TEXT_SECONDARY;

pub(super) const CONTROL_HEIGHT: f32 = 32.;
pub(super) const COMPACT_CONTROL_HEIGHT: f32 = 28.;
pub(super) const HEADER_HEIGHT: f32 = 52.;
pub(super) const CONTROL_RADIUS: f32 = 8.;
pub(super) const CARD_RADIUS: f32 = 12.;

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
    theme.radius = px(CONTROL_RADIUS);
    theme.radius_lg = px(CARD_RADIUS);
    theme.shadow = true;

    theme.background = rgb(CANVAS).into();
    theme.foreground = rgb(TEXT).into();
    theme.border = rgb(BORDER).into();
    theme.input = rgba(0xffffff14).into();
    theme.caret = rgb(TEXT).into();
    theme.ring = rgb(ACCENT).into();
    theme.selection = rgba(0x346bf140).into();
    theme.muted = rgb(RECESSED).into();
    theme.muted_foreground = rgb(MUTED).into();
    theme.accent = rgb(HOVER).into();
    theme.accent_foreground = rgb(TEXT).into();
    theme.primary = rgb(ACCENT).into();
    theme.primary_hover = rgb(ACCENT_HOVER).into();
    theme.primary_active = rgb(0x2d5ed4).into();
    theme.primary_foreground = rgb(TEXT).into();
    theme.secondary = rgb(RECESSED).into();
    theme.secondary_hover = rgb(HOVER).into();
    theme.secondary_active = rgb(SELECTED).into();
    theme.secondary_foreground = rgb(TEXT_SECONDARY).into();
    theme.link = rgb(LINK).into();
    theme.link_hover = rgb(0xadc7ff).into();
    theme.link_active = rgb(LINK).into();
    theme.danger = rgb(DANGER).into();
    theme.warning = rgb(WARNING).into();
    theme.success = rgb(SUCCESS).into();
    theme.danger_hover = rgb(0xfca5a5).into();
    theme.danger_active = rgb(0xef4444).into();
    theme.danger_foreground = rgb(CANVAS).into();
    theme.popover = rgb(SURFACE).into();
    theme.popover_foreground = rgb(TEXT).into();
    theme.overlay = rgba(0x00000066).into();
    theme.title_bar = rgb(CANVAS).into();
    theme.title_bar_border = rgba(0xffffff00).into();
    theme.sidebar = rgb(SURFACE).into();
    theme.sidebar_foreground = rgb(TEXT).into();
    theme.sidebar_accent = rgb(SELECTED).into();
    theme.sidebar_accent_foreground = rgb(TEXT).into();
    theme.sidebar_border = rgb(BORDER).into();
    theme.colors.list = rgb(SURFACE).into();
    theme.list_active = rgb(SELECTED).into();
    theme.list_active_border = rgba(0xffffff00).into();
    theme.list_hover = rgb(HOVER).into();
    theme.scrollbar = rgba(0xffffff00).into();
    theme.scrollbar_thumb = rgba(0xffffff14).into();
    theme.scrollbar_thumb_hover = rgba(0xffffff1f).into();
    theme.switch = rgb(SELECTED).into();
    theme.switch_thumb = rgb(TEXT).into();
    theme.button = rgb(RECESSED).into();
    theme.button_hover = rgb(HOVER).into();
    theme.button_active = rgb(SELECTED).into();
    theme.button_foreground = rgb(TEXT).into();
    theme.button_primary = theme.primary;
    theme.button_primary_hover = theme.primary_hover;
    theme.button_primary_active = theme.primary_active;
    theme.button_primary_foreground = theme.primary_foreground;
    theme.button_danger = theme.danger;
    theme.button_danger_hover = theme.danger_hover;
    theme.button_danger_active = theme.danger_active;
    theme.button_danger_foreground = theme.danger_foreground;
    theme.tokens = (&theme.colors).into();
    Theme::sync_base(cx);
}
