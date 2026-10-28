//! Film type handling: positives (slides), colour negatives (orange mask removal and
//! inversion) and black & white.

use rayon::prelude::*;

use crate::Image16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FilmType {
    #[default]
    Positive,
    Negative,
    BwPositive,
    BwNegative,
}

impl FilmType {
    pub const ALL: [FilmType; 4] = [FilmType::Positive, FilmType::Negative, FilmType::BwPositive, FilmType::BwNegative];

    pub fn label(self) -> &'static str {
        match self {
            FilmType::Positive => "Slide (positive)",
            FilmType::Negative => "Colour negative",
            FilmType::BwPositive => "B&W positive",
            FilmType::BwNegative => "B&W negative",
        }
    }

    /// Infrared cleaning only works where the image dyes are IR-transparent; silver
    /// (B&W) images block IR, so the result would destroy detail.
    pub fn supports_ir_cleaning(self) -> bool {
        matches!(self, FilmType::Positive | FilmType::Negative)
    }
}

fn percentile(p: &[f32], q: f32) -> f32 {
    let mut s: Vec<f32> = p.iter().step_by(7).copied().collect();
    if s.is_empty() {
        return 0.0;
    }
    let k = ((s.len() - 1) as f32 * q) as usize;
    s.select_nth_unstable_by(k, |a, b| a.partial_cmp(b).unwrap());
    s[k]
}

/// Converts the linear scan into a linear positive image.
pub fn convert(img: &mut Image16, film: FilmType) {
    match film {
        FilmType::Positive => {}
        FilmType::BwPositive => to_gray(img),
        FilmType::Negative => invert_negative(img),
        FilmType::BwNegative => {
            to_gray(img);
            invert_negative(img);
        }
    }
}

fn to_gray(img: &mut Image16) {
    let n = img.len();
    let g: Vec<f32> = (0..n).into_par_iter().map(|i| 0.2126 * img.rgb[0][i] + 0.7152 * img.rgb[1][i] + 0.0722 * img.rgb[2][i]).collect();
    img.rgb = [g.clone(), g.clone(), g];
}

/// Density-based inversion: the film base (orange mask) is the brightest part of a
/// negative; each channel is divided by it, inverted in log space and stretched.
fn invert_negative(img: &mut Image16) {
    for c in 0..3 {
        let base = percentile(&img.rgb[c], 0.995).max(1e-3);
        let dense = percentile(&img.rgb[c], 0.005).max(1e-5);
        let dmax = (base / dense).log10().max(0.3);
        img.rgb[c].par_iter_mut().for_each(|v| {
            let d = (base / v.max(1e-5)).log10().clamp(0.0, dmax) / dmax;
            // density is proportional to log exposure; map back to linear light
            *v = (10f32.powf(d * 2.2) - 1.0) / (10f32.powf(2.2) - 1.0);
        });
    }
}
