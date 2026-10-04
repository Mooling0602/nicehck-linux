//! CRC-32 (reflected, polynomial 0xEDB88320), as used by the NICEHCK TTGK protocol.
//!
//! Recovered from `Lrh0;->z(I [B)I` in the 2.3.8 APK. That routine initialises the
//! accumulator to `-1` (0xFFFFFFFF), walks `length` bytes, shifts right, and XORs
//! with `0xEDB88320` (`-306674912` as a signed int32) whenever the low bit is set.
//! It finally returns the bitwise NOT of the accumulator — i.e. exactly the
//! standard CRC-32/ISO-HDLC (zlib) checksum.

/// Standard CRC-32 (zlib / ISO-HDLC). Returns the same value as `crc32()` in zlib.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

/// Table-driven CRC-32, producing identical output to [`crc32`].
pub fn crc32_table(data: &[u8]) -> u32 {
    static TABLE: [u32; 256] = build_table();
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc = (crc >> 8) ^ TABLE[((crc ^ byte as u32) & 0xFF) as usize];
    }
    !crc
}

const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_known_crc32_vectors() {
        // Well-known CRC-32/ISO-HDLC ("zlib") test vectors.
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
    }

    #[test]
    fn table_matches_bitwise() {
        for len in 0..64usize {
            let data: Vec<u8> = (0..len).map(|i| (i * 37 % 251) as u8).collect();
            assert_eq!(crc32(&data), crc32_table(&data), "mismatch at len {len}");
        }
    }
}
