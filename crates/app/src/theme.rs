//! Brand palette and small colour helpers on top of the component theme.

use gpui_kit::component::{ActiveTheme, Theme, ThemeMode};
use gpui_kit::{App, Hsla, Rgba, rgb, rgba};
use mango_core::Rgba as CoreRgba;

/// Mango orange: primary actions, the playhead, the active word.
pub const MANGO: u32 = 0xFF8A00;
/// Can blue: selection and secondary accents.
pub const LAGOON: u32 = 0x14A3C7;
pub const HIBISCUS: u32 = 0xE63E6D;
pub const LEAF: u32 = 0x4CAF50;

pub const HIGHLIGHTS: [u32; 5] = [0xF2C91D, 0x4CAF50, 0x14A3C7, 0xE63E6D, 0x8E5BD9];

pub fn mango() -> Hsla {
    rgb(MANGO).into()
}
pub fn lagoon() -> Hsla {
    rgb(LAGOON).into()
}
pub fn hibiscus() -> Hsla {
    rgb(HIBISCUS).into()
}

pub fn alpha(hex: u32, a: f32) -> Hsla {
    let c: Hsla = rgb(hex).into();
    c.opacity(a)
}

pub fn from_core(c: CoreRgba) -> Hsla {
    let v = ((c.r as u32) << 24) | ((c.g as u32) << 16) | ((c.b as u32) << 8) | c.a as u32;
    let r: Rgba = rgba(v);
    r.into()
}

pub fn to_core(c: Hsla) -> CoreRgba {
    let r: Rgba = c.into();
    let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    CoreRgba::rgba(ch(r.r), ch(r.g), ch(r.b), ch(r.a))
}

/// Applies the Mango Loco accents to the component theme.
pub fn apply(cx: &mut App, dark: bool) {
    Theme::change(if dark { ThemeMode::Dark } else { ThemeMode::Light }, None, cx);
    Theme::update(cx, |t| {
        let m = mango();
        t.primary = m;
        t.primary_hover = m.opacity(0.9);
        t.primary_active = m.opacity(0.8);
        t.primary_foreground = rgb(0x1A1206).into();
        t.button_primary = m;
        t.button_primary_hover = m.opacity(0.9);
        t.button_primary_active = m.opacity(0.8);
        t.button_primary_foreground = rgb(0x1A1206).into();
        t.ring = m;
        t.caret = m;
        t.slider_bar = m;
        t.progress_bar = m;
        t.selection = lagoon().opacity(0.35);
        t.link = lagoon();
        if dark {
            t.background = rgb(0x14161A).into();
            t.sidebar = rgb(0x191C21).into();
            t.title_bar = rgb(0x191C21).into();
            t.border = rgb(0x2A2E36).into();
            t.muted = rgb(0x22262D).into();
        }
    });
}

pub fn is_dark(cx: &App) -> bool {
    cx.theme().is_dark()
}
