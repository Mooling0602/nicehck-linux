//! Frame codec for the firmware/OTA channel.
//!
//! Recovered from `Lsc6;`, `Lrc6;` and `Lqc6;` in the 2.3.8 APK. This channel uses
//! a 14-byte header followed by the body and a 4-byte CRC, transported inside
//! 64-byte reports carrying report ID [`crate::command::REPORT_ID_OTA`] (0x54).
//!
//! ```text
//! offset  size  field
//!      0     4  magic 'B' 'U' 'X' 'X' (little-endian 0x58585542)
//!      4     2  command   (u16 LE)
//!      6     1  flags / channel
//!      7     1  sequence number
//!      8     2  body length (u16 LE)
//!     10     4  header CRC-32 (over bytes 0..10 of this header)
//!     14     n  body
//!   14+n     4  body CRC-32 (over the body; omitted when the body is empty)
//! ```
//!
//! The magic is stored as the integer `1482184002` == `0x58585542`; written
//! little-endian that spells `B U X X` on the wire, and the vendor parser
//! resynchronises by hunting for exactly those four bytes.

use crate::crc::crc32;

/// Header magic as stored in the app (`1482184002` == 0x58585542); on the wire it
/// appears as the ASCII bytes `BUXX`.
pub const MAGIC: u32 = 0x5858_5542;
/// Serialised header size.
pub const HEADER_LEN: usize = 14;
/// Trailing CRC size.
pub const CRC_LEN: usize = 4;
/// Sync byte 0 — 'B'.
const SYNC_B: u8 = 66;
/// Sync byte 1 — 'U'.
const SYNC_U: u8 = 85;
/// Sync byte 2 and 3 — 'X'.
const SYNC_X: u8 = 88;

/// OTA commands recovered from `qj4.f(short)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum OtaCommand {
    /// `HANDSHAKE(0x10)`
    Handshake = 0x10,
    /// `SET_FLASH_WRITE_AREA(0x20)`
    SetFlashWriteArea = 0x20,
    /// `WRITE_FLASH_DATA(0x21)`
    WriteFlashData = 0x21,
    /// `END_AND_REBOOT(0x24)`
    EndAndReboot = 0x24,
}

impl OtaCommand {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0x10 => Some(Self::Handshake),
            0x20 => Some(Self::SetFlashWriteArea),
            0x21 => Some(Self::WriteFlashData),
            0x24 => Some(Self::EndAndReboot),
            _ => None,
        }
    }

    /// The app's debug label, e.g. `HANDSHAKE(0x10)`.
    pub fn label(self) -> String {
        let name = match self {
            Self::Handshake => "HANDSHAKE",
            Self::SetFlashWriteArea => "SET_FLASH_WRITE_AREA",
            Self::WriteFlashData => "WRITE_FLASH_DATA",
            Self::EndAndReboot => "END_AND_REBOOT",
        };
        format!("{name}(0x{:X})", self as u16)
    }
}

/// Header errors.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("bad magic: expected 0x{MAGIC:08X} (BUXX), got 0x{0:08X}")]
    BadMagic(u32),
    #[error("header CRC mismatch: computed 0x{computed:08X}, field 0x{found:08X}")]
    HeaderCrc { computed: u32, found: u32 },
    #[error("body CRC mismatch: computed 0x{computed:08X}, field 0x{found:08X}")]
    BodyCrc { computed: u32, found: u32 },
    #[error("buffer too short: need {need} bytes, have {have}")]
    Truncated { need: usize, have: usize },
}

/// A decoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub command: u16,
    pub flags: u8,
    pub sequence: u8,
    pub body: Vec<u8>,
}

impl Frame {
    pub fn new(command: u16, flags: u8, sequence: u8, body: Vec<u8>) -> Self {
        Self {
            command,
            flags,
            sequence,
            body,
        }
    }

    /// Serialise the 14-byte header with its CRC.
    pub fn header(&self) -> [u8; HEADER_LEN] {
        let mut h = [0u8; HEADER_LEN];
        h[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        h[4..6].copy_from_slice(&self.command.to_le_bytes());
        h[6] = self.flags;
        h[7] = self.sequence;
        h[8..10].copy_from_slice(&(self.body.len() as u16).to_le_bytes());
        let crc = crc32(&h[0..10]);
        h[10..14].copy_from_slice(&crc.to_le_bytes());
        h
    }

    /// Full wire encoding: header + body + (body CRC when non-empty).
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.body.len() + CRC_LEN);
        out.extend_from_slice(&self.header());
        out.extend_from_slice(&self.body);
        if !self.body.is_empty() {
            out.extend_from_slice(&crc32(&self.body).to_le_bytes());
        }
        out
    }

    /// Decode a complete frame, verifying both CRCs.
    pub fn decode(buf: &[u8]) -> Result<Self, FrameError> {
        if buf.len() < HEADER_LEN {
            return Err(FrameError::Truncated {
                need: HEADER_LEN,
                have: buf.len(),
            });
        }
        let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        if magic != MAGIC {
            return Err(FrameError::BadMagic(magic));
        }
        let command = u16::from_le_bytes([buf[4], buf[5]]);
        let flags = buf[6];
        let sequence = buf[7];
        let body_len = u16::from_le_bytes([buf[8], buf[9]]) as usize;
        let found_header_crc = u32::from_le_bytes([buf[10], buf[11], buf[12], buf[13]]);
        let computed_header_crc = crc32(&buf[0..10]);
        if found_header_crc != computed_header_crc {
            return Err(FrameError::HeaderCrc {
                computed: computed_header_crc,
                found: found_header_crc,
            });
        }

        let body_end = HEADER_LEN + body_len;
        if body_len == 0 {
            return Ok(Self::new(command, flags, sequence, Vec::new()));
        }
        if buf.len() < body_end + CRC_LEN {
            return Err(FrameError::Truncated {
                need: body_end + CRC_LEN,
                have: buf.len(),
            });
        }
        let body = buf[HEADER_LEN..body_end].to_vec();
        let found_body_crc = u32::from_le_bytes([
            buf[body_end],
            buf[body_end + 1],
            buf[body_end + 2],
            buf[body_end + 3],
        ]);
        let computed_body_crc = crc32(&body);
        if found_body_crc != computed_body_crc {
            return Err(FrameError::BodyCrc {
                computed: computed_body_crc,
                found: found_body_crc,
            });
        }
        Ok(Self::new(command, flags, sequence, body))
    }
}

/// Incremental frame decoder, mirroring the app's byte-at-a-time state machine.
///
/// The vendor parser hunts for the `B`/`U`/`X` sync sequence before accepting a
/// header, which lets it resynchronise after a truncated transfer. This decoder
/// keeps that behaviour so it can be fed raw report streams directly.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    state: u8,
    sync_index: u8,
    header: [u8; HEADER_LEN],
    header_pos: usize,
    body: Vec<u8>,
    body_pos: usize,
    crc_bytes: [u8; CRC_LEN],
    crc_pos: usize,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset all state, as `sc6.d()` does.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Feed one byte; returns a frame once one is complete and verified.
    pub fn push(&mut self, byte: u8) -> Option<Result<Frame, FrameError>> {
        match self.state {
            0 => {
                // Hunt for the four-byte 'B' 'U' 'X' 'X' sync sequence.
                let expected = [SYNC_B, SYNC_U, SYNC_X, SYNC_X];
                if byte == expected[self.sync_index as usize] {
                    self.sync_index += 1;
                    if self.sync_index == 4 {
                        self.sync_index = 0;
                        self.header = [0u8; HEADER_LEN];
                        // The magic bytes we just consumed open the header.
                        self.header[0..4].copy_from_slice(&expected);
                        self.header_pos = 4;
                        self.state = 1;
                    }
                } else {
                    // Restart the hunt, allowing this byte to begin a new match.
                    self.sync_index = u8::from(byte == SYNC_B);
                }
                None
            }
            1 => {
                self.header[self.header_pos] = byte;
                self.header_pos += 1;
                if self.header_pos == HEADER_LEN {
                    let magic = u32::from_le_bytes([
                        self.header[0],
                        self.header[1],
                        self.header[2],
                        self.header[3],
                    ]);
                    if magic != MAGIC {
                        self.reset();
                        return Some(Err(FrameError::BadMagic(magic)));
                    }
                    let computed = crc32(&self.header[0..10]);
                    let found = u32::from_le_bytes([
                        self.header[10],
                        self.header[11],
                        self.header[12],
                        self.header[13],
                    ]);
                    if computed != found {
                        self.reset();
                        return Some(Err(FrameError::HeaderCrc { computed, found }));
                    }
                    let body_len = u16::from_le_bytes([self.header[8], self.header[9]]) as usize;
                    if body_len == 0 {
                        let frame = self.take_header_only();
                        return Some(Ok(frame));
                    }
                    if body_len > 4096 {
                        // Implausible length: treat as loss of sync.
                        self.reset();
                        return None;
                    }
                    self.body = vec![0u8; body_len];
                    self.body_pos = 0;
                    self.state = 2;
                }
                None
            }
            2 => {
                self.body[self.body_pos] = byte;
                self.body_pos += 1;
                if self.body_pos == self.body.len() {
                    self.crc_bytes = [0u8; CRC_LEN];
                    self.crc_pos = 0;
                    self.state = 3;
                }
                None
            }
            _ => {
                self.crc_bytes[self.crc_pos] = byte;
                self.crc_pos += 1;
                if self.crc_pos == CRC_LEN {
                    let computed = crc32(&self.body);
                    let found = u32::from_le_bytes(self.crc_bytes);
                    let body = std::mem::take(&mut self.body);
                    let (command, flags, sequence) = self.header_fields();
                    self.reset();
                    if computed != found {
                        return Some(Err(FrameError::BodyCrc { computed, found }));
                    }
                    return Some(Ok(Frame::new(command, flags, sequence, body)));
                }
                None
            }
        }
    }

    fn header_fields(&self) -> (u16, u8, u8) {
        (
            u16::from_le_bytes([self.header[4], self.header[5]]),
            self.header[6],
            self.header[7],
        )
    }

    fn take_header_only(&mut self) -> Frame {
        let (command, flags, sequence) = self.header_fields();
        self.reset();
        Frame::new(command, flags, sequence, Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_spells_buxx_on_the_wire() {
        assert_eq!(&MAGIC.to_le_bytes(), b"BUXX");
        assert_eq!(MAGIC, 1482184002);
    }

    #[test]
    fn header_layout_matches_app() {
        let f = Frame::new(OtaCommand::Handshake as u16, 7, 9, vec![1, 2, 3]);
        let h = f.header();
        assert_eq!(&h[0..4], b"BUXX");
        assert_eq!(u16::from_le_bytes([h[4], h[5]]), 0x10);
        assert_eq!(h[6], 7);
        assert_eq!(h[7], 9);
        assert_eq!(u16::from_le_bytes([h[8], h[9]]), 3);
        let stored = u32::from_le_bytes([h[10], h[11], h[12], h[13]]);
        assert_eq!(stored, crc32(&h[0..10]));
    }

    #[test]
    fn roundtrips_with_body() {
        let original = Frame::new(OtaCommand::WriteFlashData as u16, 0, 1, vec![0xAA; 100]);
        let wire = original.encode();
        assert_eq!(wire.len(), HEADER_LEN + 100 + CRC_LEN);
        assert_eq!(Frame::decode(&wire).unwrap(), original);
    }

    #[test]
    fn roundtrips_without_body() {
        let original = Frame::new(OtaCommand::EndAndReboot as u16, 0, 2, Vec::new());
        let wire = original.encode();
        // No body CRC is emitted for an empty body.
        assert_eq!(wire.len(), HEADER_LEN);
        assert_eq!(Frame::decode(&wire).unwrap(), original);
    }

    #[test]
    fn detects_header_corruption() {
        let mut wire = Frame::new(0x10, 0, 0, vec![1]).encode();
        wire[7] ^= 0xFF;
        assert!(matches!(
            Frame::decode(&wire),
            Err(FrameError::HeaderCrc { .. })
        ));
    }

    #[test]
    fn detects_body_corruption() {
        let mut wire = Frame::new(0x21, 0, 0, vec![1, 2, 3, 4]).encode();
        let idx = HEADER_LEN;
        wire[idx] ^= 0xFF;
        assert!(matches!(
            Frame::decode(&wire),
            Err(FrameError::BodyCrc { .. })
        ));
    }

    #[test]
    fn rejects_bad_magic() {
        let mut wire = Frame::new(0x10, 0, 0, Vec::new()).encode();
        wire[0] = b'Z';
        assert!(matches!(Frame::decode(&wire), Err(FrameError::BadMagic(_))));
    }

    #[test]
    fn reports_truncation() {
        let wire = Frame::new(0x21, 0, 0, vec![0; 32]).encode();
        assert!(matches!(
            Frame::decode(&wire[..20]),
            Err(FrameError::Truncated { .. })
        ));
    }

    #[test]
    fn decoder_resynchronises_after_garbage() {
        let mut dec = FrameDecoder::new();
        // Garbage prefix, including a partial sync sequence.
        let mut result = None;
        for b in [0x00, 0x42, 0x00, 0x42, 0x55] {
            result = dec.push(b);
            assert!(result.is_none());
        }
        let wire = Frame::new(OtaCommand::Handshake as u16, 3, 4, vec![9, 9]).encode();
        for (i, b) in wire.iter().enumerate() {
            if let Some(r) = dec.push(*b) {
                result = Some(r);
                assert_eq!(i, wire.len() - 1, "frame should complete on last byte");
            }
        }
        let frame = result.expect("frame decoded").expect("no error");
        assert_eq!(frame.command, 0x10);
        assert_eq!(frame.flags, 3);
        assert_eq!(frame.sequence, 4);
        assert_eq!(frame.body, vec![9, 9]);
    }

    #[test]
    fn decoder_handles_back_to_back_frames() {
        let mut dec = FrameDecoder::new();
        let a = Frame::new(0x10, 0, 0, vec![1]).encode();
        let b = Frame::new(0x24, 0, 1, vec![2, 3]).encode();
        let mut frames = Vec::new();
        for byte in a.iter().chain(b.iter()) {
            if let Some(Ok(f)) = dec.push(*byte) {
                frames.push(f);
            }
        }
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].body, vec![1]);
        assert_eq!(frames[1].body, vec![2, 3]);
    }

    #[test]
    fn ota_command_labels_match_app() {
        assert_eq!(OtaCommand::Handshake.label(), "HANDSHAKE(0x10)");
        assert_eq!(
            OtaCommand::SetFlashWriteArea.label(),
            "SET_FLASH_WRITE_AREA(0x20)"
        );
        assert_eq!(OtaCommand::WriteFlashData.label(), "WRITE_FLASH_DATA(0x21)");
        assert_eq!(OtaCommand::EndAndReboot.label(), "END_AND_REBOOT(0x24)");
    }
}
