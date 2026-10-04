//! `nicehck-gui` — native Linux control panel for NICEHCK / YUANDAO USB-C DSP
//! earphones, built on egui/eframe.
//!
//! The UI mirrors the Android app's structure but talks to `/dev/hidraw` directly.
//! Read paths are live; every control that would change the sound is disabled
//! until the user explicitly unlocks writes, and the write path is described in
//! the UI so nothing is silent.

mod app;
mod curve;
mod fonts;
mod presets;
mod theme;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([880.0, 600.0])
            // Close the OS title bar and draw our own: the system one carries
            // the compositor's palette, which clashes with the app theme and
            // pulls the eye away from the EQ controls.
            .with_decorations(false)
            .with_title("NICEHCK Headset Control"),
        ..Default::default()
    };

    eframe::run_native(
        "NICEHCK Headset Control",
        options,
        Box::new(|cc| {
            fonts::install(&cc.egui_ctx);
            Ok(Box::new(app::NicehckApp::new(cc)))
        }),
    )
}
