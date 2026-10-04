//! Biquad (peaking EQ) coefficient computation, matching the vendor app exactly.
//!
//! Recovered from `Lji0;->A(DDDDZ)Lbk1;` in the 2.3.8 APK, which is the standard
//! RBJ audio-EQ-cookbook peaking filter:
//!
//! ```text
//! w0    = 2*pi*f0/Fs
//! A     = sqrt(10^(gain_db/20))
//! alpha = sin(w0)/(2*Q)
//! b0 = 1 + alpha*A      b1 = -2*cos(w0)      b2 = 1 - alpha*A
//! a0 = 1 + alpha/A      a1 = -2*cos(w0)      a2 = 1 - alpha/A
//! ```
//!
//! The app then normalises by `a0`, so the transmitted coefficients are
//! `b0/a0, b1/a0, b2/a0, -a1/a0, -a2/a0` quantised to signed Q30.

/// Q30 fixed-point scale used by the DSP for biquad coefficients.
pub const COEFF_SCALE: f64 = 1_073_741_824.0; // 2^30

/// Normalised biquad coefficients, `a0` already divided out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    /// `b0/a0`
    pub b0: f64,
    /// `b1/a0`
    pub b1: f64,
    /// `b2/a0`
    pub b2: f64,
    /// `a1/a0` (note: positive sign; the wire format transmits its negation)
    pub a1: f64,
    /// `a2/a0`
    pub a2: f64,
}

/// Compute peaking-EQ biquad coefficients for one band.
///
/// * `gain_db` – band gain in dB
/// * `freq` – centre frequency in Hz
/// * `q` – quality factor
/// * `sample_rate` – DSP sample rate in Hz (96000 for most NICEHCK DSP devices)
pub fn peaking(gain_db: f64, freq: f64, q: f64, sample_rate: f64) -> Biquad {
    let w0 = 2.0 * std::f64::consts::PI * freq / sample_rate;
    let a = 10f64.powf(gain_db / 20.0).sqrt();
    let alpha = w0.sin() / (2.0 * q);
    let alpha_a = alpha * a;
    let alpha_over_a = alpha / a;
    let cos_w0 = w0.cos();

    let b0 = 1.0 + alpha_a;
    let b1 = -2.0 * cos_w0;
    let b2 = 1.0 - alpha_a;
    let a0 = 1.0 + alpha_over_a;
    let a1 = -2.0 * cos_w0;
    let a2 = 1.0 - alpha_over_a;

    Biquad {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

/// Round-half-up to i32, matching Kotlin/Java `Math.round(double)`.
///
/// Java defines `Math.round(x)` as `floor(x + 0.5)`, which differs from Rust's
/// `f64::round` (half away from zero) for negative halves: `Math.round(-2.5) == -2`
/// but `(-2.5f64).round() == -3`. The vendor app routes every coefficient and
/// fixed-point field through this, so the distinction changes transmitted bytes.
pub fn round_i32(v: f64) -> i32 {
    if v.is_nan() {
        return 0;
    }
    if v >= i32::MAX as f64 {
        return i32::MAX;
    }
    if v <= i32::MIN as f64 {
        return i32::MIN;
    }
    (v + 0.5).floor() as i32
}

/// Quantise a normalised coefficient into signed Q30, as sent on the wire.
pub fn quantise_q30(coefficient: f64) -> i32 {
    round_i32(coefficient * COEFF_SCALE)
}

/// Quantise a real-world value into the 1/256 fixed-point field used for
/// frequency, gain and Q in the band report.
pub fn quantise_q8(value: f64) -> i16 {
    let scaled = round_i32(value * 256.0);
    scaled.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_band_is_unity() {
        // A 0 dB band must be a pass-through: b == a, so b/a is all-pass identity.
        let c = peaking(0.0, 1000.0, 0.75, 96_000.0);
        assert!((c.b0 - 1.0).abs() < 1e-12, "b0={}", c.b0);
        assert!((c.b1 - c.a1).abs() < 1e-12);
        assert!((c.b2 - c.a2).abs() < 1e-12);
    }

    #[test]
    fn matches_reference_for_known_band() {
        // Hand-computed RBJ reference for +6 dB @ 1 kHz, Q=1, Fs=96 kHz.
        let c = peaking(6.0, 1000.0, 1.0, 96_000.0);
        let a = 10f64.powf(6.0 / 40.0); // sqrt(10^(6/20))
        let w0 = 2.0 * std::f64::consts::PI * 1000.0 / 96_000.0;
        let alpha = w0.sin() / 2.0;
        let a0 = 1.0 + alpha / a;
        assert!((c.b0 - (1.0 + alpha * a) / a0).abs() < 1e-12);
        assert!((c.a1 - (-2.0 * w0.cos()) / a0).abs() < 1e-12);
        assert!((c.a2 - (1.0 - alpha / a) / a0).abs() < 1e-12);
    }

    #[test]
    fn q30_roundtrip_is_stable() {
        let c = peaking(4.5, 250.0, 0.35, 96_000.0);
        let q = quantise_q30(c.b0);
        assert!((q as f64 / COEFF_SCALE - c.b0).abs() < 1e-9);
    }

    #[test]
    fn round_i32_matches_kotlin_rounding() {
        assert_eq!(round_i32(2.5), 3);
        assert_eq!(round_i32(-2.5), -2);
        assert_eq!(round_i32(2.4), 2);
        assert_eq!(round_i32(f64::NAN), 0);
    }
}
