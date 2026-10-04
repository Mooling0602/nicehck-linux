//! Shared colours and text styling.

use egui::Color32;

/// Accent used for the response curve and active controls.
pub const ACCENT: Color32 = Color32::from_rgb(0x4C, 0xC2, 0xA6);
/// Colour for the flat / 0 dB reference line.
pub const REFERENCE: Color32 = Color32::from_rgb(0x7A, 0x84, 0x94);
/// Warning colour for the write-unlock banner.
pub const WARN: Color32 = Color32::from_rgb(0xE0, 0xA4, 0x3C);
/// Success / connected colour.
pub const OK: Color32 = Color32::from_rgb(0x5A, 0xC8, 0x6A);
/// Error colour.
pub const ERR: Color32 = Color32::from_rgb(0xE0, 0x6C, 0x6C);

/// En dash, used in range labels.
pub const DASH: &str = "–";
