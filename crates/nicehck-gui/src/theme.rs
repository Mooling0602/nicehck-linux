//! Colours and styling.
//!
//! Two palettes with the same structure, picked by the active [`egui::Theme`].
//! `ctx.theme()` resolves `ThemePreference::System` against the desktop's own
//! preference, so following the system costs nothing extra: the palette is
//! looked up per frame rather than cached at startup, and a mid-session theme
//! switch is picked up immediately.
//!
//! The accent is a teal-cyan: it reads as "audio gear" rather than as a warning,
//! stays legible on both backgrounds, and is distinct from the amber used for
//! the write-unlock warning.

use egui::{Color32, Context, Theme};

/// Colours that differ between light and dark mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Accent for the response curve, active controls and headings.
    pub accent: Color32,
    /// A dimmer accent, for fills under the accent (chips, badges).
    pub accent_dim: Color32,
    /// 0 dB reference line and other de-emphasised structure.
    pub reference: Color32,
    /// Warning: the write-unlock banner and the band control points.
    pub warn: Color32,
    /// Connected / healthy.
    pub ok: Color32,
    /// Failure.
    pub err: Color32,
    /// Window background.
    pub bg: Color32,
    /// Cards and grouped panels, one step off `bg`.
    pub surface: Color32,
    /// Slightly further off `bg`, for nested or inset areas.
    pub surface_alt: Color32,
    /// Hairline borders and separators.
    pub border: Color32,
    /// Grid lines inside the plot.
    pub grid: Color32,
    /// Primary text.
    pub text: Color32,
    /// Secondary text.
    pub text_weak: Color32,
}

/// Dark palette — the default look.
pub const DARK: Palette = Palette {
    accent: Color32::from_rgb(0x2E, 0xD6, 0xC4),
    accent_dim: Color32::from_rgb(0x1B, 0x4A, 0x48),
    reference: Color32::from_rgb(0x6B, 0x76, 0x84),
    warn: Color32::from_rgb(0xF0, 0xB0, 0x4A),
    ok: Color32::from_rgb(0x4A, 0xD0, 0x74),
    err: Color32::from_rgb(0xF0, 0x6C, 0x6C),
    bg: Color32::from_rgb(0x14, 0x17, 0x1C),
    surface: Color32::from_rgb(0x1C, 0x20, 0x27),
    surface_alt: Color32::from_rgb(0x24, 0x29, 0x32),
    border: Color32::from_rgb(0x30, 0x36, 0x40),
    grid: Color32::from_rgb(0x2A, 0x30, 0x39),
    text: Color32::from_rgb(0xE6, 0xEA, 0xEF),
    text_weak: Color32::from_rgb(0x9A, 0xA4, 0xB2),
};

/// Light palette — same structure, adjusted for contrast on a bright background.
///
/// The accent is darkened relative to [`DARK`]: the dark-mode teal is too pale
/// to read against white, and a curve drawn in it would wash out.
pub const LIGHT: Palette = Palette {
    accent: Color32::from_rgb(0x00, 0x82, 0x7A),
    accent_dim: Color32::from_rgb(0xD0, 0xEC, 0xE9),
    reference: Color32::from_rgb(0x9A, 0xA3, 0xAE),
    warn: Color32::from_rgb(0xB5, 0x74, 0x00),
    ok: Color32::from_rgb(0x1B, 0x7F, 0x3B),
    err: Color32::from_rgb(0xC0, 0x28, 0x28),
    bg: Color32::from_rgb(0xF6, 0xF7, 0xF9),
    surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    surface_alt: Color32::from_rgb(0xEE, 0xF0, 0xF4),
    border: Color32::from_rgb(0xD5, 0xDA, 0xE1),
    grid: Color32::from_rgb(0xE2, 0xE6, 0xEB),
    text: Color32::from_rgb(0x1A, 0x1D, 0x22),
    text_weak: Color32::from_rgb(0x5F, 0x6A, 0x78),
};

/// The palette for the currently active theme.
///
/// Cheap enough to call per frame: it only matches on the theme and copies a
/// handful of `Color32`s, and reading it fresh each frame is what makes a system
/// theme change apply without a restart.
pub fn palette(ctx: &Context) -> &'static Palette {
    match ctx.theme() {
        Theme::Dark => &DARK,
        Theme::Light => &LIGHT,
    }
}

/// En dash, used in range labels.
pub const DASH: &str = "–";

/// Corner radius for cards, in points.
pub const CARD_RADIUS: u8 = 10;
/// Corner radius for chips and small controls.
pub const CHIP_RADIUS: u8 = 6;
/// Standard inner padding for cards.
pub const CARD_PADDING: f32 = 12.0;

/// A raised card: the main structural element of the layout.
pub fn card(_ui: &egui::Ui, p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.surface)
        .stroke(egui::Stroke::new(1.0, p.border))
        .corner_radius(CARD_RADIUS)
        .inner_margin(CARD_PADDING)
}

/// A tinted callout strip, for warnings and errors.
pub fn callout(tint: Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(tint.gamma_multiply(0.18))
        .stroke(egui::Stroke::new(1.0, tint.gamma_multiply(0.55)))
        .corner_radius(CHIP_RADIUS)
        .inner_margin(egui::Margin::symmetric(12, 10))
}

/// A small rounded label, for tags like "8 段" or "只读".
pub fn chip(ui: &mut egui::Ui, p: &Palette, text: &str, tint: Color32) {
    egui::Frame::new()
        .fill(tint.gamma_multiply(if p.text == DARK.text { 0.22 } else { 0.14 }))
        .corner_radius(egui::CornerRadius::same(CHIP_RADIUS))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(12.0).color(tint));
        });
}

/// Apply fonts-independent style: spacing, rounding and widget colours.
///
/// Called every frame so that a system theme change takes effect immediately.
/// egui keeps one style per theme, so we mutate only the active one.
pub fn apply(ctx: &Context) {
    let p = palette(ctx);
    let theme = ctx.theme();
    ctx.style_mut_of(theme, |style| {
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.slider_width = 150.0;
        style.spacing.interact_size.y = 24.0;

        // Rounded, borderless widgets; the palette supplies the depth instead of
        // shadows, which keeps the look flat and consistent across both themes.
        style.visuals.window_corner_radius = egui::CornerRadius::same(CARD_RADIUS);
        style.visuals.menu_corner_radius = egui::CornerRadius::same(CHIP_RADIUS);

        style.visuals.panel_fill = p.bg;
        style.visuals.window_fill = p.surface;
        style.visuals.extreme_bg_color = p.bg;
        style.visuals.faint_bg_color = p.surface_alt;
        style.visuals.override_text_color = Some(p.text);
        style.visuals.window_stroke = egui::Stroke::new(1.0, p.border);

        let w = &mut style.visuals.widgets;
        for v in [
            &mut w.noninteractive,
            &mut w.inactive,
            &mut w.hovered,
            &mut w.active,
            &mut w.open,
        ] {
            v.corner_radius = egui::CornerRadius::same(CHIP_RADIUS);
        }
        w.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.border);
        w.inactive.bg_fill = p.surface_alt;
        w.inactive.weak_bg_fill = p.surface_alt;
        w.inactive.bg_stroke = egui::Stroke::new(1.0, p.border);
        w.inactive.fg_stroke = egui::Stroke::new(1.0, p.text);
        w.hovered.bg_fill = p.accent.gamma_multiply(0.25);
        w.hovered.weak_bg_fill = p.accent.gamma_multiply(0.25);
        w.hovered.bg_stroke = egui::Stroke::new(1.0, p.accent);
        w.hovered.fg_stroke = egui::Stroke::new(1.5, p.text);
        w.active.bg_fill = p.accent.gamma_multiply(0.40);
        w.active.weak_bg_fill = p.accent.gamma_multiply(0.40);
        w.active.bg_stroke = egui::Stroke::new(1.0, p.accent);
        w.active.fg_stroke = egui::Stroke::new(1.5, p.text);

        style.visuals.selection.bg_fill = p.accent.gamma_multiply(0.45);
        style.visuals.selection.stroke = egui::Stroke::new(1.0, p.accent);
        style.visuals.hyperlink_color = p.accent;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palettes_are_distinct_and_self_consistent() {
        // Guards against a copy-paste that leaves a colour identical in both
        // themes, which is how a "invisible in light mode" bug gets in.
        assert_ne!(DARK.bg, LIGHT.bg);
        assert_ne!(DARK.text, LIGHT.text);
        assert_ne!(DARK.accent, LIGHT.accent);
        assert_ne!(DARK.surface, LIGHT.surface);

        for p in [DARK, LIGHT] {
            assert_ne!(p.bg, p.text, "text must contrast with the background");
            assert_ne!(p.surface, p.border, "borders must be visible on cards");
        }
    }

    #[test]
    fn light_background_is_lighter_than_dark() {
        let lum = |c: Color32| c.r() as u32 + c.g() as u32 + c.b() as u32;
        assert!(lum(LIGHT.bg) > lum(DARK.bg));
        // Inverted text polarity is the whole point of the two palettes.
        assert!(lum(LIGHT.text) < lum(DARK.text));
    }

    #[test]
    fn palette_follows_the_context_theme() {
        let ctx = Context::default();
        ctx.set_theme(Theme::Dark);
        assert_eq!(palette(&ctx).bg, DARK.bg);
        ctx.set_theme(Theme::Light);
        assert_eq!(palette(&ctx).bg, LIGHT.bg);
    }
}
