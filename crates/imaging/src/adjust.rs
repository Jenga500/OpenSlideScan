//! Colour and tone adjustments (CyberView "Image Adjustment": colour balance, curves and
//! levels, auto colour/contrast, sharpening, grain reduction).

use rayon::prelude::*;

use crate::dust::{remove_dust, DustOptions};
use crate::film::{convert, FilmType};
use crate::Image16;

#[derive(Clone, Debug, PartialEq)]
pub struct Adjustments {
    pub film: FilmType,
    /// IR dust removal strength 0..1 (0 = off).
    pub dust: f32,
    /// Exposure correction in stops.
    pub exposure: f32,
    /// -1..1
    pub brightness: f32,
    /// -1..1
    pub contrast: f32,
    /// -1..1
    pub saturation: f32,
    /// Colour balance -1..1: cyan/red, magenta/green, yellow/blue.
    pub balance: [f32; 3],
    /// Levels black/white points (0..1) and midtone gamma.
    pub black: f32,
    pub white: f32,
    pub gamma: f32,
    pub auto_levels: bool,
    /// Neutralise colour casts of faded slides (restoration, "ROC"-like).
    pub auto_color: bool,
    /// 0..2 unsharp mask amount.
    pub sharpen: f32,
    /// 0..1 grain / noise reduction.
    pub grain: f32,
    /// Remove sensor noise and streaks automatically, but only on dark (dense) slides.
    pub auto_dark_denoise: bool,
    /// Quarter turns clockwise.
    pub rotation: u8,
    pub flip_h: bool,
    pub flip_v: bool,
    /// Relative crop rectangle (x, y, w, h) in 0..1 of the oriented image.
    pub crop: Option<[f32; 4]>,
}

impl Default for Adjustments {
    fn default() -> Self {
        Self {
            film: FilmType::Positive,
            dust: 0.5,
            exposure: 0.0,
            brightness: 0.0,
            contrast: 0.0,
            saturation: 0.0,
            balance: [0.0; 3],
            black: 0.0,
            white: 1.0,
            gamma: 1.0,
            auto_levels: true,
            auto_color: false,
            sharpen: 0.3,
            grain: 0.0,
            auto_dark_denoise: true,
            rotation: 1,
            flip_h: false,
            flip_v: false,
            crop: None,
        }
    }
}

fn srgb_encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

fn percentile(p: &[f32], q: f32) -> f32 {
    let mut s: Vec<f32> = p.iter().step_by(5).copied().collect();
    if s.is_empty() {
        return 0.0;
    }
    let k = ((s.len() - 1) as f32 * q) as usize;
    s.select_nth_unstable_by(k, |a, b| a.partial_cmp(b).unwrap());
    s[k]
}

fn blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let s = &src[y * w..(y + 1) * w];
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            row[x] = s[a..=b].iter().sum::<f32>() / (b - a + 1) as f32;
        }
    });
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
        for x in 0..w {
            let mut s = 0.0;
            for yy in a..=b {
                s += tmp[yy * w + x];
            }
            row[x] = s / (b - a + 1) as f32;
        }
    });
    out
}

/// Full processing pipeline from the shading-corrected linear scan to a display-ready
/// (sRGB-encoded) image.
pub fn process(src: &Image16, a: &Adjustments) -> Image16 {
    let mut img = src.clone();
    if a.dust > 0.0 && a.film.supports_ir_cleaning() {
        remove_dust(&mut img, &DustOptions { strength: a.dust, grow: 2 });
    }
    // Dense slides sit near the sensor's noise floor: remove line-sensor streaks and noise,
    // scaled by how dark the slide is. Normal slides are left untouched.
    if a.auto_dark_denoise {
        let dark = crate::denoise::darkness(&img);
        if dark > 0.0 {
            crate::denoise::destripe(&mut img);
            crate::denoise::denoise(&mut img, 0.25 + 0.6 * dark);
        }
    }
    convert(&mut img, a.film);

    // orientation and crop
    for _ in 0..(a.rotation % 4) {
        img = img.rotate90(true);
    }
    if a.flip_h {
        img = img.flip(true);
    }
    if a.flip_v {
        img = img.flip(false);
    }
    if let Some([x, y, w, h]) = a.crop {
        let (iw, ih) = (img.width as f32, img.height as f32);
        img = img.crop((x * iw) as usize, (y * ih) as usize, ((w * iw) as usize).max(1), ((h * ih) as usize).max(1));
    }

    // linear-light corrections
    let gain = 2f32.powf(a.exposure);
    let mut lo = [0f32; 3];
    let mut hi = [1f32; 3];
    if a.auto_levels || a.auto_color {
        let l: Vec<f32> = (0..3).map(|c| percentile(&img.rgb[c], 0.002)).collect();
        let h: Vec<f32> = (0..3).map(|c| percentile(&img.rgb[c], 0.998)).collect();
        for c in 0..3 {
            if a.auto_color {
                lo[c] = l[c];
                hi[c] = h[c].max(l[c] + 1e-3);
            } else {
                lo[c] = l.iter().cloned().fold(f32::MAX, f32::min);
                hi[c] = h.iter().cloned().fold(0.0, f32::max).max(lo[c] + 1e-3);
            }
        }
    }
    let bal = a.balance;
    for c in 0..3 {
        let (l, h) = (lo[c], hi[c]);
        let cb = 2f32.powf(bal[c] * 0.5);
        img.rgb[c].par_iter_mut().for_each(|v| {
            *v = ((*v - l) / (h - l)).max(0.0) * gain * cb;
        });
    }

    // tone: levels, gamma, sRGB, brightness/contrast
    let (b, w) = (a.black, a.white.max(a.black + 1e-3));
    let inv_g = 1.0 / a.gamma.max(0.05);
    let contrast = 1.0 + a.contrast;
    for c in 0..3 {
        img.rgb[c].par_iter_mut().for_each(|v| {
            let mut x = srgb_encode(*v);
            x = ((x - b) / (w - b)).clamp(0.0, 1.0).powf(inv_g);
            x = (x - 0.5) * contrast + 0.5 + a.brightness * 0.5;
            *v = x.clamp(0.0, 1.0);
        });
    }

    // saturation
    if a.saturation != 0.0 {
        let s = 1.0 + a.saturation;
        let n = img.len();
        let (r, rest) = img.rgb.split_at_mut(1);
        let (g, bl) = rest.split_at_mut(1);
        r[0].par_iter_mut().zip(g[0].par_iter_mut()).zip(bl[0].par_iter_mut()).for_each(|((r, g), b)| {
            let y = 0.2126 * *r + 0.7152 * *g + 0.0722 * *b;
            *r = (y + (*r - y) * s).clamp(0.0, 1.0);
            *g = (y + (*g - y) * s).clamp(0.0, 1.0);
            *b = (y + (*b - y) * s).clamp(0.0, 1.0);
        });
        let _ = n;
    }

    let (w, h) = (img.width, img.height);
    if w > 4 && h > 4 {
        // grain reduction: blend with a small blur, stronger in flat areas
        if a.grain > 0.0 {
            let r = 1 + (a.grain * 2.0) as usize;
            for c in 0..3 {
                let bl = blur(&img.rgb[c], w, h, r);
                let k = a.grain.clamp(0.0, 1.0);
                img.rgb[c].par_iter_mut().zip(bl.par_iter()).for_each(|(v, &m)| {
                    let edge = ((*v - m).abs() * 12.0).min(1.0);
                    *v = *v + (m - *v) * k * (1.0 - edge);
                });
            }
        }
        // unsharp mask
        if a.sharpen > 0.0 {
            let r = (img.dpi as usize / 1200).clamp(1, 3);
            for c in 0..3 {
                let bl = blur(&img.rgb[c], w, h, r);
                img.rgb[c].par_iter_mut().zip(bl.par_iter()).for_each(|(v, &m)| {
                    *v = (*v + (*v - m) * a.sharpen).clamp(0.0, 1.0);
                });
            }
        }
    }
    img.ir = None;
    img
}

/// Finds the image area inside a slide mount: the mount is opaque (dark) on a positive.
/// Returns a relative crop (x, y, w, h) of the unrotated scan.
pub fn auto_crop(img: &Image16, film: FilmType) -> Option<[f32; 4]> {
    let (w, h) = (img.width, img.height);
    if w < 16 || h < 16 {
        return None;
    }
    let lum: Vec<f32> = (0..w * h).map(|i| img.rgb[0][i] + img.rgb[1][i] + img.rgb[2][i]).collect();
    let mut sorted: Vec<f32> = lum.iter().step_by(3).copied().collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let thr = sorted[sorted.len() * 3 / 100] * 0.5 + sorted[sorted.len() / 2] * 0.5;
    let inside = |v: f32| if matches!(film, FilmType::Negative | FilmType::BwNegative) { v < thr * 3.0 } else { v > thr * 0.3 };
    let col_frac: Vec<f32> = (0..w).map(|x| (0..h).filter(|&y| inside(lum[y * w + x])).count() as f32 / h as f32).collect();
    let row_frac: Vec<f32> = (0..h).map(|y| (0..w).filter(|&x| inside(lum[y * w + x])).count() as f32 / w as f32).collect();
    let span = |f: &[f32]| -> Option<(usize, usize)> {
        let a = f.iter().position(|&v| v > 0.5)?;
        let b = f.len() - 1 - f.iter().rev().position(|&v| v > 0.5)?;
        (b > a + f.len() / 4).then_some((a, b))
    };
    let (x0, x1) = span(&col_frac)?;
    let (y0, y1) = span(&row_frac)?;
    let m = 2;
    let (x0, y0) = (x0 + m, y0 + m);
    let (x1, y1) = (x1.saturating_sub(m), y1.saturating_sub(m));
    Some([x0 as f32 / w as f32, y0 as f32 / h as f32, (x1 - x0) as f32 / w as f32, (y1 - y0) as f32 / h as f32])
}
