//! Linux transport: talk to the headset over `/dev/hidraw*`.
//!
//! The device exposes one vendor HID interface (bInterfaceClass 0x03) bound by
//! the stock `usbhid` driver, so no kernel module or libusb detach is required —
//! `hidraw` gives us the same 64-byte reports the Android app exchanges over
//! `UsbDeviceConnection.bulkTransfer`.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::command::{Report, REPORT_LEN};

/// Errors from the HID transport.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("no NICEHCK/YUANDAO headset found on hidraw (is it plugged in?)")]
    NotFound,
    #[error("cannot open {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// Opened by an unprivileged user against a root-only device node.
    ///
    /// Carries the ready-to-paste fix, because "Permission denied" on its own
    /// sends people looking for a bug in the tool rather than at udev.
    #[error(
        "cannot open {path}: permission denied\n\n\
         /dev/hidraw* is root-only until a udev rule grants access. Install the\n\
         bundled rule, then replug the headset:\n\
         \x20 sudo cp udev/70-nicehck.rules /etc/udev/rules.d/\n\
         \x20 sudo udevadm control --reload-rules && sudo udevadm trigger\n\n\
         On NixOS, add the flake's udevRules package to services.udev.packages\n\
         instead of copying the file by hand (see README)."
    )]
    PermissionDenied { path: PathBuf },
    #[error("HID write failed: {0}")]
    Write(#[source] std::io::Error),
    #[error("HID read failed: {0}")]
    Read(#[source] std::io::Error),
    #[error("timed out after {0:?} waiting for a {1} reply")]
    Timeout(Duration, &'static str),
}

/// A control channel bound to one hidraw node.
pub struct HidDevice {
    file: File,
    path: PathBuf,
    vendor_id: u16,
    product_id: u16,
    product_name: String,
    readable: bool,
}

/// Metadata for a discovered hidraw node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidRawInfo {
    pub path: PathBuf,
    pub vendor_id: u16,
    pub product_id: u16,
    pub product_name: String,
}

/// Parse a `HID_ID=0003:00003302:0000C200` uevent line into a VID/PID pair.
fn parse_hid_id(line: &str) -> Option<(u16, u16)> {
    let value = line.strip_prefix("HID_ID=")?;
    let mut parts = value.split(':');
    let _bus = parts.next()?;
    let vid = u16::from_str_radix(parts.next()?.trim_start_matches("0000"), 16).ok()?;
    let pid = u16::from_str_radix(parts.next()?.trim_start_matches("0000"), 16).ok()?;
    Some((vid, pid))
}

/// Enumerate every hidraw node belonging to the given vendor.
pub fn enumerate(vendor_id: u16) -> Vec<HidRawInfo> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/hidraw") else {
        return found;
    };
    for entry in entries.flatten() {
        let node = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(uevent) = fs::read_to_string(node.join("device/uevent")) else {
            continue;
        };
        let mut vid = None;
        let mut pid = None;
        let mut product = String::new();
        for line in uevent.lines() {
            if let Some((v, p)) = parse_hid_id(line) {
                vid = Some(v);
                pid = Some(p);
            } else if let Some(rest) = line.strip_prefix("HID_NAME=") {
                product = rest.to_string();
            }
        }
        let (Some(vid), Some(pid)) = (vid, pid) else {
            continue;
        };
        if vid != vendor_id {
            continue;
        }
        found.push(HidRawInfo {
            path: PathBuf::from(format!("/dev/{name}")),
            vendor_id: vid,
            product_id: pid,
            product_name: product,
        });
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

impl HidDevice {
    /// Open the first attached headset, matching on the shared NICEHCK vendor ID.
    pub fn open_first() -> Result<Self, TransportError> {
        let candidates = enumerate(crate::device_config::VENDOR_ID);
        let info = candidates
            .into_iter()
            .next()
            .ok_or(TransportError::NotFound)?;
        Self::open_path(&info)
    }

    /// Open a specific hidraw node.
    pub fn open_path(info: &HidRawInfo) -> Result<Self, TransportError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc_o_noblock())
            .open(&info.path)
            .map_err(|source| {
                if source.kind() == ErrorKind::PermissionDenied {
                    TransportError::PermissionDenied {
                        path: info.path.clone(),
                    }
                } else {
                    TransportError::Open {
                        path: info.path.clone(),
                        source,
                    }
                }
            })?;
        Ok(Self {
            file,
            path: info.path.clone(),
            vendor_id: info.vendor_id,
            product_id: info.product_id,
            product_name: info.product_name.clone(),
            readable: true,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn vendor_id(&self) -> u16 {
        self.vendor_id
    }

    pub fn product_id(&self) -> u16 {
        self.product_id
    }

    pub fn product_name(&self) -> &str {
        &self.product_name
    }

    /// Write one 64-byte report (report ID included as byte 0).
    pub fn write_report(&mut self, report: &Report) -> Result<(), TransportError> {
        let written = self
            .file
            .write(report.as_slice())
            .map_err(TransportError::Write)?;
        if written != REPORT_LEN {
            return Err(TransportError::Write(std::io::Error::new(
                ErrorKind::WriteZero,
                format!("short HID write: {written}/{REPORT_LEN} bytes"),
            )));
        }
        Ok(())
    }

    /// Read one report, waiting up to `timeout` for data to arrive.
    pub fn read_report(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; REPORT_LEN];
        loop {
            match self.file.read(&mut buf) {
                Ok(n) if n > 0 => return Ok(buf[..n].to_vec()),
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(TransportError::Timeout(timeout, "report"));
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(TransportError::Read(e)),
            }
        }
    }

    /// Send one request and collect replies until `timeout` elapses.
    ///
    /// The firmware may emit several reports for a single request (one per EQ
    /// band, for example), so this drains the endpoint rather than returning the
    /// first packet.
    pub fn request(
        &mut self,
        reports: &[Report],
        timeout: Duration,
    ) -> Result<Vec<Vec<u8>>, TransportError> {
        for r in reports {
            self.write_report(r)?;
            // The app sleeps 20 ms after each staged command before reading.
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut out = Vec::new();
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.read_report(remaining) {
                Ok(buf) => out.push(buf),
                Err(TransportError::Timeout(..)) => break,
                Err(e) => return Err(e),
            }
        }
        if out.is_empty() {
            return Err(TransportError::Timeout(timeout, "command"));
        }
        Ok(out)
    }

    /// Flush any stale reports left in the endpoint buffer.
    pub fn drain(&mut self, budget: Duration) {
        let deadline = Instant::now() + budget;
        let mut scratch = [0u8; REPORT_LEN];
        while Instant::now() < deadline {
            match self.file.read(&mut scratch) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }

    /// Whether reads are expected to succeed (always true once opened).
    pub fn is_readable(&self) -> bool {
        self.readable
    }
}

/// `O_NONBLOCK` so reads can be polled with a deadline instead of blocking
/// forever when the headset stays silent.
const fn libc_o_noblock() -> i32 {
    // Linux value for O_NONBLOCK; avoids a libc dependency for one constant.
    0o4000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uevent_hid_id() {
        assert_eq!(
            parse_hid_id("HID_ID=0003:00003302:0000C200"),
            Some((0x3302, 0xC200))
        );
        assert_eq!(
            parse_hid_id("HID_ID=0018:00002808:00000101"),
            Some((0x2808, 0x0101))
        );
        assert_eq!(parse_hid_id("HID_NAME=nope"), None);
        assert_eq!(parse_hid_id("HID_ID=malformed"), None);
    }

    #[test]
    fn enumerate_finds_anniversary_device_when_present() {
        // Runs against real hardware; tolerant of the device being absent so the
        // suite stays green on CI and on unplugged machines.
        let found = enumerate(0x3302);
        for info in &found {
            assert_eq!(info.vendor_id, 0x3302);
            assert_eq!(info.path.parent(), Some(Path::new("/dev")));
            let name = info.path.file_name().unwrap().to_string_lossy();
            assert!(name.starts_with("hidraw"), "unexpected node name {name}");
            assert!(!info.product_name.is_empty());
        }
        // Every node must be sorted deterministically for stable UI listings.
        let mut sorted = found.clone();
        sorted.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(found, sorted);
    }

    #[test]
    fn non_vendor_nodes_are_filtered_out() {
        // The keyboard's hidraw nodes must not be reported as headsets.
        for info in enumerate(0x3302) {
            assert_eq!(info.vendor_id, 0x3302);
        }
        assert!(
            enumerate(0xFFFF).is_empty(),
            "bogus VID should match nothing"
        );
    }

    #[test]
    fn permission_denied_maps_to_actionable_error() {
        // Opening a root-only node must yield the udev fix, not a bare errno.
        // /proc/1/mem is root-only for any unprivileged test runner; skip if the
        // suite itself runs as root (where the open would instead fail with EIO).
        if unsafe { is_root() } {
            return;
        }
        let info = HidRawInfo {
            path: PathBuf::from("/proc/1/mem"),
            vendor_id: 0x3302,
            product_id: 0xC200,
            product_name: "test".into(),
        };
        match HidDevice::open_path(&info) {
            Err(TransportError::PermissionDenied { path }) => {
                assert_eq!(path, PathBuf::from("/proc/1/mem"));
                let msg = TransportError::PermissionDenied { path }.to_string();
                assert!(msg.contains("70-nicehck.rules"), "missing udev hint: {msg}");
                assert!(
                    msg.contains("udevadm trigger"),
                    "missing trigger hint: {msg}"
                );
            }
            Err(other) => panic!("expected PermissionDenied, got {other:?}"),
            Ok(_) => panic!("expected PermissionDenied, but the node opened"),
        }
    }

    /// `geteuid() == 0`, without pulling in the libc crate.
    unsafe fn is_root() -> bool {
        extern "C" {
            fn geteuid() -> u32;
        }
        geteuid() == 0
    }
}
