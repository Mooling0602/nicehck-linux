//! Frequency-response curve rendering.
//!
//! The device stores each band as a peaking biquad, so we evaluate the same
//! transfer function the DSP implements: `H(z)` sampled along a log frequency
//! axis. That gives an honest preview of what the hardware will do rather than a
//! cosmetic spline through the control points.

use nicehck_protocol::biquad;
use nicehck_protocol::command::Band;

/// Magnitude response in dB of one peaking band at `freq` Hz.
fn band_response_db(band: &Band, freq: f64, sample_rate: f64) -> f64 {
    let c = biquad::peaking(
        band.gain as f64,
        band.frequency as f64,
        band.q as f64,
        sample_rate,
    );
    // Evaluate |H(e^{jw})| for H(z) = (b0 + b1 z^-1 + b2 z^-2)/(1 + a1 z^-1 + a2 z^-2).
    let w = 2.0 * std::f64::consts::PI * freq / sample_rate;
    let (sin1, cos1) = (w.sin(), w.cos());
    let (sin2, cos2) = ((2.0 * w).sin(), (2.0 * w).cos());

    let num_re = c.b0 + c.b1 * cos1 + c.b2 * cos2;
    let num_im = -(c.b1 * sin1 + c.b2 * sin2);
    let den_re = 1.0 + c.a1 * cos1 + c.a2 * cos2;
    let den_im = -(c.a1 * sin1 + c.a2 * sin2);

    let num = (num_re * num_re + num_im * num_im).sqrt();
    let den = (den_re * den_re + den_im * den_im).sqrt();
    // A missing a0 term is impossible here: peaking() normalises by a0 already.
    20.0 * (num / den).log10()
}

/// Combined response of every band plus the global offset, in dB.
pub fn combined_response_db(bands: &[Band], offset_db: f64, freq: f64, sample_rate: f64) -> f64 {
    bands
        .iter()
        .map(|b| band_response_db(b, freq, sample_rate))
        .sum::<f64>()
        + offset_db
}

/// A log-spaced frequency sweep suitable for plotting.
pub fn log_sweep(min_hz: f64, max_hz: f64, points: usize) -> Vec<f64> {
    if points < 2 {
        return vec![min_hz];
    }
    let (lo, hi) = (min_hz.max(10.0).log10(), max_hz.max(min_hz).log10());
    (0..points)
        .map(|i| {
            let t = i as f64 / (points - 1) as f64;
            10f64.powf(lo + (hi - lo) * t)
        })
        .collect()
}

/// Sample the combined curve for plotting, as `(frequency_hz, gain_db)` pairs.
pub fn curve_points(
    bands: &[Band],
    offset_db: f64,
    sample_rate: f64,
    min_hz: f64,
    max_hz: f64,
    points: usize,
) -> Vec<[f64; 2]> {
    log_sweep(min_hz, max_hz, points)
        .into_iter()
        .map(|f| [f, combined_response_db(bands, offset_db, f, sample_rate)])
        .collect()
}

/// Peak-to-peak gain swing of the curve, used to auto-scale the plot.
pub fn curve_range(points: &[[f64; 2]]) -> (f64, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for p in points {
        lo = lo.min(p[1]);
        hi = hi.max(p[1]);
    }
    if !lo.is_finite() || !hi.is_finite() {
        return (-12.0, 12.0);
    }
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(index: u8, freq: u16, gain: f32, q: f32) -> Band {
        Band {
            index,
            frequency: freq,
            gain,
            q,
        }
    }

    #[test]
    fn flat_bands_produce_flat_curve() {
        let bands = vec![band(0, 1000, 0.0, 0.75)];
        for f in log_sweep(20.0, 20000.0, 32) {
            let db = combined_response_db(&bands, 0.0, f, 96_000.0);
            assert!(db.abs() < 1e-6, "{f} Hz gave {db} dB");
        }
    }

    #[test]
    fn boost_peaks_at_centre_frequency() {
        let bands = vec![band(0, 1000, 6.0, 1.0)];
        let at_centre = combined_response_db(&bands, 0.0, 1000.0, 96_000.0);
        // A peaking filter applies (approximately) the full gain at f0.
        assert!((at_centre - 6.0).abs() < 0.1, "centre gain {at_centre} dB");
        let far = combined_response_db(&bands, 0.0, 20.0, 96_000.0);
        assert!(far.abs() < 0.5, "far-field gain {far} dB");
    }

    #[test]
    fn cut_is_negative_at_centre() {
        let bands = vec![band(0, 500, -8.0, 2.0)];
        let db = combined_response_db(&bands, 0.0, 500.0, 96_000.0);
        assert!((db + 8.0).abs() < 0.2, "centre gain {db} dB");
    }

    #[test]
    fn offset_shifts_whole_curve() {
        let bands = vec![band(0, 1000, 0.0, 0.75)];
        let base = combined_response_db(&bands, 0.0, 1000.0, 96_000.0);
        let shifted = combined_response_db(&bands, 3.0, 1000.0, 96_000.0);
        assert!((shifted - base - 3.0).abs() < 1e-9);
    }

    #[test]
    fn bands_sum_independently() {
        let a = vec![band(0, 200, 4.0, 1.0)];
        let b = vec![band(0, 5000, 3.0, 1.0)];
        let both: Vec<Band> = a.iter().chain(b.iter()).copied().collect();
        for f in [100.0, 200.0, 1000.0, 5000.0] {
            let sa = combined_response_db(&a, 0.0, f, 96_000.0);
            let sb = combined_response_db(&b, 0.0, f, 96_000.0);
            let s = combined_response_db(&both, 0.0, f, 96_000.0);
            assert!((s - (sa + sb)).abs() < 1e-9, "at {f} Hz");
        }
    }

    #[test]
    fn sweep_is_log_spaced_and_bounded() {
        let s = log_sweep(20.0, 20000.0, 100);
        assert_eq!(s.len(), 100);
        assert!((s[0] - 20.0).abs() < 1e-6);
        assert!((s[99] - 20000.0).abs() < 1e-3);
        for w in s.windows(2) {
            assert!(w[1] > w[0], "sweep must be monotonic");
        }
    }

    #[test]
    fn range_reports_curve_extremes() {
        let pts = vec![[20.0, -3.0], [100.0, 5.0], [1000.0, 1.0]];
        let (lo, hi) = curve_range(&pts);
        assert_eq!((lo, hi), (-3.0, 5.0));
    }
}
