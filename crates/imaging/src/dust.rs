//! Infrared dust and scratch removal (the technique behind Digital ICE / Magic Touch).
//!
//! Colour dyes in slide film are transparent to near-infrared light, while dust, hairs and
//! scratches block it. The IR channel therefore shows only defects. Steps:
//! 1. Compensate the red-dye crosstalk into the IR channel (IR ~ a + b * R fitted by least
//!    squares on clean pixels).
//! 2. Normalise IR by its local (blurred) level so lamp falloff does not matter.
//! 3. Threshold the attenuation into a defect mask and dilate it to cover soft edges.
//! 4. Inpaint masked pixels from the surrounding clean pixels by iterative diffusion
//!    seeded with a coarse fill (multi-scale), which handles long scratches well.
//!
//! Kodachrome and silver-based B&W film absorb IR, so this must be disabled for them.

use rayon::prelude::*;

use crate::Image16;

#[derive(Clone, Copy, Debug)]
pub struct DustOptions {
    /// 0.0 (off) .. 1.0 (aggressive). Maps to the attenuation threshold.
    pub strength: f32,
    /// Extra pixels around each defect to replace.
    pub grow: usize,
}

impl Default for DustOptions {
    fn default() -> Self {
        Self { strength: 0.5, grow: 2 }
    }
}

fn box_blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    // Two separable passes with a running sum.
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let s = &src[y * w..(y + 1) * w];
        let mut acc = 0f32;
        let mut n = 0f32;
        for x in 0..=r.min(w - 1) {
            acc += s[x];
            n += 1.0;
        }
        for x in 0..w {
            row[x] = acc / n;
            if x + r + 1 < w {
                acc += s[x + r + 1];
                n += 1.0;
            }
            if x >= r {
                acc -= s[x - r];
                n -= 1.0;
            }
        }
    });
    let mut out = vec![0f32; w * h];
    let cols: Vec<Vec<f32>> = (0..w)
        .into_par_iter()
        .map(|x| {
            let mut col = vec![0f32; h];
            let mut acc = 0f32;
            let mut n = 0f32;
            for y in 0..=r.min(h - 1) {
                acc += tmp[y * w + x];
                n += 1.0;
            }
            for y in 0..h {
                col[y] = acc / n;
                if y + r + 1 < h {
                    acc += tmp[(y + r + 1) * w + x];
                    n += 1.0;
                }
                if y >= r {
                    acc -= tmp[(y - r) * w + x];
                    n -= 1.0;
                }
            }
            col
        })
        .collect();
    for (x, col) in cols.iter().enumerate() {
        for y in 0..h {
            out[y * w + x] = col[y];
        }
    }
    out
}

/// Separable running max (`max = true`) or min filter with radius `r`.
fn rank_filter(src: &[f32], w: usize, h: usize, r: usize, max: bool) -> Vec<f32> {
    let pick = |a: f32, b: f32| if max { a.max(b) } else { a.min(b) };
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let s = &src[y * w..(y + 1) * w];
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            row[x] = s[a..=b].iter().copied().reduce(pick).unwrap();
        }
    });
    let cols: Vec<Vec<f32>> = (0..w)
        .into_par_iter()
        .map(|x| (0..h).map(|y| {
            let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
            (a..=b).map(|yy| tmp[yy * w + x]).reduce(pick).unwrap()
        }).collect())
        .collect();
    let mut out = vec![0f32; w * h];
    for (x, col) in cols.iter().enumerate() {
        for y in 0..h {
            out[y * w + x] = col[y];
        }
    }
    out
}

/// Returns the defect mask (true = dust/scratch).
pub fn defect_mask(img: &Image16, opts: &DustOptions) -> Option<Vec<bool>> {
    let ir = img.ir.as_ref()?;
    let (w, h) = (img.width, img.height);
    if w < 8 || h < 8 {
        return None;
    }
    let red = &img.rgb[0];

    // Film area: where the (smoothed) IR transmission is high. The slide mount is opaque in
    // IR as well and must not be treated as a defect; stay a margin away from its edge.
    let coarse = box_blur(ir, w, h, (w.max(h) / 60).max(4));
    let mut lv: Vec<f32> = coarse.iter().step_by(11).copied().collect();
    lv.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let film_level = lv[lv.len() * 9 / 10];
    let margin = (w.max(h) / 80).max(3);
    let film = box_blur(&coarse.iter().map(|&v| if v > 0.6 * film_level { 1.0 } else { 0.0 }).collect::<Vec<f32>>(), w, h, margin);
    let in_film: Vec<bool> = film.iter().map(|&f| f > 0.999).collect();

    // Red-dye crosstalk: IR = a + b*R, fitted on film pixels only.
    let (mut sx, mut sy, mut sxx, mut sxy, mut n) = (0f64, 0f64, 0f64, 0f64, 0f64);
    for i in (0..w * h).step_by(5).filter(|&i| in_film[i]) {
        let (x, y) = (red[i] as f64, ir[i] as f64);
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
        n += 1.0;
    }
    if n < 100.0 {
        return None;
    }
    let b = ((n * sxy - sx * sy) / (n * sxx - sx * sx).max(1e-9)).clamp(0.0, 0.6) as f32;
    let mean_r = (sx / n) as f32;
    let corrected: Vec<f32> = ir.par_iter().zip(red.par_iter()).map(|(&i, &r)| i - b * (r - mean_r)).collect();

    // Clean background: morphological closing removes dark specks narrower than the radius.
    let r = (img.dpi as usize / 150).clamp(2, 32);
    let bg = rank_filter(&rank_filter(&corrected, w, h, r, true), w, h, r, false);

    // A defect attenuates IR by at least `t` relative to the background.
    let t = 0.45 - 0.35 * opts.strength.clamp(0.0, 1.0);
    let mut mask: Vec<bool> = (0..w * h).map(|i| in_film[i] && bg[i] > 1e-3 && (bg[i] - corrected[i]) / bg[i] > t).collect();
    if std::env::var("DUST_DEBUG").is_ok() {
        eprintln!("crosstalk b={b:.3} radius={r} t={t:.2} film_px={} defects={}", in_film.iter().filter(|&&f| f).count(), mask.iter().filter(|&&m| m).count());
    }

    // dilate to cover the soft edges of each defect
    for _ in 0..opts.grow {
        let src = mask.clone();
        for y in 0..h {
            for x in 0..w {
                if src[y * w + x] {
                    continue;
                }
                let near = (x > 0 && src[y * w + x - 1]) || (x + 1 < w && src[y * w + x + 1]) || (y > 0 && src[(y - 1) * w + x]) || (y + 1 < h && src[(y + 1) * w + x]);
                if near {
                    mask[y * w + x] = true;
                }
            }
        }
    }
    Some(mask)
}

/// Fills masked pixels of one plane from their clean neighbourhood.
pub fn inpaint(plane: &mut [f32], mask: &[bool], w: usize, h: usize) {
    if !mask.iter().any(|&m| m) {
        return;
    }
    // Coarse seed: blur of clean pixels only (normalised convolution) at a few scales.
    let clean: Vec<f32> = plane.iter().zip(mask).map(|(&v, &m)| if m { 0.0 } else { v }).collect();
    let weight: Vec<f32> = mask.iter().map(|&m| if m { 0.0 } else { 1.0 }).collect();
    for r in [2usize, 6, 18, 54] {
        let bv = box_blur(&clean, w, h, r);
        let bw = box_blur(&weight, w, h, r);
        for i in 0..w * h {
            if mask[i] && bw[i] > 0.05 && plane[i] < -0.5 {
                plane[i] = bv[i] / bw[i];
            }
        }
        // mark the first scale that reaches a pixel by using a sentinel
        if r == 2 {
            for i in 0..w * h {
                if mask[i] {
                    plane[i] = if bw[i] > 0.05 { bv[i] / bw[i] } else { -1.0 };
                }
            }
        }
    }
    for v in plane.iter_mut() {
        if *v < 0.0 {
            *v = 0.5;
        }
    }
    // Diffusion refinement (Gauss-Seidel on the Laplace equation) inside the mask.
    let idx: Vec<usize> = (0..w * h).filter(|&i| mask[i]).collect();
    for _ in 0..40 {
        for &i in &idx {
            let (x, y) = (i % w, i / w);
            let mut s = 0.0;
            let mut n = 0.0;
            if x > 0 {
                s += plane[i - 1];
                n += 1.0;
            }
            if x + 1 < w {
                s += plane[i + 1];
                n += 1.0;
            }
            if y > 0 {
                s += plane[i - w];
                n += 1.0;
            }
            if y + 1 < h {
                s += plane[i + w];
                n += 1.0;
            }
            plane[i] = s / n;
        }
    }
}

/// Removes dust and scratches in place. Returns the number of corrected pixels.
pub fn remove_dust(img: &mut Image16, opts: &DustOptions) -> usize {
    if opts.strength <= 0.0 {
        return 0;
    }
    let Some(mask) = defect_mask(img, opts) else { return 0 };
    let (w, h) = (img.width, img.height);
    img.rgb.par_iter_mut().for_each(|p| inpaint(p, &mask, w, h));
    mask.iter().filter(|&&m| m).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_synthetic_scratch() {
        let (w, h) = (128, 96);
        let mut img = Image16 { width: w, height: h, rgb: [vec![0.6; w * h], vec![0.4; w * h], vec![0.3; w * h]], ir: Some(vec![0.8; w * h]), dpi: 300 };
        for x in 10..110 {
            let i = 40 * w + x;
            for c in 0..3 {
                img.rgb[c][i] = 0.05;
            }
            img.ir.as_mut().unwrap()[i] = 0.1;
        }
        let n = remove_dust(&mut img, &DustOptions::default());
        assert!(n >= 100, "masked {n}");
        assert!((img.rgb[0][40 * w + 60] - 0.6).abs() < 0.02);
    }
}
