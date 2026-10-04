//! `nicehck` — command-line control for NICEHCK / YUANDAO USB-C DSP earphones.
//!
//! This build focuses on safe, read-only inspection so the recovered protocol can
//! be validated against real hardware before anything writes to the DSP. Write
//! subcommands are gated behind `--write` and an explicit confirmation.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use nicehck_protocol::command::{self, Opcode};
use nicehck_protocol::device_config::{self, DeviceConfigFile};
use nicehck_protocol::transport::{self, HidDevice};

#[derive(Parser, Debug)]
#[command(
    name = "nicehck",
    version,
    about = "Configure NICEHCK / YUANDAO USB-C DSP earphones on Linux",
    long_about = "Talks the vendor's USB HID protocol (reverse-engineered from the \
                  Android app) over /dev/hidraw.\n\n\
                  Reads are always safe. Writes need --write and will change what you hear."
)]
struct Cli {
    /// hidraw node to use, e.g. /dev/hidraw3 (default: first headset found)
    #[arg(short, long, global = true)]
    device: Option<String>,

    /// Reply timeout in milliseconds
    #[arg(long, global = true, default_value_t = 1200)]
    timeout: u64,

    /// Print raw report bytes as hex
    #[arg(long, global = true)]
    hexdump: bool,

    /// Emit machine-readable JSON
    #[arg(long, global = true)]
    json: bool,

    /// Allow commands that modify the device
    #[arg(long, global = true)]
    write: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List attached NICEHCK/YUANDAO headsets and known device models
    List,

    /// Show device identity, USB IDs and supported features
    Info,

    /// Read the current equaliser state from the device
    Eq,

    /// Read firmware version, volume, filter, gain, work mode and balance
    Status,

    /// Toggle writes (demonstrates the safety gate; performs no I/O)
    Unlock,
}

/// Millisecond sleep the vendor app inserts between staged commands.
const STAGED_DELAY: Duration = Duration::from_millis(20);

fn main() -> Result<()> {
    let cli = Cli::parse();
    let timeout = Duration::from_millis(cli.timeout);

    match &cli.command {
        Command::List => cmd_list(&cli),
        Command::Info => cmd_info(&cli),
        Command::Eq => cmd_eq(&cli, timeout),
        Command::Status => cmd_status(&cli, timeout),
        Command::Unlock => {
            println!("Write commands are gated behind --write.");
            println!("This build performs read-only inspection.");
            Ok(())
        }
    }
}

/// Open the configured device, or the first headset found.
fn open_device(cli: &Cli) -> Result<HidDevice> {
    match &cli.device {
        Some(path) => {
            let all = transport::enumerate(device_config::VENDOR_ID);
            let info = all
                .iter()
                .find(|i| i.path.to_string_lossy() == path.as_str())
                .with_context(|| {
                    format!(
                        "{path} is not a NICEHCK/YUANDAO hidraw node; use `nicehck list` to see candidates"
                    )
                })?;
            Ok(HidDevice::open_path(info)?)
        }
        None => Ok(HidDevice::open_first()?),
    }
}

fn cmd_list(cli: &Cli) -> Result<()> {
    let attached = transport::enumerate(device_config::VENDOR_ID);

    if cli.json {
        let entries: Vec<_> = attached
            .iter()
            .map(|info| {
                let known = device_config::find_usb_device(info.vendor_id, info.product_id);
                serde_json::json!({
                    "path": info.path,
                    "vendorId": format!("0x{:04X}", info.vendor_id),
                    "productId": format!("0x{:04X}", info.product_id),
                    "productName": info.product_name,
                    "knownModel": known.as_ref().map(|k| k.device.name.clone()),
                    "deviceId": known.as_ref().map(|k| k.device.id),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    println!("Attached NICEHCK/YUANDAO headsets:");
    if attached.is_empty() {
        println!("  (none — is the headset plugged in?)");
    }
    for info in &attached {
        match device_config::find_usb_device(info.vendor_id, info.product_id) {
            Some(cfg) => println!(
                "  {}  {:04X}:{:04X}  {}  [model: {} (id {})]",
                info.path.display(),
                info.vendor_id,
                info.product_id,
                info.product_name,
                cfg.device.name,
                cfg.device.id
            ),
            None => println!(
                "  {}  {:04X}:{:04X}  {}  [unknown model]",
                info.path.display(),
                info.vendor_id,
                info.product_id,
                info.product_name
            ),
        }
    }

    println!("\nModels known to this build:");
    for dev in device_config::all_devices() {
        match dev.usb_id() {
            Some((vid, pid)) => {
                let features: Vec<&str> = dev.features.iter().map(|f| f.key.as_str()).collect();
                println!(
                    "  [{:04X}:{:04X}] {} ({}) — {}",
                    vid,
                    pid,
                    dev.device.name,
                    dev.device.runtime.protocol_variant,
                    features.join(", ")
                );
            }
            None => println!(
                "  [bluetooth] {} ({})",
                dev.device.name, dev.device.runtime.protocol_variant
            ),
        }
    }
    Ok(())
}

fn cmd_info(cli: &Cli) -> Result<()> {
    let dev = open_device(cli)?;
    let (vid, pid) = (dev.vendor_id(), dev.product_id());
    let known: Option<DeviceConfigFile> = device_config::find_usb_device(vid, pid);

    if cli.json {
        let features = known
            .as_ref()
            .map(|k| k.features.clone())
            .unwrap_or_default();
        let out = serde_json::json!({
            "path": dev.path(),
            "vendorId": format!("0x{vid:04X}"),
            "productId": format!("0x{pid:04X}"),
            "productName": dev.product_name(),
            "model": known.as_ref().map(|k| &k.device.name),
            "protocolVariant": known.as_ref().map(|k| &k.device.runtime.protocol_variant),
            "controllerKind": known.as_ref().map(|k| &k.device.runtime.controller_kind),
            "features": features,
            "equalizer": known.as_ref().and_then(|k| k.equalizer()),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("Device:      {}", dev.product_name());
    println!("Node:        {}", dev.path().display());
    println!("USB ID:      {vid:04X}:{pid:04X}");
    match known {
        Some(cfg) => {
            println!("Model:       {} (id {})", cfg.device.name, cfg.device.id);
            println!(
                "Protocol:    {} / {}",
                cfg.device.runtime.protocol_variant, cfg.device.runtime.controller_kind
            );
            println!("Features:");
            for f in &cfg.features {
                println!(
                    "  - {}{}",
                    f.key,
                    f.behavior_variant
                        .as_deref()
                        .map(|v| format!(" (variant: {v})"))
                        .unwrap_or_default()
                );
            }
            if let Some(eq) = cfg.equalizer() {
                println!("Equaliser:");
                println!(
                    "  {} bands, {:.0}–{:.0} Hz, gain {:.0}..{:.0} dB, Q {}..{}, offset {}..{} dB, Fs {:.0} Hz",
                    eq.frequency_number,
                    eq.min_frequency,
                    eq.max_frequency,
                    eq.min_gain,
                    eq.max_gain,
                    eq.min_q_value,
                    eq.max_q_value,
                    eq.min_offset,
                    eq.max_offset,
                    eq.sample_rate,
                );
                println!("  Presets:");
                for p in &eq.presets {
                    println!(
                        "    [{}] {} ({}) offset {:+.1} dB",
                        p.preset_index,
                        p.display_name(),
                        p.preset_key,
                        p.offset
                    );
                }
            }
        }
        None => println!("Model:       unknown to this build"),
    }
    Ok(())
}

fn cmd_eq(cli: &Cli, timeout: Duration) -> Result<()> {
    let mut dev = open_device(cli)?;
    let known = device_config::find_usb_device(dev.vendor_id(), dev.product_id());
    let band_count = known
        .as_ref()
        .and_then(|k| k.equalizer())
        .map(|e| e.frequency_number)
        .unwrap_or(8);

    println!(
        "Reading {} EQ band(s) from {} ...",
        band_count,
        dev.path().display()
    );

    dev.drain(Duration::from_millis(30));
    let requests = command::read_eq_bands(band_count);
    let replies = dev.request(&requests, timeout)?;

    if cli.hexdump {
        for (i, raw) in replies.iter().enumerate() {
            println!("  reply[{i}] {}", hex(raw));
        }
    }

    let mut bands = Vec::new();
    let mut preset = None;
    for raw in &replies {
        let Some(r) = command::Report::parse(raw) else {
            continue;
        };
        if r.opcode() == Opcode::Eq {
            if let Some((band, p)) = command::parse_eq_band(raw) {
                preset = Some(p);
                bands.push(band);
            } else if let Some(off) = command::parse_eq_offset(raw) {
                println!("EQ offset: {off:+} dB");
            }
        }
    }
    bands.sort_by_key(|b| b.index);

    if cli.json {
        let out = serde_json::json!({
            "device": dev.product_name(),
            "path": dev.path(),
            "replies": replies.len(),
            "presetIndex": preset,
            "bands": bands.iter().map(|b| serde_json::json!({
                "index": b.index,
                "frequency": b.frequency,
                "gain": b.gain,
                "q": b.q,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    if bands.is_empty() {
        bail!(
            "no EQ bands decoded from {} reply/replies — the device may need \
             a different opcode; re-run with --hexdump to inspect",
            replies.len()
        );
    }

    if let Some(p) = preset {
        let label = known
            .as_ref()
            .and_then(|k| k.equalizer())
            .and_then(|e| e.presets.iter().find(|x| x.preset_index == p).cloned())
            .map(|x| format!("{} ({})", x.display_name(), x.preset_key))
            .unwrap_or_else(|| "custom".to_string());
        println!("\nActive preset: [{p}] {label}");
    }
    println!(
        "\n{:>5}  {:>10}  {:>9}  {:>7}",
        "band", "freq (Hz)", "gain (dB)", "Q"
    );
    for b in &bands {
        println!(
            "{:>5}  {:>10}  {:>+9.2}  {:>7.2}",
            b.index, b.frequency, b.gain, b.q
        );
    }
    Ok(())
}

fn cmd_status(cli: &Cli, timeout: Duration) -> Result<()> {
    let mut dev = open_device(cli)?;
    let mut out = serde_json::Map::new();
    out.insert("device".into(), dev.product_name().into());
    out.insert("path".into(), dev.path().display().to_string().into());

    // Each of these is an independent read; failures are reported, not fatal,
    // because firmware revisions expose different subsets.
    dev.drain(Duration::from_millis(30));

    type Parser = fn(&[u8]) -> Option<serde_json::Value>;

    let single: [(&str, nicehck_protocol::Report, Parser); 2] = [
        (
            "device_volume",
            command::read_device_volume(),
            parse_device_volume_json,
        ),
        (
            "filter_type",
            command::read_filter_type(),
            parse_filter_json,
        ),
    ];

    for (label, req, parser) in single {
        match dev.request(&[req], timeout) {
            Ok(replies) => {
                let mut value = serde_json::Value::Null;
                for raw in &replies {
                    if cli.hexdump {
                        println!("  {label} raw: {}", hex(raw));
                    }
                    if let Some(v) = parser(raw) {
                        value = v;
                    }
                }
                out.insert(label.into(), value);
            }
            Err(e) => {
                out.insert(
                    label.into(),
                    serde_json::Value::String(format!("error: {e}")),
                );
            }
        }
    }

    // Version comes back as an ASCII string.
    match dev.request(&[command::Report::read(Opcode::Version)], timeout) {
        Ok(replies) => {
            let mut version = serde_json::Value::Null;
            for raw in &replies {
                if cli.hexdump {
                    println!("  version raw: {}", hex(raw));
                }
                if let Some(v) = command::parse_version(raw) {
                    if !v.is_empty() {
                        version = v.into();
                    }
                }
            }
            out.insert("firmware_version".into(), version);
        }
        Err(e) => {
            out.insert(
                "firmware_version".into(),
                serde_json::Value::String(format!("error: {e}")),
            );
        }
    }

    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::Value::Object(out))?
        );
        return Ok(());
    }

    println!("Device:   {}", dev.product_name());
    println!("Node:     {}", dev.path().display());
    for (k, v) in &out {
        if k == "device" || k == "path" {
            continue;
        }
        println!("{k:<18} {v}");
    }
    Ok(())
}

fn parse_device_volume_json(raw: &[u8]) -> Option<serde_json::Value> {
    command::parse_device_volume(raw).map(|v| serde_json::json!(v))
}

fn parse_filter_json(raw: &[u8]) -> Option<serde_json::Value> {
    command::parse_filter_type(raw).map(|v| serde_json::json!(v.label()))
}

/// Lowercase hex, for `--hexdump`.
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{b:02X}"));
    }
    s
}

/// Kept for symmetry with staged writes once those land.
#[allow(dead_code)]
const _: Duration = STAGED_DELAY;
