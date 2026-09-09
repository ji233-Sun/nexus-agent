use crate::model::{AppearanceSettings, ThemePreference};
use gpui_kit::component::{Theme, ThemeMode, box_shadow};
use gpui_kit::{
    App, BoxShadow, Global, Hsla, WindowAppearance, WindowBackgroundAppearance, px, rgb, rgba,
};

// Quiet chrome, a distinct reading canvas and raised controls across both themes.
#[derive(Clone, Copy)]
pub(super) struct Palette {
    pub canvas: u32,
    pub surface: u32,
    pub elevated: u32,
    pub recessed: u32,
    pub hover: u32,
    pub selected: u32,
    pub border: u32,
    pub input_border: u32,
    pub text: u32,
    pub text_secondary: u32,
    pub muted: u32,
    pub accent: u32,
    pub success: u32,
    pub warning: u32,
    pub danger: u32,
    pub diff_added: u32,
    pub diff_removed: u32,
}

impl Palette {
    pub(super) const fn for_dark(dark: bool) -> Self {
        if dark {
            Self {
                canvas: 0x1b1e24,
                surface: 0x15181d,
                elevated: 0x242830,
                recessed: 0x292f39,
                hover: 0x303846,
                selected: 0x2b3b55,
                border: 0x353d49,
                input_border: 0x8491a4,
                text: 0xeff2f7,
                text_secondary: 0xc5cdd9,
                muted: 0xa9b5c6,
                accent: 0x9abaff,
                success: 0x67d4a0,
                warning: 0xeac36b,
                danger: 0xff9b96,
                diff_added: 0x183c2e,
                diff_removed: 0x452b2d,
            }
        } else {
            Self {
                canvas: 0xfbfcfe,
                surface: 0xf0f2f6,
                elevated: 0xffffff,
                recessed: 0xe8edf4,
                hover: 0xe3e9f2,
                selected: 0xe2eafb,
                border: 0xdce2eb,
                input_border: 0x7a879a,
                text: 0x202735,
                text_secondary: 0x4b586d,
                muted: 0x56657a,
                accent: 0x315dc5,
                success: 0x126f43,
                warning: 0x815a00,
                danger: 0xb42332,
                diff_added: 0xe3f2e8,
                diff_removed: 0xfbe6e8,
            }
        }
    }
}

pub(super) fn palette(cx: &App) -> Palette {
    Palette::for_dark(Theme::global(cx).is_dark())
}

pub(super) const CONTROL_HEIGHT: f32 = 36.;
pub(super) const COMPACT_CONTROL_HEIGHT: f32 = 32.;
pub(super) const HEADER_HEIGHT: f32 = 54.;
pub(super) const SIDEBAR_WIDTH: f32 = 264.;
pub(super) const CONTENT_WIDTH: f32 = 800.;
pub(super) const CONTROL_RADIUS: f32 = 8.;
pub(super) const CARD_RADIUS: f32 = 12.;

pub(super) const MONO_FONT: &str = if cfg!(target_os = "macos") {
    "SF Mono"
} else if cfg!(target_os = "windows") {
    "Consolas"
} else {
    "DejaVu Sans Mono"
};

#[derive(Clone, Copy, Default)]
pub(crate) struct SystemAccessibility {
    pub reduce_transparency: bool,
    pub reduce_motion: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedAppearance {
    pub dark: bool,
    pub glass: bool,
    pub reduced_motion: bool,
    pub active: bool,
}

impl Global for ResolvedAppearance {}

impl ResolvedAppearance {
    pub(crate) fn resolve(
        settings: AppearanceSettings,
        system: WindowAppearance,
        accessibility: SystemAccessibility,
        active: bool,
        blur_supported: bool,
    ) -> Self {
        Self {
            dark: match settings.theme {
                ThemePreference::System => matches!(
                    system,
                    WindowAppearance::Dark | WindowAppearance::VibrantDark
                ),
                ThemePreference::Light => false,
                ThemePreference::Dark => true,
            },
            glass: settings.glass && blur_supported && !accessibility.reduce_transparency,
            reduced_motion: settings.reduced_motion || accessibility.reduce_motion,
            active,
        }
    }

    pub(crate) fn window_background(self) -> WindowBackgroundAppearance {
        if self.glass {
            WindowBackgroundAppearance::Blurred
        } else {
            WindowBackgroundAppearance::Opaque
        }
    }

    pub(super) fn materials(self) -> Materials {
        let colors = Palette::for_dark(self.dark);
        Materials {
            // Only chrome exposes the native behind-window blur. Floating controls
            // blend with the app beneath them; GPUI has no per-element backdrop blur.
            chrome: with_alpha(
                colors.surface,
                if self.glass && self.active { 0.94 } else { 1. },
            ),
            floating: with_alpha(colors.elevated, if self.glass { 0.98 } else { 1. }),
            edge: if self.glass && self.dark {
                rgba(0xffffff24).into()
            } else {
                rgb(colors.border).into()
            },
            shadow: if self.dark {
                rgba(0x00000038).into()
            } else {
                rgba(0x22375916).into()
            },
        }
    }
}

pub(super) struct Materials {
    pub chrome: Hsla,
    pub floating: Hsla,
    pub edge: Hsla,
    shadow: Hsla,
}

impl Materials {
    pub(super) fn shadow(&self) -> Vec<BoxShadow> {
        vec![box_shadow(0., 4., 16., -4., self.shadow)]
    }
}

pub(super) fn materials(cx: &App) -> Materials {
    cx.global::<ResolvedAppearance>().materials()
}

fn with_alpha(color: u32, alpha: f32) -> Hsla {
    let mut color: Hsla = rgb(color).into();
    color.a = alpha;
    color
}

pub(super) fn contrast_ratio(foreground: gpui_kit::Rgba, background: gpui_kit::Rgba) -> f32 {
    let luminance = |color: gpui_kit::Rgba| {
        let linear = |channel: f32| {
            if channel <= 0.04045 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
    };
    let a = luminance(foreground);
    let b = luminance(background);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

pub(crate) fn configure_theme(cx: &mut App) {
    apply_theme(
        ResolvedAppearance::resolve(
            AppearanceSettings::default(),
            cx.window_appearance(),
            system_accessibility(),
            true,
            cfg!(target_os = "macos"),
        ),
        cx,
    );
}

pub(crate) fn apply_theme(appearance: ResolvedAppearance, cx: &mut App) -> bool {
    if cx.has_global::<ResolvedAppearance>() && *cx.global::<ResolvedAppearance>() == appearance {
        return false;
    }
    cx.set_global(appearance);
    cx.set_reduce_motion(appearance.reduced_motion);
    Theme::change(
        if appearance.dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        None,
        cx,
    );
    let colors = Palette::for_dark(appearance.dark);
    let material = appearance.materials();
    let theme = Theme::global_mut(cx);
    theme.font_family = ".SystemUIFont".into();
    theme.font_size = px(14.);
    theme.mono_font_family = MONO_FONT.into();
    theme.mono_font_size = px(13.);
    theme.radius = px(CONTROL_RADIUS);
    theme.radius_lg = px(CARD_RADIUS);
    theme.shadow = appearance.glass;

    theme.background = rgb(colors.canvas).into();
    theme.foreground = rgb(colors.text).into();
    theme.border = rgb(colors.border).into();
    theme.input = rgb(colors.input_border).into();
    theme.caret = rgb(colors.text).into();
    theme.ring = rgb(colors.accent).into();
    theme.selection = with_alpha(colors.accent, 0.25);
    theme.muted = rgb(colors.recessed).into();
    theme.muted_foreground = rgb(colors.muted).into();
    theme.accent = rgb(colors.hover).into();
    theme.accent_foreground = rgb(colors.text).into();
    theme.primary = rgb(0x3765d6).into();
    theme.primary_hover = rgb(0x2e58c2).into();
    theme.primary_active = rgb(0x274dab).into();
    theme.primary_foreground = rgb(0xffffff).into();
    theme.secondary = rgb(colors.recessed).into();
    theme.secondary_hover = rgb(colors.hover).into();
    theme.secondary_active = rgb(colors.selected).into();
    theme.secondary_foreground = rgb(colors.text_secondary).into();
    theme.link = rgb(colors.accent).into();
    theme.link_hover = theme.link;
    theme.link_active = theme.link;
    theme.danger = rgb(colors.danger).into();
    theme.warning = rgb(colors.warning).into();
    theme.success = rgb(colors.success).into();
    theme.danger_hover = theme.danger;
    theme.danger_active = theme.danger;
    theme.danger_foreground = rgb(colors.canvas).into();
    theme.popover = material.floating;
    theme.popover_foreground = rgb(colors.text).into();
    theme.overlay = rgba(0x00000066).into();
    theme.title_bar = material.chrome;
    theme.title_bar_border = material.edge;
    theme.sidebar = material.chrome;
    theme.sidebar_foreground = rgb(colors.text).into();
    theme.sidebar_accent = rgb(colors.selected).into();
    theme.sidebar_accent_foreground = rgb(colors.text).into();
    theme.sidebar_border = material.edge;
    theme.colors.list = material.chrome;
    theme.list_active = rgb(colors.selected).into();
    theme.list_active_border = rgb(colors.selected).into();
    theme.list_hover = rgb(colors.hover).into();
    theme.scrollbar = rgba(0x00000000).into();
    theme.scrollbar_thumb = with_alpha(colors.text, 0.20);
    theme.scrollbar_thumb_hover = with_alpha(colors.text, 0.32);
    theme.switch = rgb(colors.input_border).into();
    theme.switch_thumb = rgb(0xffffff).into();
    theme.button = rgb(colors.recessed).into();
    theme.button_hover = rgb(colors.hover).into();
    theme.button_active = rgb(colors.selected).into();
    theme.button_foreground = rgb(colors.text).into();
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
    gpui_kit::base::TextViewDefaults::global(cx)
        .with_code_block_highlighter(move |block| {
            let language = block.lang().unwrap_or_default();
            super::tools::code_highlights(
                &block.code(),
                &language,
                language == "diff",
                appearance.dark,
            )
        })
        .install(cx);
    true
}

#[cfg(any(not(target_os = "macos"), test))]
pub(crate) fn system_accessibility() -> SystemAccessibility {
    SystemAccessibility::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    fn appearance_follows_system_and_applies_accessibility_without_idle_updates(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        for system in [WindowAppearance::Light, WindowAppearance::Dark] {
            for preference in [
                ThemePreference::System,
                ThemePreference::Light,
                ThemePreference::Dark,
            ] {
                let appearance = ResolvedAppearance::resolve(
                    AppearanceSettings {
                        theme: preference,
                        ..Default::default()
                    },
                    system,
                    SystemAccessibility::default(),
                    true,
                    true,
                );
                assert_eq!(
                    appearance.dark,
                    match preference {
                        ThemePreference::System => system == WindowAppearance::Dark,
                        ThemePreference::Light => false,
                        ThemePreference::Dark => true,
                    }
                );
                cx.update(|cx| {
                    apply_theme(appearance, cx);
                    assert_eq!(Theme::global(cx).is_dark(), appearance.dark);
                    assert!(!apply_theme(appearance, cx));
                });
            }
        }
        for blur_supported in [true, false] {
            for reduced in [true, false] {
                let appearance = ResolvedAppearance::resolve(
                    AppearanceSettings::default(),
                    WindowAppearance::Dark,
                    SystemAccessibility {
                        reduce_transparency: reduced,
                        reduce_motion: reduced,
                    },
                    true,
                    blur_supported,
                );
                assert_eq!(appearance.glass, blur_supported && !reduced);
                assert_eq!(
                    appearance.window_background(),
                    if appearance.glass {
                        WindowBackgroundAppearance::Blurred
                    } else {
                        WindowBackgroundAppearance::Opaque
                    }
                );
                cx.update(|cx| {
                    apply_theme(appearance, cx);
                    assert_eq!(cx.reduce_motion(), reduced);
                });
            }
        }
    }

    #[test]
    fn text_contrast_survives_glass_composition_on_black_and_white_desktops() {
        for dark in [true, false] {
            let colors = Palette::for_dark(dark);
            for glass in [true, false] {
                for active in [true, false] {
                    let appearance = ResolvedAppearance {
                        dark,
                        glass,
                        active,
                        reduced_motion: true,
                    };
                    let material = appearance.materials();
                    for background in [
                        colors.canvas,
                        colors.surface,
                        colors.elevated,
                        colors.recessed,
                        colors.hover,
                        colors.selected,
                    ] {
                        for foreground in [colors.text, colors.text_secondary, colors.muted] {
                            assert!(contrast_ratio(rgb(foreground), rgb(background)) >= 4.5);
                        }
                    }
                    for surface in [material.chrome, material.floating] {
                        if !glass {
                            assert_eq!(surface.a, 1.);
                        }
                        let surface: gpui_kit::Rgba = surface.into();
                        for desktop in [0., 1.] {
                            let background = gpui_kit::Rgba {
                                r: surface.r * surface.a + desktop * (1. - surface.a),
                                g: surface.g * surface.a + desktop * (1. - surface.a),
                                b: surface.b * surface.a + desktop * (1. - surface.a),
                                a: 1.,
                            };
                            for foreground in [colors.text, colors.text_secondary, colors.muted] {
                                assert!(contrast_ratio(rgb(foreground), background) >= 4.5);
                            }
                        }
                    }
                    assert!(contrast_ratio(rgb(colors.input_border), rgb(colors.surface)) >= 3.);
                }
            }
        }
    }
}

#[cfg(all(target_os = "macos", not(test)))]
pub(crate) fn system_accessibility() -> SystemAccessibility {
    use std::ffi::{c_char, c_void};

    // Read public NSWorkspace accessibility properties on startup/activation,
    // never during rendering or on a timer. No extra Objective-C dependency.
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    // SAFETY: All names are static NUL-terminated strings. These selectors take
    // no arguments and return an object or BOOL, matching the typed call sites.
    unsafe {
        let object: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let boolean: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i8 =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let workspace = object(
            objc_getClass(c"NSWorkspace".as_ptr()),
            sel_registerName(c"sharedWorkspace".as_ptr()),
        );
        SystemAccessibility {
            reduce_transparency: boolean(
                workspace,
                sel_registerName(c"accessibilityDisplayShouldReduceTransparency".as_ptr()),
            ) != 0,
            reduce_motion: boolean(
                workspace,
                sel_registerName(c"accessibilityDisplayShouldReduceMotion".as_ptr()),
            ) != 0,
        }
    }
}
