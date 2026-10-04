//! Reverse-engineered protocol for NICEHCK / YUANDAO USB-C DSP earphones.
//!
//! This crate reimplements, for Linux, what the Android app `com.yuandao.nicehck`
//! (v2.3.8) does over USB HID. It is the result of statically analysing the APK,
//! not of any vendor documentation — see the repository `docs/PROTOCOL.md` for the
//! full derivation and the evidence behind each field.
//!
//! # Layout
//!
//! * [`crc`] – standard CRC-32, used by the frame format on the OTA channel
//! * [`biquad`] – peaking-EQ coefficient maths and Q30 quantisation
//! * [`command`] – report encoding/decoding and every recovered opcode
//! * [`device_config`] – the vendor's own device catalogue, embedded verbatim
//! * [`transport`] – `/dev/hidraw` I/O
//!
//! # Example
//!
//! ```no_run
//! use nicehck_protocol::{command, transport::HidDevice};
//! use std::time::Duration;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut dev = HidDevice::open_first()?;
//! println!("connected to {}", dev.product_name());
//!
//! // Ask for every EQ band and print what comes back.
//! let replies = dev.request(&command::read_eq_bands(8), Duration::from_millis(1200))?;
//! for raw in replies {
//!     if let Some((band, preset)) = command::parse_eq_band(&raw) {
//!         println!("band {} @ {} Hz gain {} dB (preset {preset})",
//!                  band.index, band.frequency, band.gain);
//!     }
//! }
//! # Ok(())
//! # }
//! ```

pub mod biquad;
pub mod command;
pub mod crc;
pub mod device_config;
pub mod frame;
pub mod transport;

pub use command::{Band, EqState, Opcode, Report};
pub use device_config::{DeviceConfigFile, EqualizerCapability, Preset};
pub use transport::{HidDevice, HidRawInfo, TransportError};
