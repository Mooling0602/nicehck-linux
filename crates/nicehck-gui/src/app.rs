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

use egui::{Color32, RichText};
use egui_plot::{Line, PlotPoints};
use nicehck_protocol::command::{self, Band, EqState};
use nicehck_protocol::device_config::{self, DeviceConfigFile, EqualizerCapability};
use nicehck_protocol::transport::{self, HidDevice, TransportError};

use crate::curve;
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
}

impl NicehckApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());

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
            match dev.request(&reports, IO_TIMEOUT) {
                Ok(_) => {
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
}

impl eframe::App for NicehckApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump();

        if self.auto_refresh && !self.busy && self.connected.is_some() {
            let now = ctx.input(|i| i.time);
            if now - self.last_refresh > AUTO_REFRESH_SECS {
                self.last_refresh = now;
                self.refresh_eq();
            }
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        egui::Panel::top("header").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("NICEHCK Headset Control");
                ui.separator();
                match &self.connected {
                    Some(c) => {
                        ui.colored_label(theme::OK, "●");
                        ui.label(RichText::new(&c.product_name).strong());
                        ui.weak(&c.path);
                    }
                    None => {
                        ui.colored_label(theme::ERR, "●");
                        ui.label("not connected");
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.busy {
                        ui.spinner();
                    }
                    if ui
                        .add_enabled(!self.busy, egui::Button::new("Reconnect"))
                        .clicked()
                    {
                        self.connect();
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Equalizer, "Equalizer");
                ui.selectable_value(&mut self.tab, Tab::Device, "Device");
                ui.selectable_value(&mut self.tab, Tab::About, "About");
            });
            ui.add_space(6.0);
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if self.writes_unlocked {
                    ui.colored_label(theme::WARN, "⚠ writes UNLOCKED");
                } else {
                    ui.colored_label(theme::REFERENCE, "🔒 read-only");
                }
                ui.separator();
                ui.label(format!("{} band(s)", self.bands.len()));
                if let Some(p) = self.preset_index {
                    ui.separator();
                    let name = self
                        .connected
                        .as_ref()
                        .map(|c| c.preset_name(p))
                        .unwrap_or_else(|| format!("preset {p}"));
                    ui.label(format!("Active: {name}"));
                }
                if let Some(eq) = self.capability() {
                    ui.separator();
                    ui.weak(format!("Fs {:.0} Hz", eq.sample_rate));
                }
            });
            ui.add_space(2.0);
        });

        egui::CentralPanel::default_margins().show(ui, |ui| {
            if let Some(err) = self.error.clone() {
                ui.add_space(8.0);
                egui::Frame::group(ui.style())
                    .fill(Color32::from_rgb(0x3A, 0x22, 0x22))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(theme::ERR, "⚠");
                            ui.label(&err);
                        });
                        if ui.button("Dismiss").clicked() {
                            self.error = None;
                        }
                    });
                ui.add_space(8.0);
            }

            match self.tab {
                Tab::Equalizer => self.ui_equalizer(ui),
                Tab::Device => self.ui_device(ui),
                Tab::About => ui_about(ui),
            }
        });
    }
}

impl NicehckApp {
    fn ui_equalizer(&mut self, ui: &mut egui::Ui) {
        let cap = self.capability();
        let (min_offset, max_offset) = cap
            .as_ref()
            .map(|c| (c.min_offset, c.max_offset))
            .unwrap_or((-12.0, 8.0));

        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.busy, egui::Button::new("↻ Read from device"))
                .clicked()
            {
                self.refresh_eq();
            }
            ui.checkbox(&mut self.auto_refresh, "Auto");
            ui.separator();
            ui.label("Offset");
            ui.add(
                egui::Slider::new(&mut self.eq_offset_db, min_offset..=max_offset)
                    .suffix(" dB")
                    .fixed_decimals(1),
            );
            ui.separator();
            let can_write = self.writes_unlocked && !self.busy;
            if ui
                .add_enabled(can_write, egui::Button::new("Apply to device"))
                .on_disabled_hover_text("Unlock writes on the Device tab first")
                .clicked()
            {
                self.apply_eq();
            }
        });

        ui.add_space(6.0);

        if self.bands.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.colored_label(
                    theme::REFERENCE,
                    "No EQ data yet — press “Read from device”.",
                );
            });
            return;
        }

        self.ui_plot(ui);

        ui.add_space(8.0);
        ui.separator();
        ui.label(RichText::new("Bands").strong());
        ui.weak("Edits preview immediately; they reach the headset only on Apply.");
        ui.add_space(4.0);

        let (min_gain, max_gain, min_q, max_q) = cap
            .as_ref()
            .map(|c| (c.min_gain, c.max_gain, c.min_q_value, c.max_q_value))
            .unwrap_or((-12.0, 12.0, 0.2, 12.0));

        egui::ScrollArea::vertical().show(ui, |ui| {
            for band in self.bands.iter_mut() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{:>2}", band.index)).monospace());
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
                    ui.label("Q");
                    ui.add(
                        egui::Slider::new(&mut band.q, min_q..=max_q)
                            .fixed_decimals(2)
                            .max_decimals(2),
                    );
                });
            }
        });
    }

    /// Draw the combined frequency-response curve and band control points.
    fn ui_plot(&self, ui: &mut egui::Ui) {
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
            .height(260.0)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .include_y(y_lo)
            .include_y(y_hi)
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
                        .color(theme::REFERENCE)
                        .style(egui_plot::LineStyle::dashed_loose()),
                );
                plot_ui.line(
                    Line::new("Response", PlotPoints::from(points))
                        .color(theme::ACCENT)
                        .width(2.0),
                );
                plot_ui.points(
                    egui_plot::Points::new("Band", PlotPoints::from(centres))
                        .color(theme::WARN)
                        .radius(4.0),
                );
            });
    }

    fn ui_device(&mut self, ui: &mut egui::Ui) {
        let Some(c) = self.connected.clone() else {
            ui.colored_label(theme::REFERENCE, "Not connected.");
            return;
        };

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(RichText::new("Identity").strong());
            egui::Grid::new("identity")
                .num_columns(2)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    ui.label("Product");
                    ui.label(&c.product_name);
                    ui.end_row();
                    ui.label("Node");
                    ui.monospace(&c.path);
                    ui.end_row();
                    ui.label("USB ID");
                    ui.monospace(format!("{:04X}:{:04X}", c.vendor_id, c.product_id));
                    ui.end_row();
                    if let Some(k) = &c.config {
                        ui.label("Model");
                        ui.label(&k.device.name);
                        ui.end_row();
                        ui.label("Protocol");
                        ui.monospace(&k.device.runtime.protocol_variant);
                        ui.end_row();
                        ui.label("Controller");
                        ui.monospace(&k.device.runtime.controller_kind);
                        ui.end_row();
                    }
                });

            if let Some(eq) = c.capability() {
                ui.add_space(12.0);
                ui.separator();
                ui.label(RichText::new("Equalizer capability").strong());
                egui::Grid::new("eqcap")
                    .num_columns(2)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("Bands");
                        ui.label(format!("{}", eq.frequency_number));
                        ui.end_row();
                        ui.label("Frequency range");
                        ui.label(format!(
                            "{:.0} {dash} {:.0} Hz",
                            eq.min_frequency,
                            eq.max_frequency,
                            dash = theme::DASH
                        ));
                        ui.end_row();
                        ui.label("Gain range");
                        ui.label(format!("{:+.0} .. {:+.0} dB", eq.min_gain, eq.max_gain));
                        ui.end_row();
                        ui.label("Q range");
                        ui.label(format!("{:.2} .. {:.2}", eq.min_q_value, eq.max_q_value));
                        ui.end_row();
                        ui.label("Offset range");
                        ui.label(format!("{:+.0} .. {:+.0} dB", eq.min_offset, eq.max_offset));
                        ui.end_row();
                        ui.label("Sample rate");
                        ui.label(format!("{:.0} Hz", eq.sample_rate));
                        ui.end_row();
                    });

                ui.add_space(8.0);
                ui.label(RichText::new("Factory presets").strong());
                for p in &eq.presets {
                    ui.horizontal(|ui| {
                        ui.monospace(format!("[{:>2}]", p.preset_index));
                        ui.label(RichText::new(p.display_name()).strong());
                        ui.weak(format!("({}) offset {:+.1} dB", p.preset_key, p.offset));
                    });
                }
            }

            ui.add_space(12.0);
            ui.separator();
            ui.label(RichText::new("Writes").strong());
            egui::Frame::group(ui.style())
                .fill(Color32::from_rgb(0x33, 0x2C, 0x1C))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(theme::WARN, "⚠");
                        ui.label(
                            "Writing changes the DSP immediately and is audible. \
                             The headset stores no backup, so note your current settings first.",
                        );
                    });
                    ui.add_space(4.0);
                    let label = if self.writes_unlocked {
                        "Lock writes"
                    } else {
                        "Unlock writes for this session"
                    };
                    if ui.button(label).clicked() {
                        self.writes_unlocked = !self.writes_unlocked;
                    }
                });

            ui.add_space(12.0);
            ui.separator();
            ui.label(RichText::new("Log").strong());
            egui::ScrollArea::vertical()
                .max_height(140.0)
                .show(ui, |ui| {
                    for line in self.log.iter().rev() {
                        ui.monospace(line);
                    }
                });
        });
    }
}

fn ui_about(ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.label(RichText::new("About").strong());
        ui.add_space(4.0);
        ui.label(format!(
            "nicehck-linux {} — native control for NICEHCK / YUANDAO USB-C DSP earphones.",
            env!("CARGO_PKG_VERSION")
        ));
        ui.add_space(8.0);
        ui.label(
            "This tool speaks the vendor's USB HID protocol over /dev/hidraw. It was \
             produced by statically analysing the official Android app \
             (com.yuandao.nicehck 2.3.8); no vendor documentation is involved.",
        );
        ui.add_space(8.0);
        ui.label(RichText::new("Protocol highlights").strong());
        for b in [
            "HID report ID 0x4B (control) / 0x54 (firmware), 64-byte reports",
            "Write frame: 4B 01 <opcode> <arg> <payload…>",
            "Read frame:  4B 80 <opcode>",
            "Firmware channel frames carry magic BUXX and CRC-32",
            "EQ bands are RBJ peaking biquads quantised to signed Q30",
        ] {
            ui.horizontal_wrapped(|ui| {
                ui.label("•");
                ui.label(b);
            });
        }
        ui.add_space(8.0);
        ui.label(RichText::new("Permissions").strong());
        ui.label("The hidraw node is root-only by default. Install the bundled rule:");
        ui.monospace("sudo cp udev/70-nicehck.rules /etc/udev/rules.d/");
        ui.monospace("sudo udevadm control --reload-rules && sudo udevadm trigger");
        ui.add_space(8.0);
        ui.weak("Not affiliated with NICEHCK or YUANDAO. Use at your own risk.");
    });
}
