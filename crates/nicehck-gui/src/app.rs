//! Application state and UI.
//!
//! Design constraints that shaped this:
//!
//! * **Nothing is written without an explicit unlock.** The headset is live audio
//!   gear; a stray EQ write is audible immediately. The write gate is visible,
//!   revocable state rather than a hidden flag.
//! * **HID I/O never blocks the UI.** The device sits behind a mutex shared with
//!   worker threads, and results arrive over a channel, so a silent device cannot
//!   freeze the window.
//! * **Failures are shown, not swallowed.** Permission errors and timeouts render
//!   with the exact remediation (udev rule / group membership).

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui::RichText;
use egui_plot::{Line, PlotPoints};
use nicehck_protocol::command::{self, Band, EqState};
use nicehck_protocol::device_config::{self, DeviceConfigFile, EqualizerCapability};
use nicehck_protocol::transport::{self, HidDevice, TransportError};

use crate::curve;
use crate::presets;
use crate::theme;

/// Default reply timeout for a single command round-trip.
const IO_TIMEOUT: Duration = Duration::from_millis(1500);

/// How often `Auto` re-reads the device.
const AUTO_REFRESH_SECS: f64 = 3.0;

/// A message from an I/O worker back to the UI thread.
enum IoResult {
    Connected(Box<Connected>),
    EqSnapshot {
        bands: Vec<Band>,
        preset: Option<u8>,
    },
    Applied {
        reports: usize,
    },
    Error(String),
}

/// Device metadata collected at connect time.
#[derive(Clone)]
struct Connected {
    path: String,
    product_name: String,
    vendor_id: u16,
    product_id: u16,
    config: Option<DeviceConfigFile>,
}

impl Connected {
    fn capability(&self) -> Option<EqualizerCapability> {
        self.config.as_ref().and_then(|k| k.equalizer())
    }

    /// Localised name of a factory preset index.
    fn preset_name(&self, index: u8) -> String {
        self.capability()
            .and_then(|e| {
                e.presets
                    .iter()
                    .find(|x| x.preset_index == index)
                    .map(|x| format!("{} ({})", x.display_name(), x.preset_key))
            })
            .unwrap_or_else(|| format!("preset {index}"))
    }
}

/// Which panel is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Equalizer,
    Device,
    About,
}

/// Shared, thread-safe device handle.
type SharedDevice = Arc<Mutex<Option<HidDevice>>>;

pub struct NicehckApp {
    tab: Tab,
    connected: Option<Connected>,
    device: SharedDevice,
    bands: Vec<Band>,
    preset_index: Option<u8>,
    error: Option<String>,
    busy: bool,
    writes_unlocked: bool,
    eq_offset_db: f32,
    sample_rate: f64,
    band_count: u8,
    auto_refresh: bool,
    last_refresh: f64,
    tx: Sender<IoResult>,
    rx: Receiver<IoResult>,
    log: Vec<String>,

    /// Continuous write: every slider change is pushed to the device as soon as
    /// the debounce timer expires, so the listener hears the result immediately.
    live_write: bool,
    /// Wall-clock time of the last slider change, for debounce.
    last_edit_time: f64,
    /// True when the bands/offset changed and a live write is pending.
    pending_write: bool,

    /// Custom presets loaded from disk.
    custom_presets: Vec<presets::CustomPreset>,
    /// Save-dialog state.
    show_save_dialog: bool,
    save_name: String,
    save_offset: f32,
    save_bands: Vec<Band>,
}

impl NicehckApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Follow the desktop's light/dark preference. `ThemePreference::System`
        // is egui's default, but stating it makes the intent explicit and keeps
        // it from being lost if a different default ever lands upstream.
        cc.egui_ctx.set_theme(egui::ThemePreference::System);
        theme::apply(&cc.egui_ctx);

        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            tab: Tab::Equalizer,
            connected: None,
            device: Arc::new(Mutex::new(None)),
            bands: Vec::new(),
            preset_index: None,
            error: None,
            busy: false,
            writes_unlocked: false,
            eq_offset_db: 0.0,
            sample_rate: 96_000.0,
            band_count: 8,
            auto_refresh: false,
            last_refresh: 0.0,
            tx,
            rx,
            log: Vec::new(),
            live_write: false,
            last_edit_time: 0.0,
            pending_write: false,
            custom_presets: presets::load_presets(),
            show_save_dialog: false,
            save_name: String::new(),
            save_offset: 0.0,
            save_bands: Vec::new(),
        };
        app.connect();
        app
    }

    /// Discover and open the headset on a worker thread.
    fn connect(&mut self) {
        self.busy = true;
        self.error = None;
        let tx = self.tx.clone();
        let slot = Arc::clone(&self.device);

        std::thread::spawn(move || {
            let nodes = transport::enumerate(device_config::VENDOR_ID);
            let Some(info) = nodes.first() else {
                let _ = tx.send(IoResult::Error(
                    "No NICEHCK/YUANDAO headset found on hidraw.\n\
                     Check that the headset is plugged in, then press Reconnect."
                        .into(),
                ));
                return;
            };

            match HidDevice::open_path(info) {
                Ok(dev) => {
                    let connected = Connected {
                        path: dev.path().display().to_string(),
                        product_name: dev.product_name().to_string(),
                        vendor_id: dev.vendor_id(),
                        product_id: dev.product_id(),
                        config: device_config::find_usb_device(dev.vendor_id(), dev.product_id()),
                    };
                    if let Ok(mut guard) = slot.lock() {
                        *guard = Some(dev);
                    }
                    let _ = tx.send(IoResult::Connected(Box::new(connected)));
                }
                Err(e) => {
                    // The transport error already spells out the udev fix for the
                    // permission case; only add context for other failures.
                    let msg = match e {
                        TransportError::PermissionDenied { .. } => {
                            format!("Found {} but could not open it:\n{e}", info.path.display())
                        }
                        _ => format!(
                            "Found {} but could not open it:\n{e}\n\n\
                             Check that no other program holds the interface, \
                             then press Reconnect.",
                            info.path.display()
                        ),
                    };
                    let _ = tx.send(IoResult::Error(msg));
                }
            }
        });
    }

    /// Read the full EQ snapshot on a worker thread.
    fn refresh_eq(&mut self) {
        if self.device.lock().map(|g| g.is_none()).unwrap_or(true) {
            self.error = Some("Not connected.".into());
            return;
        }
        self.busy = true;
        let tx = self.tx.clone();
        let slot = Arc::clone(&self.device);
        let count = self.band_count;

        std::thread::spawn(move || {
            let Ok(mut guard) = slot.lock() else {
                let _ = tx.send(IoResult::Error("Device lock poisoned.".into()));
                return;
            };
            let Some(dev) = guard.as_mut() else {
                let _ = tx.send(IoResult::Error("Not connected.".into()));
                return;
            };

            dev.drain(Duration::from_millis(30));
            match dev.request(&command::read_eq_bands(count), IO_TIMEOUT) {
                Ok(replies) => {
                    let mut bands = Vec::new();
                    let mut preset = None;
                    for raw in &replies {
                        if let Some((band, p)) = command::parse_eq_band(raw) {
                            preset = Some(p);
                            bands.push(band);
                        }
                    }
                    bands.sort_by_key(|b| b.index);
                    let _ = tx.send(IoResult::EqSnapshot { bands, preset });
                }
                Err(e) => {
                    let _ = tx.send(IoResult::Error(format!("Failed to read EQ: {e}")));
                }
            }
        });
    }

    /// Push the edited EQ to the device. Requires `writes_unlocked`.
    fn apply_eq(&mut self) {
        if !self.writes_unlocked {
            self.error = Some("Writes are locked.".into());
            return;
        }
        if self.device.lock().map(|g| g.is_none()).unwrap_or(true) {
            self.error = Some("Not connected.".into());
            return;
        }

        // A custom edit no longer corresponds to a factory preset, so the active
        // preset index is preserved from the last read rather than invented.
        let state = EqState {
            bands: self.bands.clone(),
            preset_index: self.preset_index.unwrap_or(0),
        };
        let reports = command::write_eq(&state, self.sample_rate);
        let tx = self.tx.clone();
        let slot = Arc::clone(&self.device);
        self.busy = true;

        std::thread::spawn(move || {
            let Ok(mut guard) = slot.lock() else {
                let _ = tx.send(IoResult::Error("Device lock poisoned.".into()));
                return;
            };
            let Some(dev) = guard.as_mut() else {
                let _ = tx.send(IoResult::Error("Not connected.".into()));
                return;
            };
            // Write commands are fire-and-forget: the device does not reply,
            // so we must not use `request()` (which waits for replies and
            // would always time out here).
            match dev.write_reports(&reports, Duration::from_millis(20)) {
                Ok(()) => {
                    let _ = tx.send(IoResult::Applied {
                        reports: reports.len(),
                    });
                }
                Err(e) => {
                    let _ = tx.send(IoResult::Error(format!("Apply failed: {e}")));
                }
            }
        });
    }

    /// Drain worker messages; called once per frame.
    fn pump(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                IoResult::Connected(c) => {
                    if let Some(eq) = c.capability() {
                        self.band_count = eq.frequency_number;
                        self.sample_rate = eq.sample_rate;
                    }
                    self.log.push(match &c.config {
                        Some(k) => format!("Connected: {} at {}", k.device.name, c.path),
                        None => format!("Connected: {} (unknown model)", c.product_name),
                    });
                    self.error = None;
                    self.connected = Some(*c);
                    self.busy = false;
                    self.refresh_eq();
                }
                IoResult::EqSnapshot { bands, preset } => {
                    if !bands.is_empty() {
                        self.bands = bands;
                    }
                    if preset.is_some() {
                        self.preset_index = preset;
                    }
                    self.busy = false;
                }
                IoResult::Applied { reports } => {
                    self.log
                        .push(format!("Applied {reports} reports to the device"));
                    self.busy = false;
                }
                IoResult::Error(e) => {
                    self.log.push(e.clone());
                    self.error = Some(e);
                    self.busy = false;
                }
            }
        }
    }

    /// EQ limits from the matched device config, if any.
    fn capability(&self) -> Option<EqualizerCapability> {
        self.connected.as_ref().and_then(|c| c.capability())
    }

    /// Live-write debounce: called every frame after the UI has run. If the
    /// bands or offset changed and enough time has passed since the last edit,
    /// push the state to the device automatically.
    ///
    /// The debounce prevents flooding the HID endpoint while a slider is being
    /// dragged — without it every mouse pixel would become a full 12-report
    /// write burst. 150 ms is long enough to coalesce a drag but short enough
    /// that the listener hears the result almost immediately.
    fn maybe_live_write(&mut self, ctx: &egui::Context) {
        if !self.live_write || self.busy || !self.writes_unlocked {
            return;
        }
        if !self.pending_write {
            return;
        }
        let now = ctx.input(|i| i.time);
        if now - self.last_edit_time < 0.15 {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            return;
        }
        self.pending_write = false;
        self.apply_eq();
    }

    /// Record that the user changed something, so the live-write timer restarts.
    fn mark_edited(&mut self, ctx: &egui::Context) {
        self.last_edit_time = ctx.input(|i| i.time);
        self.pending_write = true;
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }

    /// Open the save-preset dialog with a fresh default name.
    fn open_save_dialog(&mut self) {
        self.save_name = presets::next_default_name(&self.custom_presets);
        self.save_offset = self.eq_offset_db;
        self.save_bands = self.bands.clone();
        self.show_save_dialog = true;
    }

    /// Persist the current state as a new custom preset.
    fn commit_save_preset(&mut self) {
        let name = if self.save_name.trim().is_empty() {
            presets::next_default_name(&self.custom_presets)
        } else {
            self.save_name.trim().to_string()
        };
        let preset = presets::CustomPreset {
            name: name.clone(),
            offset: self.save_offset,
            bands: self.save_bands.clone(),
        };
        self.custom_presets.push(preset);
        match presets::save_presets(&self.custom_presets) {
            Ok(path) => {
                self.log
                    .push(format!("已保存预设「{name}」→ {}", path.display()));
                self.show_save_dialog = false;
            }
            Err(e) => {
                self.error = Some(format!("保存预设失败：{e}"));
            }
        }
    }

    /// Load a custom preset into the sliders (local preview only).
    fn load_custom_preset(&mut self, idx: usize) {
        if let Some(p) = self.custom_presets.get(idx) {
            self.bands = p.bands.clone();
            self.eq_offset_db = p.offset;
            self.preset_index = None;
            self.log.push(format!("已载入预设「{}」", p.name));
            // Loading is an edit too: with live-write on it should propagate.
            // We cannot call mark_edited here (no ctx), so set the flag; the
            // next frame's `pump` will pick it up through the normal path.
            self.pending_write = true;
        }
    }

    /// Delete a custom preset and persist the list.
    fn delete_custom_preset(&mut self, idx: usize) {
        if idx < self.custom_presets.len() {
            let name = self.custom_presets[idx].name.clone();
            self.custom_presets.remove(idx);
            match presets::save_presets(&self.custom_presets) {
                Ok(_) => self.log.push(format!("已删除预设「{name}」")),
                Err(e) => self.error = Some(format!("保存预设列表失败：{e}")),
            }
        }
    }
}

impl eframe::App for NicehckApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Re-applied per frame: `ctx.theme()` resolves `ThemePreference::System`
        // against the desktop setting, so a live theme switch is picked up here.
        theme::apply(&ctx);
        let p = *theme::palette(&ctx);

        self.pump();

        if self.auto_refresh && !self.busy && self.connected.is_some() {
            let now = ctx.input(|i| i.time);
            if now - self.last_refresh > AUTO_REFRESH_SECS {
                self.last_refresh = now;
                self.refresh_eq();
            }
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        self.ui_title_bar(ui, &p);

        egui::Panel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(p.surface)
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Title block.
                    ui.vertical(|ui| {
                        ui.label(RichText::new("NICEHCK").size(18.0).strong().color(p.text));
                        ui.label(RichText::new("耳机控制台").size(11.0).color(p.text_weak));
                    });

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // Connection state: a chip, not a sentence.
                    match &self.connected {
                        Some(c) => {
                            theme::chip(ui, &p, "● 已连接", p.ok);
                            ui.label(RichText::new(&c.product_name).strong());
                            ui.label(RichText::new(&c.path).size(11.0).color(p.text_weak));
                        }
                        None => {
                            theme::chip(ui, &p, "● 未连接", p.err);
                            ui.label(RichText::new("未检测到耳机").color(p.text_weak));
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!self.busy, egui::Button::new("重新连接"))
                            .clicked()
                        {
                            self.connect();
                        }
                        if self.busy {
                            ui.spinner();
                        }
                        if self.writes_unlocked {
                            theme::chip(ui, &p, "⚠ 可写入", p.warn);
                        } else {
                            theme::chip(ui, &p, "🔒 只读", p.reference);
                        }
                    });
                });
            });

        egui::Panel::top("tabs")
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(egui::Margin::symmetric(16, 6)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (tab, label) in [
                        (Tab::Equalizer, "均衡器"),
                        (Tab::Device, "设备"),
                        (Tab::About, "关于"),
                    ] {
                        let selected = self.tab == tab;
                        let text = RichText::new(label).size(14.0).color(if selected {
                            p.accent
                        } else {
                            p.text_weak
                        });
                        let mut btn = egui::Button::new(text).frame(false);
                        if selected {
                            // An underline reads as a tab; a filled pill would
                            // compete with the cards below.
                            btn = btn.stroke(egui::Stroke::NONE);
                        }
                        let resp = ui.add(btn);
                        if resp.clicked() {
                            self.tab = tab;
                        }
                        if selected {
                            let r = resp.rect;
                            ui.painter().hline(
                                r.x_range(),
                                r.bottom() + 2.0,
                                egui::Stroke::new(2.0, p.accent),
                            );
                        }
                    }
                });
            });

        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(p.surface)
                    .inner_margin(egui::Margin::symmetric(16, 6)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.writes_unlocked {
                        ui.colored_label(p.warn, "⚠ 写入已解锁");
                    } else {
                        ui.colored_label(p.reference, "🔒 只读模式");
                    }
                    ui.separator();
                    ui.label(RichText::new(format!("{} 段", self.bands.len())).color(p.text_weak));
                    if let Some(idx) = self.preset_index {
                        ui.separator();
                        let name = self
                            .connected
                            .as_ref()
                            .map(|c| c.preset_name(idx))
                            .unwrap_or_else(|| format!("预设 {idx}"));
                        ui.label(RichText::new(format!("当前预设 {name}")).color(p.text_weak));
                    }
                    if let Some(eq) = self.capability() {
                        ui.separator();
                        ui.label(
                            RichText::new(format!("采样率 {:.0} Hz", eq.sample_rate))
                                .color(p.text_weak),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mode = match ctx.theme() {
                            egui::Theme::Dark => "深色",
                            egui::Theme::Light => "浅色",
                        };
                        ui.label(
                            RichText::new(format!("{mode} · 跟随系统"))
                                .size(11.0)
                                .color(p.text_weak),
                        );
                    });
                });
            });

        egui::CentralPanel::default_margins()
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(egui::Margin::symmetric(16, 12)),
            )
            .show(ui, |ui| {
                if let Some(err) = self.error.clone() {
                    theme::callout(p.err).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(p.err, "⚠");
                            ui.label(&err);
                        });
                        if ui.button("知道了").clicked() {
                            self.error = None;
                        }
                    });
                    ui.add_space(10.0);
                }

                match self.tab {
                    Tab::Equalizer => self.ui_equalizer(ui, &p),
                    Tab::Device => self.ui_device(ui, &p),
                    Tab::About => ui_about(ui, &p),
                }
            });

        // Save-preset dialog: floats over everything.
        self.ui_save_dialog(&ctx, &p);

        // Live-write debounce: runs after the UI so slider changes are picked up.
        self.maybe_live_write(&ctx);
    }
}

impl NicehckApp {
    /// Custom title bar: replaces the OS chrome so the window chrome matches
    /// the app theme and the controls live in the same visual language.
    fn ui_title_bar(&mut self, ui: &mut egui::Ui, p: &theme::Palette) {
        let ctx = ui.ctx().clone();

        egui::Panel::top("title_bar")
            .exact_size(32.0)
            .frame(
                egui::Frame::new()
                    .fill(p.surface)
                    .inner_margin(egui::Margin::symmetric(10, 0)),
            )
            .show(ui, |ui| {
                // Whole bar is a drag handle; the controls below opt out by
                // being drawn after and using their own interaction.
                let bar_rect = ui.max_rect();
                let drag = ui.interact(
                    bar_rect,
                    ui.id().with("title_drag"),
                    egui::Sense::click_and_drag(),
                );
                if drag.dragged() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                // Double-click the bar to toggle maximise, the standard gesture.
                if drag.double_clicked() {
                    let maximised = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximised));
                }

                ui.horizontal(|ui| {
                    ui.label(RichText::new("NICEHCK").size(13.0).strong().color(p.accent));
                    ui.add_space(6.0);
                    ui.label(RichText::new("耳机控制台").size(11.0).color(p.text_weak));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Icons drawn with Painter: the CJK font we load has no
                        // dingbat glyphs, so text icons (✕ ▢ ❐) rendered as tofu.
                        let stroke_w = 1.4;
                        let icon_r = 5.0;

                        // Close (rightmost)
                        let (close_rect, close_resp) =
                            ui.allocate_exact_size(egui::vec2(30.0, 28.0), egui::Sense::click());
                        let painter = ui.painter();
                        if close_resp.hovered() {
                            painter.rect_filled(
                                close_rect,
                                egui::CornerRadius::same(theme::CHIP_RADIUS),
                                p.err.gamma_multiply(0.45),
                            );
                        }
                        let c = close_rect.center();
                        painter.line_segment(
                            [
                                c - egui::vec2(icon_r, icon_r),
                                c + egui::vec2(icon_r, icon_r),
                            ],
                            egui::Stroke::new(stroke_w, p.text),
                        );
                        painter.line_segment(
                            [
                                c + egui::vec2(icon_r, -icon_r),
                                c - egui::vec2(icon_r, -icon_r),
                            ],
                            egui::Stroke::new(stroke_w, p.text),
                        );
                        if close_resp.clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }

                        // Maximise / restore
                        let maximised = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                        let (max_rect, max_resp) =
                            ui.allocate_exact_size(egui::vec2(30.0, 28.0), egui::Sense::click());
                        let painter = ui.painter();
                        if max_resp.hovered() {
                            painter.rect_filled(
                                max_rect,
                                egui::CornerRadius::same(theme::CHIP_RADIUS),
                                p.text.gamma_multiply(0.15),
                            );
                        }
                        let c = max_rect.center();
                        let sk = egui::StrokeKind::Inside;
                        if maximised {
                            painter.rect_stroke(
                                egui::Rect::from_center_size(
                                    c + egui::vec2(-1.5, -1.5),
                                    egui::vec2(7.0, 7.0),
                                ),
                                egui::CornerRadius::ZERO,
                                egui::Stroke::new(1.2, p.text),
                                sk,
                            );
                            painter.rect_stroke(
                                egui::Rect::from_center_size(
                                    c + egui::vec2(1.5, 1.5),
                                    egui::vec2(7.0, 7.0),
                                ),
                                egui::CornerRadius::ZERO,
                                egui::Stroke::new(1.2, p.text),
                                sk,
                            );
                        } else {
                            painter.rect_stroke(
                                egui::Rect::from_center_size(c, egui::vec2(10.0, 10.0)),
                                egui::CornerRadius::ZERO,
                                egui::Stroke::new(1.2, p.text),
                                sk,
                            );
                        }
                        if max_resp.clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximised));
                        }

                        // Minimise
                        let (min_rect, min_resp) =
                            ui.allocate_exact_size(egui::vec2(30.0, 28.0), egui::Sense::click());
                        let painter = ui.painter();
                        if min_resp.hovered() {
                            painter.rect_filled(
                                min_rect,
                                egui::CornerRadius::same(theme::CHIP_RADIUS),
                                p.text.gamma_multiply(0.15),
                            );
                        }
                        let c = min_rect.center();
                        painter.line_segment(
                            [c - egui::vec2(5.0, 0.0), c + egui::vec2(5.0, 0.0)],
                            egui::Stroke::new(stroke_w, p.text),
                        );
                        if min_resp.clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                    });
                });
            });
    }

    /// Floating save-preset dialog with a name input.
    ///
    /// Uses `egui::Window` with a `TextEdit`: egui routes IME events from winit
    /// through the text cursor, so Chinese input works here without extra code.
    fn ui_save_dialog(&mut self, ctx: &egui::Context, p: &theme::Palette) {
        if !self.show_save_dialog {
            return;
        }
        let mut open = self.show_save_dialog;
        egui::Window::new("保存自定义预设")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(RichText::new("预设名称").color(p.text_weak));
                ui.add_space(4.0);
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.save_name)
                        .hint_text("自定义-1")
                        .desired_width(260.0),
                );
                // Focus on open so the user can type immediately.
                resp.request_focus();
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!(
                        "将保存 {} 个频段，offset {:+.1} dB",
                        self.save_bands.len(),
                        self.save_offset
                    ))
                    .size(11.0)
                    .color(p.text_weak),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(RichText::new("保存").color(p.bg)).fill(p.accent))
                        .clicked()
                    {
                        self.commit_save_preset();
                    }
                    if ui.button("取消").clicked() {
                        self.show_save_dialog = false;
                    }
                });
            });
        if !open {
            self.show_save_dialog = false;
        }
    }

    fn ui_equalizer(&mut self, ui: &mut egui::Ui, p: &theme::Palette) {
        // The EQ page can overflow: preset row + curve card + 8-band grid
        // together exceed a short viewport. Wrapping it in a vertical
        // ScrollArea lets the whole page scroll instead of clipping.
        egui::ScrollArea::vertical()
            .id_salt("eq_scroll")
            .auto_shrink(false)
            .show(ui, |ui| {
                self.ui_equalizer_inner(ui, p);
            });
    }

    fn ui_equalizer_inner(&mut self, ui: &mut egui::Ui, p: &theme::Palette) {
        let cap = self.capability();
        let (min_offset, max_offset) = cap
            .as_ref()
            .map(|c| (c.min_offset, c.max_offset))
            .unwrap_or((-12.0, 8.0));

        // ── Preset chips ────────────────────────────────────────────────
        // The factory presets are the app's primary navigation, so they get a
        // full-width scrollable row rather than living behind a dropdown.
        if let Some(eq) = cap.as_ref().filter(|e| !e.presets.is_empty()) {
            ui.label(RichText::new("预设").size(12.0).color(p.text_weak));
            ui.add_space(4.0);
            egui::ScrollArea::horizontal()
                .id_salt("presets")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for preset in &eq.presets {
                            let active = self.preset_index == Some(preset.preset_index);
                            let text = RichText::new(preset.display_name())
                                .size(13.0)
                                .color(if active { p.bg } else { p.text });
                            let fill = if active { p.accent } else { p.surface_alt };
                            let btn = egui::Button::new(text).fill(fill).stroke(egui::Stroke::new(
                                1.0,
                                if active { p.accent } else { p.border },
                            ));
                            let resp = ui.add(btn);
                            if resp.clicked() {
                                // Selecting a preset is a local preview: it
                                // moves the sliders and offset, nothing is
                                // written until Apply.
                                self.apply_preset(preset);
                            }
                            if !active {
                                resp.on_hover_text(format!(
                                    "{}  ·  offset {:+.1} dB  ·  编号 {}",
                                    preset.preset_key, preset.offset, preset.preset_index
                                ));
                            } else {
                                resp.on_hover_text("当前预设");
                            }
                        }
                    });
                });
            ui.add_space(10.0);
        }

        // ── Custom presets ──────────────────────────────────────────────
        if !self.custom_presets.is_empty() {
            ui.label(RichText::new("自定义预设").size(12.0).color(p.text_weak));
            ui.add_space(4.0);
            egui::ScrollArea::horizontal()
                .id_salt("custom_presets")
                .auto_shrink(false)
                .show(ui, |ui| {
                    // Collect the action first: the loop borrows
                    // `self.custom_presets` immutably, while the callbacks need
                    // `&mut self`. Executing after the loop avoids the conflict.
                    let mut action: Option<(usize, bool)> = None; // (idx, is_delete)
                    ui.horizontal(|ui| {
                        for (idx, cp) in self.custom_presets.iter().enumerate() {
                            let text = RichText::new(&cp.name).size(13.0);
                            let btn = egui::Button::new(text)
                                .fill(p.surface_alt)
                                .stroke(egui::Stroke::new(1.0, p.border));
                            if ui.add(btn).clicked() {
                                action = Some((idx, false));
                            }
                            let del = ui.add(
                                egui::Button::new(RichText::new("×").size(11.0).color(p.text_weak))
                                    .frame(false)
                                    .min_size(egui::vec2(18.0, 18.0)),
                            );
                            if del.clicked() {
                                action = Some((idx, true));
                            }
                        }
                    });
                    match action {
                        Some((idx, false)) => self.load_custom_preset(idx),
                        Some((idx, true)) => self.delete_custom_preset(idx),
                        None => {}
                    }
                });
            ui.add_space(10.0);
        }

        // ── Response curve ──────────────────────────────────────────────
        theme::card(ui, p).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("频响曲线").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(!self.busy, egui::Button::new("从设备读取 EQ"))
                        .on_hover_text("从设备读取 EQ")
                        .clicked()
                    {
                        self.refresh_eq();
                    }
                    ui.checkbox(&mut self.auto_refresh, "自动刷新");
                });
            });
            ui.label(
                RichText::new("按 DSP 实际传输函数计算，非示意曲线")
                    .size(11.0)
                    .color(p.text_weak),
            );
            ui.add_space(6.0);

            if self.bands.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.label(RichText::new("尚未读取数据 — 点右上角「读取」").color(p.text_weak));
                    ui.add_space(40.0);
                });
            } else {
                self.ui_plot(ui, p);
            }
        });

        if self.bands.is_empty() {
            return;
        }

        ui.add_space(10.0);

        // ── Bands ───────────────────────────────────────────────────────
        let (min_gain, max_gain, min_q, max_q) = cap
            .as_ref()
            .map(|c| (c.min_gain, c.max_gain, c.min_q_value, c.max_q_value))
            .unwrap_or((-12.0, 12.0, 0.2, 12.0));

        theme::card(ui, p).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("频段").strong());
                ui.label(
                    RichText::new(format!("共 {} 段", self.bands.len()))
                        .size(11.0)
                        .color(p.text_weak),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Offset is a global pre-gain, so it sits with Apply
                    // rather than with the per-band sliders.
                    let r_off = ui.add(
                        egui::Slider::new(&mut self.eq_offset_db, min_offset..=max_offset)
                            .suffix(" dB")
                            .fixed_decimals(1)
                            .text("Offset"),
                    );
                    if r_off.changed() {
                        self.mark_edited(ui.ctx());
                    }
                });
            });
            ui.label(
                RichText::new("拖动即时预览；只有点「写入耳机」才会改变声音")
                    .size(11.0)
                    .color(p.text_weak),
            );
            ui.add_space(8.0);

            egui::Grid::new("bands")
                .num_columns(4)
                .spacing([14.0, 8.0])
                .striped(true)
                .show(ui, |ui| {
                    for h in ["频率", "增益", "Q 值", "增益趋势"] {
                        ui.label(RichText::new(h).size(11.0).color(p.text_weak));
                    }
                    ui.end_row();

                    for band in self.bands.iter_mut() {
                        ui.label(
                            RichText::new(format!("{}", band.index))
                                .monospace()
                                .color(p.accent),
                        );
                        ui.add(
                            egui::DragValue::new(&mut band.frequency)
                                .range(20..=20_000)
                                .suffix(" Hz")
                                .speed(5.0),
                        );
                        ui.add(
                            egui::Slider::new(&mut band.gain, min_gain..=max_gain)
                                .suffix(" dB")
                                .fixed_decimals(1),
                        );
                        ui.add(
                            egui::Slider::new(&mut band.q, min_q..=max_q)
                                .fixed_decimals(2)
                                .max_decimals(2),
                        );
                        // A signed bar makes the EQ shape visible without
                        // reading numbers, the way the app's sliders do.
                        ui.add_sized(
                            egui::vec2(80.0, 20.0),
                            egui::ProgressBar::new(
                                ((band.gain - min_gain) / (max_gain - min_gain)).clamp(0.0, 1.0),
                            )
                            .show_percentage(),
                        );
                        ui.end_row();
                    }
                });

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);

            let can_write = self.writes_unlocked && !self.busy;
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        can_write,
                        egui::Button::new(RichText::new("写入耳机").color(if can_write {
                            p.bg
                        } else {
                            p.text_weak
                        }))
                        .fill(if can_write { p.accent } else { p.surface_alt })
                        .min_size(egui::vec2(110.0, 30.0)),
                    )
                    .on_disabled_hover_text("请先在「设备」页解锁写入")
                    .clicked()
                {
                    self.apply_eq();
                }
                if !self.writes_unlocked {
                    ui.label(RichText::new("写入未解锁").size(11.0).color(p.warn));
                }

                ui.add_space(12.0);

                // Save current tuning as a named custom preset.
                if ui
                    .add_enabled(
                        !self.bands.is_empty(),
                        egui::Button::new(RichText::new("保存预设").color(p.text))
                            .fill(p.surface_alt),
                    )
                    .clicked()
                {
                    self.open_save_dialog();
                }
            });
        });
    }

    /// Load a factory preset into the sliders as a local preview.
    fn apply_preset(&mut self, preset: &device_config::Preset) {
        // Presets carry an offset and up to `frequency_number` bands. The stored
        // band list may be shorter than the device's band count, so match by
        // index and leave anything unmatched alone rather than guessing.
        self.preset_index = Some(preset.preset_index);
        self.eq_offset_db = preset.offset;
        for target in self.bands.iter_mut() {
            if let Some(src) = preset.bands.iter().find(|b| b.band_index == target.index) {
                target.frequency = src.frequency;
                target.gain = src.gain;
                target.q = src.q_value;
            }
        }
    }

    /// Draw the combined frequency-response curve and band control points.
    fn ui_plot(&self, ui: &mut egui::Ui, p: &theme::Palette) {
        let points = curve::curve_points(
            &self.bands,
            self.eq_offset_db as f64,
            self.sample_rate,
            20.0,
            20_000.0,
            320,
        );
        let (lo, hi) = curve::curve_range(&points);
        let y_lo = (lo - 2.0).min(-1.0);
        let y_hi = (hi + 2.0).max(1.0);

        let centres: Vec<[f64; 2]> = self
            .bands
            .iter()
            .map(|b| {
                [
                    b.frequency as f64,
                    curve::combined_response_db(
                        &self.bands,
                        self.eq_offset_db as f64,
                        b.frequency as f64,
                        self.sample_rate,
                    ),
                ]
            })
            .collect();

        egui_plot::Plot::new("response")
            .height(250.0)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .include_y(y_lo)
            .include_y(y_hi)
            // A linear frequency axis matches the app and keeps the band spacing
            // readable; the labels still use proper kHz units.
            .x_axis_formatter(|mark, _| {
                let v = mark.value;
                if v >= 1000.0 {
                    format!("{:.0}k", v / 1000.0)
                } else {
                    format!("{v:.0}")
                }
            })
            .y_axis_formatter(|mark, _| format!("{:+.0}", mark.value))
            .label_formatter(|pos| match pos {
                egui_plot::HoverPosition::NearDataPoint {
                    plot_name,
                    position,
                    ..
                } => Some(if plot_name.is_empty() {
                    format!("{:.0} Hz\n{:+.2} dB", position.x, position.y)
                } else {
                    format!("{plot_name}\n{:.0} Hz\n{:+.2} dB", position.x, position.y)
                }),
                egui_plot::HoverPosition::Elsewhere { position } => {
                    Some(format!("{:.0} Hz\n{:+.2} dB", position.x, position.y))
                }
            })
            .show(ui, |plot_ui| {
                plot_ui.hline(
                    egui_plot::HLine::new("0 dB", 0.0)
                        .color(p.reference)
                        .style(egui_plot::LineStyle::dashed_loose()),
                );
                // A soft fill under the curve reads as a level meter and makes
                // the overall tilt obvious at a glance.
                plot_ui.line(
                    Line::new("Response", PlotPoints::from(points.clone()))
                        .color(p.accent)
                        .width(2.0),
                );
                plot_ui.points(
                    egui_plot::Points::new("Band", PlotPoints::from(centres))
                        .color(p.warn)
                        .radius(4.0),
                );
            });
    }

    fn ui_device(&mut self, ui: &mut egui::Ui, p: &theme::Palette) {
        let Some(c) = self.connected.clone() else {
            theme::callout(p.warn).show(ui, |ui| {
                ui.label("未连接到耳机，请插入设备后点击「重新连接」。");
            });
            return;
        };

        egui::ScrollArea::vertical()
            .id_salt("device_scroll")
            .auto_shrink(false)
            .show(ui, |ui| {
                // ── Identity card ───────────────────────────────────────────
                theme::card(ui, p).show(ui, |ui| {
                    ui.label(RichText::new("设备信息").strong());
                    ui.add_space(6.0);
                    egui::Grid::new("identity")
                        .num_columns(2)
                        .spacing([20.0, 6.0])
                        .show(ui, |ui| {
                            let row = |ui: &mut egui::Ui, k: &str, v: &str| {
                                ui.label(RichText::new(k).color(p.text_weak));
                                ui.label(RichText::new(v).monospace());
                                ui.end_row();
                            };
                            row(ui, "设备名称", &c.product_name);
                            row(ui, "设备地址", &c.path);
                            row(
                                ui,
                                "USB ID",
                                &format!("{:04X}:{:04X}", c.vendor_id, c.product_id),
                            );
                            if let Some(k) = &c.config {
                                row(ui, "型号", &k.device.name);
                                row(ui, "协议", &k.device.runtime.protocol_variant);
                                row(ui, "控制器", &k.device.runtime.controller_kind);
                            }
                        });
                });

                // ── EQ capability card ──────────────────────────────────────
                if let Some(eq) = c.capability() {
                    ui.add_space(10.0);
                    theme::card(ui, p).show(ui, |ui| {
                        ui.label(RichText::new("均衡器参数").strong());
                        ui.add_space(6.0);
                        egui::Grid::new("eqcap")
                            .num_columns(2)
                            .spacing([20.0, 6.0])
                            .show(ui, |ui| {
                                let row = |ui: &mut egui::Ui, k: &str, v: String| {
                                    ui.label(RichText::new(k).color(p.text_weak));
                                    ui.label(RichText::new(v).monospace());
                                    ui.end_row();
                                };
                                row(ui, "频段数量", format!("{}", eq.frequency_number));
                                row(
                                    ui,
                                    "频率范围",
                                    format!(
                                        "{:.0} {} {:.0} Hz",
                                        eq.min_frequency,
                                        theme::DASH,
                                        eq.max_frequency
                                    ),
                                );
                                row(
                                    ui,
                                    "增益范围",
                                    format!("{:+.0} .. {:+.0} dB", eq.min_gain, eq.max_gain),
                                );
                                row(
                                    ui,
                                    "Q 值范围",
                                    format!("{:.2} .. {:.2}", eq.min_q_value, eq.max_q_value),
                                );
                                row(
                                    ui,
                                    "Offset 范围",
                                    format!("{:+.0} .. {:+.0} dB", eq.min_offset, eq.max_offset),
                                );
                                row(ui, "采样率", format!("{:.0} Hz", eq.sample_rate));
                            });

                        ui.add_space(10.0);
                        ui.label(RichText::new("设备预设").strong());
                        ui.add_space(4.0);
                        egui::Grid::new("presets")
                            .num_columns(3)
                            .spacing([14.0, 4.0])
                            .show(ui, |ui| {
                                for preset in &eq.presets {
                                    theme::chip(ui, p, &preset.display_name(), p.accent);
                                    ui.label(
                                        RichText::new(&preset.preset_key)
                                            .monospace()
                                            .color(p.text_weak),
                                    );
                                    ui.label(
                                        RichText::new(format!("offset {:+.1} dB", preset.offset))
                                            .size(11.0)
                                            .color(p.text_weak),
                                    );
                                    ui.end_row();
                                }
                            });
                    });
                }

                // ── Write gate card ─────────────────────────────────────────
                ui.add_space(10.0);
                theme::callout(if self.writes_unlocked {
                    p.warn
                } else {
                    p.border
                })
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(p.warn, "⚠");
                        ui.label("写入会立即改变 DSP 参数，耳机不保存备份，请先记录当前设置。");
                    });
                    ui.add_space(6.0);
                    let label = if self.writes_unlocked {
                        "🔒 锁定写入"
                    } else {
                        "🔓 解锁写入"
                    };
                    if ui
                        .button(RichText::new(label).color(if self.writes_unlocked {
                            p.text
                        } else {
                            p.warn
                        }))
                        .clicked()
                    {
                        self.writes_unlocked = !self.writes_unlocked;
                    }
                });

                // ── Log card ────────────────────────────────────────────────
                ui.add_space(10.0);
                theme::card(ui, p).show(ui, |ui| {
                    ui.label(RichText::new("操作日志").strong());
                    ui.add_space(4.0);
                    if self.log.is_empty() {
                        ui.label(RichText::new("尚无操作").color(p.text_weak));
                    } else {
                        egui::ScrollArea::vertical()
                            .id_salt("device_log_scroll")
                            .max_height(140.0)
                            .show(ui, |ui| {
                                for line in self.log.iter().rev() {
                                    ui.label(RichText::new(line).monospace().color(p.text_weak));
                                }
                            });
                    }
                });
            });
    }
}

fn ui_about(ui: &mut egui::Ui, p: &theme::Palette) {
    egui::ScrollArea::vertical()
        .id_salt("about_scroll")
        .auto_shrink(false)
        .show(ui, |ui| {
        theme::card(ui, p).show(ui, |ui| {
            ui.label(
                RichText::new("NICEHCK 耳机控制台")
                    .size(16.0)
                    .strong()
                    .color(p.text),
            );
            ui.label(
                RichText::new(format!(
                    "版本 {} · 原生 Linux 控制工具",
                    env!("CARGO_PKG_VERSION")
                ))
                .color(p.text_weak),
            );
            ui.add_space(10.0);
            ui.label(
                "本工具通过 /dev/hidraw 与厂商 USB HID 协议通信，由官方 Android 应用（com.yuandao.nicehck 2.3.8）静态逆向分析得出，不含任何厂商文档。",
            );
            ui.add_space(10.0);
            ui.label(RichText::new("协议要点").strong());
            for b in [
                "HID 报告 ID 0x4B（控制）/ 0x54（固件），报告长度 64 字节",
                "写入帧：4B 01 <opcode> <arg> <payload…>",
                "读取帧：4B 80 <opcode>",
                "固件通道帧带 BUXX 魔数和 CRC-32 校验",
                "EQ 频段采用 RBJ peaking biquad，系数量化至 signed Q30",
            ] {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("·").color(p.accent));
                    ui.label(b);
                });
            }
            ui.add_space(10.0);
            ui.label(RichText::new("字体").strong());
            if let Some(font) = crate::fonts::describe() {
                ui.label(RichText::new(format!("中文字体：{}", font.file)).monospace());
                ui.label(
                    RichText::new(format!(
                        "来源：{} · face {}",
                        font.source, font.index
                    ))
                    .color(p.text_weak),
                );
            } else {
                ui.colored_label(p.warn, "未找到中文字体，中文可能显示为方块。");
            }
            ui.add_space(10.0);
            ui.label(RichText::new("权限").strong());
            ui.label("hidraw 节点默认为 root 所有，需安装 udev 规则：");
            ui.label(RichText::new("sudo cp udev/70-nicehck.rules /etc/udev/rules.d/").monospace());
            ui.label(RichText::new("sudo udevadm control --reload-rules && sudo udevadm trigger").monospace());
            ui.add_space(10.0);
            ui.label(
                RichText::new("与 NICEHCK / YUANDAO 无任何隶属关系，使用风险自负。")
                    .size(11.0)
                    .color(p.text_weak),
            );
        });
    });
}
