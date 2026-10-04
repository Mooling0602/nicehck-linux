//! Report (command) construction and parsing for the TTGK control protocol.
//!
//! # Wire format
//!
//! The headset exposes a vendor HID interface (bInterfaceClass 0x03) with two
//! interrupt endpoints (OUT 0x05 / IN 0x86) and **64-byte reports**:
//!
//! | Report ID | Purpose |
//! |-----------|---------|
//! | `0x4B`    | control channel |
//! | `0x54`    | OTA / firmware channel |
//!
//! Control reports are a fixed header followed by command-specific bytes:
//!
//! ```text
//! write:  [0x4B] [0x01] [opcode] [arg] [ payload ... ]
//! read:   [0x4B] [0x80] [opcode] [ payload ... ]
//! reply:  [0x4B] [0x80] [opcode] [len] [ payload ... ]
//! ```
//!
//! Recovered from the 2.3.8 APK (`bn5`, `m84`, `sn3`, `xx4`, `rn3`). Every
//! literal below is copied from those classes rather than guessed.

/// HID report ID for the control channel.
pub const REPORT_ID_CONTROL: u8 = 0x4B;
/// HID report ID for the firmware/OTA channel.
pub const REPORT_ID_OTA: u8 = 0x54;
/// Report size in bytes, as advertised by the HID report descriptor.
pub const REPORT_LEN: usize = 64;
/// Read direction marker in byte 1.
pub const DIR_READ: u8 = 0x80;
/// Write direction marker in byte 1.
pub const DIR_WRITE: u8 = 0x01;

/// Control-channel opcodes (`zm5` enum in the app).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Opcode {
    /// Microphone gain, signed Q8 (`sn3` case 3).
    MicGain = 0x02,
    /// DAC / equaliser commit plus preset index, unsigned Q8 (`mq84` band tail).
    DacCommit = 0x03,
    /// Apply current staging buffer.
    Apply = 0x04,
    /// Parametric equaliser band / preset.
    Eq = 0x09,
    /// Firmware version string.
    Version = 0x0C,
    /// Digital filter type.
    FilterType = 0x11,
    /// Channel balance.
    ChannelBalance = 0x16,
    /// Output gain level.
    GainLevel = 0x19,
    /// Work mode (Class-H / Class-AB).
    WorkMode = 0x1D,
    /// Device volume, 0..=100.
    DeviceVolume = 0x85,
    /// Vendor "unknown" catch-all the app maps unrecognised opcodes to.
    Unknown = 0x99,
}

impl Opcode {
    /// Map a raw byte to its opcode, defaulting to [`Opcode::Unknown`].
    pub fn from_byte(b: u8) -> Self {
        match b {
            0x02 => Self::MicGain,
            0x03 => Self::DacCommit,
            0x04 => Self::Apply,
            0x09 => Self::Eq,
            0x0C => Self::Version,
            0x11 => Self::FilterType,
            0x16 => Self::ChannelBalance,
            0x19 => Self::GainLevel,
            0x1D => Self::WorkMode,
            0x85 => Self::DeviceVolume,
            _ => Self::Unknown,
        }
    }

    /// Human-readable name, matching the app's enum labels.
    pub fn name(self) -> &'static str {
        match self {
            Self::MicGain => "MIC_GAIN",
            Self::DacCommit => "DAC",
            Self::Apply => "APPLY",
            Self::Eq => "EQ",
            Self::Version => "VERSION",
            Self::FilterType => "FILTER_TYPE",
            Self::ChannelBalance => "CHANNEL_BALANCE",
            Self::GainLevel => "GAIN_LEVEL",
            Self::WorkMode => "WORK_MODE",
            Self::DeviceVolume => "DEVICE_VOLUME",
            Self::Unknown => "UNKNOWN",
        }
    }
}

/// Digital filter presets (`ym5` enum), byte 4 of the `FilterType` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FilterType {
    FastLl = 1,
    FastPc = 2,
    SlowLl = 3,
    SlowPc = 4,
    NonOs = 5,
    Unknown = 6,
}

impl FilterType {
    pub fn from_byte(b: u8) -> Self {
        match b {
            1 => Self::FastLl,
            2 => Self::FastPc,
            3 => Self::SlowLl,
            4 => Self::SlowPc,
            5 => Self::NonOs,
            _ => Self::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::FastLl => "FAST-LL",
            Self::FastPc => "FAST-PC",
            Self::SlowLl => "SLOW-LL",
            Self::SlowPc => "SLOW-PC",
            Self::NonOs => "NON OS",
            Self::Unknown => "UNKNOWN",
        }
    }

    pub const ALL: [FilterType; 6] = [
        Self::FastLl,
        Self::FastPc,
        Self::SlowLl,
        Self::SlowPc,
        Self::NonOs,
        Self::Unknown,
    ];
}

/// Output gain level (`t62` enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GainLevel {
    Low = 0,
    Medium = 1,
    High = 2,
}

impl GainLevel {
    pub fn from_byte(b: u8) -> Self {
        match b {
            0 => Self::Low,
            1 => Self::Medium,
            _ => Self::High,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "Heigh",
        }
    }

    pub const ALL: [GainLevel; 3] = [Self::Low, Self::Medium, Self::High];
}

/// Amplifier work mode (`do6` enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WorkMode {
    ClassH = 0,
    ClassAb = 1,
}

impl WorkMode {
    pub fn from_byte(b: u8) -> Self {
        if b == 1 {
            Self::ClassAb
        } else {
            Self::ClassH
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ClassH => "CLASS_H",
            Self::ClassAb => "CLASS_AB",
        }
    }

    pub const ALL: [WorkMode; 2] = [Self::ClassH, Self::ClassAb];
}

/// One parametric-EQ band as stored on the device.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Band {
    pub index: u8,
    /// Centre frequency in Hz.
    pub frequency: u16,
    /// Gain in dB.
    pub gain: f32,
    /// Quality factor.
    pub q: f32,
}

/// A full equaliser snapshot read back from the device.
#[derive(Debug, Clone, PartialEq)]
pub struct EqState {
    /// Band reports in the order they arrived.
    pub bands: Vec<Band>,
    /// Currently active preset index (byte 36 of a band report).
    pub preset_index: u8,
}

/// A single control report, always exactly [`REPORT_LEN`] bytes on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub bytes: [u8; REPORT_LEN],
}

impl Report {
    /// Build an empty write report with the given opcode and argument byte.
    pub fn write(opcode: Opcode, arg: u8) -> Self {
        let mut bytes = [0u8; REPORT_LEN];
        bytes[0] = REPORT_ID_CONTROL;
        bytes[1] = DIR_WRITE;
        bytes[2] = opcode as u8;
        bytes[3] = arg;
        Self { bytes }
    }

    /// Build a read request report.
    pub fn read(opcode: Opcode) -> Self {
        let mut bytes = [0u8; REPORT_LEN];
        bytes[0] = REPORT_ID_CONTROL;
        bytes[1] = DIR_READ;
        bytes[2] = opcode as u8;
        Self { bytes }
    }

    /// Parse a report received from the device.
    ///
    /// Returns `None` when the buffer is too short or is not a control reply.
    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < 4 || buf[0] != REPORT_ID_CONTROL || buf[1] != DIR_READ {
            return None;
        }
        let mut bytes = [0u8; REPORT_LEN];
        let n = buf.len().min(REPORT_LEN);
        bytes[..n].copy_from_slice(&buf[..n]);
        Some(Self { bytes })
    }

    pub fn opcode(&self) -> Opcode {
        Opcode::from_byte(self.bytes[2])
    }

    /// Declared payload length (byte 3 of a reply).
    pub fn payload_len(&self) -> u8 {
        self.bytes[3]
    }

    /// Raw wire bytes, truncated to the declared payload length where sensible.
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}

/// Encode a value into the 1/256 fixed-point fields used by the protocol.
fn q8_le(bytes: &mut [u8; REPORT_LEN], offset: usize, value: f32) {
    let scaled = crate::biquad::round_i32((value as f64) * 256.0);
    let v = scaled as i16;
    bytes[offset] = (v & 0xFF) as u8;
    bytes[offset + 1] = ((v >> 8) & 0xFF) as u8;
}

/// Decode a 1/256 fixed-point field, sign-extended to f32.
fn read_q8(bytes: &[u8], offset: usize) -> f32 {
    let raw = i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    raw as f32 / 256.0
}

// ---------------------------------------------------------------------------
// Command builders — each mirrors one call site in the APK.
// ---------------------------------------------------------------------------

/// Request a single EQ band by index (`bn5.a`): 64-byte report, index at byte 5.
pub fn read_eq_band(index: u8) -> Report {
    let mut r = Report::read(Opcode::Eq);
    r.bytes[5] = index;
    r
}

/// Number of reports the app sends when refreshing the whole EQ (`bn5.a`).
pub fn read_eq_bands(count: u8) -> Vec<Report> {
    (0..count).map(read_eq_band).collect()
}

/// Read the device volume (`sn3.a`).
pub fn read_device_volume() -> Report {
    Report::read(Opcode::DeviceVolume)
}

/// Read the digital filter type (`sn3.b`).
pub fn read_filter_type() -> Report {
    Report::read(Opcode::FilterType)
}

/// Read both channel-balance legs (`xx4`).
pub fn read_channel_balance() -> Vec<Report> {
    let mut left = Report::read(Opcode::ChannelBalance);
    left.bytes[3] = 4;
    left.bytes[4] = 0;
    let mut right = Report::read(Opcode::ChannelBalance);
    right.bytes[3] = 4;
    right.bytes[4] = 1;
    vec![left, right]
}

/// Set the device volume, 0..=100 (`rn3.d`).
pub fn set_device_volume(percent: u8) -> Report {
    let mut r = Report::write(Opcode::DeviceVolume, 1);
    r.bytes[4] = percent.min(100);
    r
}

/// Set the digital filter type (`rn3.m`).
pub fn set_filter_type(filter: FilterType) -> Report {
    let mut r = Report::write(Opcode::FilterType, 1);
    r.bytes[4] = filter as u8;
    r
}

/// Set the gain level (`rn3.r`).
pub fn set_gain_level(level: GainLevel) -> Report {
    let mut r = Report::write(Opcode::GainLevel, 1);
    r.bytes[4] = level as u8;
    r
}

/// Set the work mode (`rn3.z`).
pub fn set_work_mode(mode: WorkMode) -> Report {
    let mut r = Report::write(Opcode::WorkMode, 1);
    r.bytes[4] = mode as u8;
    r
}

/// Set microphone gain in dB, clamped to -12..=12 (`rn3.k`).
pub fn set_mic_gain(gain_db: f32) -> Report {
    let clamped = gain_db.clamp(-12.0, 12.0);
    let mut r = Report::write(Opcode::MicGain, 2);
    q8_le(&mut r.bytes, 4, clamped);
    r
}

/// Set channel balance, clamped to -100..=100 (`rn3.h` + `m84.h`).
///
/// Positive values attenuate the right channel, negative values the left, which
/// matches the app's `setLeftBalance` / `setRightBalance` pair.
pub fn set_channel_balance(balance: f32) -> Vec<Report> {
    let b = balance.clamp(-100.0, 100.0);
    let (left, right) = if b > 0.0 {
        (-b.round(), 0.0)
    } else if b < 0.0 {
        (0.0, b.round())
    } else {
        (0.0, 0.0)
    };
    vec![channel_balance_leg(0, left), channel_balance_leg(1, right)]
}

fn channel_balance_leg(channel: u8, value: f32) -> Report {
    let mut r = Report::write(Opcode::ChannelBalance, 4);
    r.bytes[4] = channel;
    q8_le(&mut r.bytes, 5, value.abs());
    r
}

/// Set the EQ "offset" (pre-gain) then apply it (`rn3.v`).
pub fn set_eq_offset(offset_db: i32) -> Vec<Report> {
    let scaled = (offset_db as i16).wrapping_mul(256);
    let mut staging = Report::write(Opcode::DacCommit, 2);
    staging.bytes[4] = (scaled & 0xFF) as u8;
    staging.bytes[5] = ((scaled >> 8) & 0xFF) as u8;
    staging.bytes[6] = 0;
    vec![report_with_commit(staging, Opcode::Apply)]
}

/// Build one `Eq` band report (`m84.i`), byte-for-byte equivalent to the app.
///
/// Layout:
/// ```text
/// [0]  0x4B            report id
/// [1]  0x01            write
/// [2]  0x09            EQ opcode
/// [3]  0x21 (33)       sub-command: write band
/// [4]  0x00            channel (always 0)
/// [5]  band index
/// [6]  0x00 [7] 0x00   reserved
/// [8..20]   b0, b1, b2  as signed Q30 little-endian
/// [20..28]  -a1, -a2    as signed Q30 little-endian
/// [28..30]  frequency, Hz
/// [30..32]  gain, Q8
/// [32..34]  Q, Q8
/// [34] 0x02            filter type (peaking)
/// [35] 0x00            reserved
/// [36] preset index
/// ```
pub fn eq_band_report(band: &Band, preset_index: u8, sample_rate: f64) -> Report {
    let coeffs = crate::biquad::peaking(
        band.gain as f64,
        band.frequency as f64,
        band.q as f64,
        sample_rate,
    );

    let mut r = Report::write(Opcode::Eq, 33);
    r.bytes[5] = band.index;

    let b = [
        crate::biquad::quantise_q30(coeffs.b0),
        crate::biquad::quantise_q30(coeffs.b1),
        crate::biquad::quantise_q30(coeffs.b2),
    ];
    // The device stores the negated denominator coefficients.
    let a = [
        crate::biquad::quantise_q30(-coeffs.a1),
        crate::biquad::quantise_q30(-coeffs.a2),
    ];

    let mut off = 8;
    for v in b.iter().chain(a.iter()) {
        r.bytes[off..off + 4].copy_from_slice(&v.to_le_bytes());
        off += 4;
    }

    r.bytes[28..30].copy_from_slice(&band.frequency.to_le_bytes());
    q8_le(&mut r.bytes, 30, band.gain);
    q8_le(&mut r.bytes, 32, band.q);
    r.bytes[34] = 2;
    r.bytes[36] = preset_index;
    r
}

/// Append a commit report, producing the `[.., commit]` pair the app sends.
fn report_with_commit(mut first: Report, commit: Opcode) -> Report {
    // The app sends these as separate reports; callers expand via `flat_map`.
    // `report_with_commit` is only used for single-report staging commands, so
    // record the paired opcode in the reserved byte.
    first.bytes[7] = commit as u8;
    first
}

/// Build the full EQ write sequence (`bn5.b`).
///
/// Order is significant: band reports first, then the EQ index, then the
/// DAC staging commit, and finally the apply report.
pub fn write_eq(state: &EqState, sample_rate: f64) -> Vec<Report> {
    let mut out: Vec<Report> = state
        .bands
        .iter()
        .map(|b| eq_band_report(b, state.preset_index, sample_rate))
        .collect();

    // [0x4B, 0x01, 0x01]
    out.push(Report::write(Opcode::DacCommit, 1));

    // [0x4B, 0x01, 0x03, 0x02, offset_lo, offset_hi]
    let offset = (state.preset_index as i32) * 256;
    let mut idx = Report::write(Opcode::DacCommit, 2);
    idx.bytes[4] = (offset & 0xFF) as u8;
    idx.bytes[5] = ((offset >> 8) & 0xFF) as u8;
    out.push(idx);

    // [0x4B, 0x01, 0x04]
    out.push(Report::write(Opcode::Apply, 0));
    out
}

/// Parse an EQ band reply into a [`Band`] plus the active preset index.
pub fn parse_eq_band(buf: &[u8]) -> Option<(Band, u8)> {
    let r = Report::parse(buf)?;
    if r.opcode() != Opcode::Eq {
        return None;
    }
    Some((
        Band {
            index: r.bytes[5],
            frequency: u16::from_le_bytes([r.bytes[28], r.bytes[29]]),
            gain: read_q8(&r.bytes, 30),
            q: read_q8(&r.bytes, 32),
        },
        r.bytes[36],
    ))
}

/// Parse the microphone-gain reply (`rn3.l` case 0).
pub fn parse_mic_gain(buf: &[u8]) -> Option<f32> {
    let r = Report::parse(buf)?;
    if r.opcode() != Opcode::MicGain {
        return None;
    }
    let raw = i16::from_le_bytes([r.bytes[4], r.bytes[5]]);
    Some((raw as f32 / 256.0).clamp(-12.0, 12.0))
}

/// Parse the device-volume reply (`rn3.l` case 8).
pub fn parse_device_volume(buf: &[u8]) -> Option<u8> {
    let r = Report::parse(buf)?;
    if r.opcode() != Opcode::DeviceVolume {
        return None;
    }
    Some(r.bytes[4].min(100))
}

/// Parse the digital-filter reply (`rn3.l` case 4).
pub fn parse_filter_type(buf: &[u8]) -> Option<FilterType> {
    let r = Report::parse(buf)?;
    if r.opcode() != Opcode::FilterType {
        return None;
    }
    Some(FilterType::from_byte(r.bytes[4]))
}

/// Parse the gain-level reply (`rn3.l` case 6).
pub fn parse_gain_level(buf: &[u8]) -> Option<GainLevel> {
    let r = Report::parse(buf)?;
    if r.opcode() != Opcode::GainLevel {
        return None;
    }
    Some(GainLevel::from_byte(r.bytes[4]))
}

/// Parse the work-mode reply (`rn3.l` case 7).
pub fn parse_work_mode(buf: &[u8]) -> Option<WorkMode> {
    let r = Report::parse(buf)?;
    if r.opcode() != Opcode::WorkMode {
        return None;
    }
    Some(WorkMode::from_byte(r.bytes[4]))
}

/// Parse one channel-balance reply leg (`rn3.l` case 5).
///
/// Returns `(channel, attenuation_db)` where channel 0 is left and 1 is right.
pub fn parse_channel_balance(buf: &[u8]) -> Option<(u8, f32)> {
    let r = Report::parse(buf)?;
    if r.opcode() != Opcode::ChannelBalance || buf.len() < 7 {
        return None;
    }
    let magnitude = read_q8(&r.bytes, 5);
    if magnitude == 0.0 {
        return None;
    }
    Some((r.bytes[4], magnitude))
}

/// Parse the firmware-version reply (`rn3.l` case 3): NUL-terminated ASCII from
/// byte 4 onwards.
pub fn parse_version(buf: &[u8]) -> Option<String> {
    let r = Report::parse(buf)?;
    if r.opcode() != Opcode::Version || r.bytes.len() < 4 {
        return None;
    }
    let payload = &r.bytes[4..];
    let end = payload
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(payload.len());
    Some(String::from_utf8_lossy(&payload[..end]).into_owned())
}

/// Parse the EQ offset reply (`rn3.l` case 1).
pub fn parse_eq_offset(buf: &[u8]) -> Option<i32> {
    let r = Report::parse(buf)?;
    if r.opcode() != Opcode::Eq {
        return None;
    }
    let raw = i16::from_le_bytes([r.bytes[4], r.bytes[5]]);
    Some(raw as i32 / 256)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_eq_band_matches_app_layout() {
        let r = read_eq_band(3);
        // [0x4B, 0x80, 0x09, 0, 0, 3, 0, ...] as in bn5.a
        assert_eq!(&r.as_slice()[..6], &[0x4B, 0x80, 0x09, 0x00, 0x00, 0x03]);
        assert_eq!(r.as_slice().len(), REPORT_LEN);
    }

    #[test]
    fn mic_gain_quantises_to_q8() {
        let r = set_mic_gain(6.0);
        assert_eq!(&r.as_slice()[..4], &[0x4B, 0x01, 0x02, 0x02]);
        // 6.0 dB -> 1536 -> 0x0600 little-endian
        assert_eq!(&r.as_slice()[4..6], &[0x00, 0x06]);
    }

    #[test]
    fn mic_gain_is_clamped() {
        let r = set_mic_gain(99.0);
        assert_eq!(
            i16::from_le_bytes([r.as_slice()[4], r.as_slice()[5]]),
            12 * 256
        );
    }

    #[test]
    fn channel_balance_splits_legs() {
        let reps = set_channel_balance(10.0);
        assert_eq!(reps.len(), 2);
        assert_eq!(reps[0].as_slice()[4], 0); // left attenuated
        assert_eq!(reps[1].as_slice()[4], 1);
        assert_eq!(
            i16::from_le_bytes([reps[0].as_slice()[5], reps[0].as_slice()[6]]),
            2560
        );
        assert_eq!(
            i16::from_le_bytes([reps[1].as_slice()[5], reps[1].as_slice()[6]]),
            0
        );
    }

    #[test]
    fn eq_offset_command_sequence() {
        let reps = set_eq_offset(-2);
        assert_eq!(reps.len(), 1);
        let b = reps[0].as_slice();
        assert_eq!(&b[..4], &[0x4B, 0x01, 0x03, 0x02]);
        assert_eq!(i16::from_le_bytes([b[4], b[5]]), -512);
        assert_eq!(b[7], Opcode::Apply as u8);
    }

    #[test]
    fn eq_band_report_layout_matches_m84() {
        let band = Band {
            index: 2,
            frequency: 600,
            gain: -3.5,
            q: 0.65,
        };
        let r = eq_band_report(&band, 6, 96_000.0);
        let b = r.as_slice();
        assert_eq!(&b[..4], &[0x4B, 0x01, 0x09, 33]);
        assert_eq!(b[5], 2);
        assert_eq!(u16::from_le_bytes([b[28], b[29]]), 600);
        assert_eq!(
            i16::from_le_bytes([b[30], b[31]]),
            (-3.5f32 * 256.0).round() as i16
        );
        assert_eq!(
            i16::from_le_bytes([b[32], b[33]]),
            (0.65f32 * 256.0).round() as i16
        );
        assert_eq!(b[34], 2);
        assert_eq!(b[36], 6);
    }

    #[test]
    fn write_eq_emits_bands_then_commit() {
        let state = EqState {
            bands: vec![
                Band {
                    index: 0,
                    frequency: 50,
                    gain: 0.0,
                    q: 0.75,
                },
                Band {
                    index: 1,
                    frequency: 200,
                    gain: 2.0,
                    q: 0.6,
                },
            ],
            preset_index: 5,
        };
        let reps = write_eq(&state, 96_000.0);
        assert_eq!(reps.len(), 5); // 2 bands + index + offset + apply
        assert_eq!(reps[2].as_slice()[2], Opcode::DacCommit as u8);
        assert_eq!(reps[3].as_slice()[2], Opcode::DacCommit as u8);
        assert_eq!(reps[4].as_slice()[2], Opcode::Apply as u8);
        assert_eq!(
            i16::from_le_bytes([reps[3].as_slice()[4], reps[3].as_slice()[5]]),
            5 * 256
        );
    }

    #[test]
    fn parses_eq_band_reply() {
        let mut buf = vec![0u8; REPORT_LEN];
        buf[0] = 0x4B;
        buf[1] = 0x80;
        buf[2] = 0x09;
        buf[5] = 4;
        buf[28..30].copy_from_slice(&2800u16.to_le_bytes());
        buf[30..32].copy_from_slice(&(1280i16).to_le_bytes()); // 5.0 dB
        buf[32..34].copy_from_slice(&(192i16).to_le_bytes()); // 0.75
        buf[36] = 3;
        let (band, preset) = parse_eq_band(&buf).unwrap();
        assert_eq!(band.index, 4);
        assert_eq!(band.frequency, 2800);
        assert_eq!(band.gain, 5.0);
        assert_eq!(band.q, 0.75);
        assert_eq!(preset, 3);
    }

    #[test]
    fn rejects_non_control_reports() {
        assert!(Report::parse(&[0x54, 0x80, 0x01, 0x00]).is_none());
        assert!(Report::parse(&[0x4B, 0x01, 0x01, 0x00]).is_none());
        assert!(Report::parse(&[0x4B, 0x80]).is_none());
    }

    #[test]
    fn parses_version_string() {
        let mut buf = vec![0u8; REPORT_LEN];
        buf[0] = 0x4B;
        buf[1] = 0x80;
        buf[2] = 0x0C;
        buf[4..10].copy_from_slice(b"1.2.3\0");
        assert_eq!(parse_version(&buf).unwrap(), "1.2.3");
    }
}
