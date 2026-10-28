//! Automatic exposure, modelled on what CyberView X5 sends to a DigitDia 4000 (tray capture):
//!
//! * every slide is first previewed with fixed base exposures at gain code 0x19;
//! * the preview histogram yields a per-channel exposure factor;
//! * the final scan raises the gain codes (0x36/0x3c/0x3f) and shortens the exposure so that
//!   `exposure * gain(code)` stays equal to the corrected preview exposure.
//!
//! The gain-code curve is the one measured by the SANE pieusb backend.

use crate::scanner::RawScan;
use crate::scsi::GainOffset;

const GAINS: [f64; 13] = [1.000, 1.075, 1.154, 1.251, 1.362, 1.491, 1.653, 1.858, 2.115, 2.458, 2.935, 3.638, 4.627];

/// Amplification of a gain code (0..=63).
pub fn gain_factor(code: u8) -> f64 {
    let c = code as usize;
    if c >= 60 {
        return (c as f64 - 55.0) * (GAINS[12] - GAINS[11]) / 5.0 + GAINS[11];
    }
    let k = c / 5;
    (c - 5 * k) as f64 * (GAINS[k + 1] - GAINS[k]) / 5.0 + GAINS[k]
}

/// Preview settings CyberView used for the first pass of every slide.
pub const PREVIEW: GainOffset = GainOffset {
    exposure: [0x7ac2, 0x7582, 0x4daf, 0x177e],
    offset: [0x0e, 0x13, 0x0f, 0x13],
    gain: [0x19, 0x19, 0x19, 0x35],
    light: 6,
};

/// Gain codes CyberView used for final scans.
pub const FINAL_GAIN: [u8; 3] = [0x36, 0x3c, 0x3f];

/// Highest usable exposure value.
const MAX_EXPOSURE: f64 = 65000.0;

/// Value of the given quantile of a plane (0..65535).
fn quantile(p: &[u16], q: f64) -> f64 {
    let mut hist = [0u32; 1024];
    for &v in p {
        hist[(v >> 6) as usize] += 1;
    }
    let target = (p.len() as f64 * q) as u64;
    let mut acc = 0u64;
    for (i, &n) in hist.iter().enumerate() {
        acc += n as u64;
        if acc >= target {
            return ((i << 6) + 32) as f64;
        }
    }
    65535.0
}

/// Computes final-scan gain/offset from a preview taken with `PREVIEW`.
/// `bias` (in stops) brightens (>0) or darkens (<0) the result.
pub fn from_preview(preview: &RawScan, bias: f64) -> GainOffset {
    let mut out = PREVIEW;
    // Use the central area: the slide mount at the edges is opaque or blank.
    let crop = |p: &[u16]| -> Vec<u16> {
        let (w, h) = (preview.width, preview.height);
        let (x0, x1, y0, y1) = (w / 6, w * 5 / 6, h / 6, h * 5 / 6);
        (y0..y1).flat_map(|y| p[y * w + x0..y * w + x1].to_vec()).collect()
    };
    let target = 0.92 * 65535.0 * 2f64.powf(bias.min(0.0));
    let planes = [crop(&preview.red), crop(&preview.green), crop(&preview.blue)];
    for c in 0..3 {
        let hi = quantile(&planes[c], 0.995).max(512.0);
        let k = (target / hi * 2f64.powf(bias.max(0.0))).clamp(0.25, 16.0);
        // Dense slides need more light than CyberView's final gain allows: raise the gain up
        // to the hardware maximum before the 16-bit exposure register saturates.
        let total = (PREVIEW.exposure[c] as f64 * gain_factor(PREVIEW.gain[c]) * k).min(MAX_EXPOSURE * gain_factor(63));
        let mut code = FINAL_GAIN[c];
        while code < 63 && total / gain_factor(code) > MAX_EXPOSURE {
            code += 1;
        }
        // Use as little gain as the exposure range allows (less noise).
        while code > PREVIEW.gain[c] && total / gain_factor(code - 1) <= MAX_EXPOSURE * 0.6 {
            code -= 1;
        }
        out.gain[c] = code;
        out.exposure[c] = (total / gain_factor(code)).clamp(256.0, MAX_EXPOSURE) as u16;
    }
    out
}

/// A preview of an empty slot shows the bare lamp: bright and nearly uniform.
pub fn looks_empty(preview: &RawScan) -> bool {
    let g = &preview.green;
    if g.is_empty() {
        return true;
    }
    let lo = quantile(g, 0.05);
    let hi = quantile(g, 0.95);
    lo > 0.55 * 65535.0 && (hi - lo) < 0.12 * 65535.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_curve_reproduces_cyberview_final_exposures() {
        // Slide 1 of the tray capture: pass-2 exposures at gain 0x19 vs. final exposures.
        let pass2 = [0x9fda as f64, 0x98c3 as f64, 0x64ef as f64];
        let fin = [0x440d as f64, 0x2f07 as f64, 0x1910 as f64];
        for c in 0..3 {
            let ratio = fin[c] * gain_factor(FINAL_GAIN[c]) / (pass2[c] * gain_factor(0x19));
            assert!((ratio - 1.0).abs() < 0.15, "channel {c}: {ratio}");
        }
    }
}
