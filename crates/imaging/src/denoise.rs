//! Noise and stripe removal for scans of dense (dark) slides.
//!
//! With dark slides the signal sits close to the CCD's noise floor: random noise of about
//! 0.17 % of full scale dominates the shadows, and small per-element (column) and per-line
//! (row) offsets of the line sensor show up as streaks. Both are removed here on the linear
//! scan, in sensor orientation (columns = CCD elements, rows = scan lines):
//!
//! * [`destripe`] estimates the column and row offsets robustly from the image itself
//!   (median of the high-pass residual along each column/row) and subtracts them;
//! * [`denoise`] is an à-trous (stationary B3-spline) wavelet shrinkage whose thresholds
//!   follow a noise model measured from the image (noise as a function of brightness), in a
//!   luminance/chrominance space so colour noise can be removed more strongly than detail.

use rayon::prelude::*;

use crate::Image16;

/// How dark the (linear, auto-exposed) scan is, 0 = normal .. 1 = very dense slide.
/// Based on the 90th percentile of luminance in the central area: normal slides measure
/// 0.35-0.5 (dim indoor shots about 0.1 even before auto exposure), a dense night/stage
/// slide about 0.02. Below 0.08 the picture approaches the sensor's noise floor; at 0.03 and
/// below the full correction is used. Normal slides are left untouched.
pub fn darkness(img: &Image16) -> f32 {
    let (w, h) = (img.width, img.height);
    if w < 8 || h < 8 {
        return 0.0;
    }
    let [r, g, b] = &img.rgb;
    let mut y: Vec<f32> = (h / 8..h * 7 / 8)
        .step_by(2)
        .flat_map(|yy| (w / 8..w * 7 / 8).step_by(2).map(move |xx| yy * w + xx))
        .map(|i| (r[i] + 2.0 * g[i] + b[i]) * 0.25)
        .collect();
    let k = y.len() * 9 / 10;
    y.select_nth_unstable_by(k, |a, c| a.partial_cmp(c).unwrap());
    let p90 = y[k].max(1e-5);
    ((0.08f32.ln() - p90.ln()) / (0.08f32.ln() - 0.03f32.ln())).clamp(0.0, 1.0)
}

/// Separable convolution with the B3 spline [1 4 6 4 1]/16, taps spaced `step` apart.
fn b3(src: &[f32], w: usize, h: usize, step: usize) -> Vec<f32> {
    const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let clamp = |i: isize, n: usize| i.clamp(0, n as isize - 1) as usize;
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let s = &src[y * w..(y + 1) * w];
        for x in 0..w {
            let mut acc = 0.0;
            for (k, &c) in K.iter().enumerate() {
                acc += c * s[clamp(x as isize + (k as isize - 2) * step as isize, w)];
            }
            row[x] = acc;
        }
    });
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (k, &c) in K.iter().enumerate() {
            let sy = clamp(y as isize + (k as isize - 2) * step as isize, h);
            let t = &tmp[sy * w..(sy + 1) * w];
            for x in 0..w {
                row[x] += c * t[x];
            }
        }
    });
    out
}

/// Noise model: standard deviation of the noise as a function of signal level, measured
/// from the finest wavelet detail (median absolute deviation per brightness bin).
struct NoiseModel {
    levels: Vec<f32>,
    sigma: Vec<f32>,
}

impl NoiseModel {
    const BINS: usize = 24;

    fn level_bin(level: f32) -> usize {
        // logarithmic bins from 1e-4 to 1 of full scale
        let l = (level.max(1e-4)).log10(); // -4..0
        (((l + 4.0) / 4.0) * (Self::BINS - 1) as f32).round().clamp(0.0, (Self::BINS - 1) as f32) as usize
    }

    fn estimate(detail0: &[f32], level: &[f32]) -> Self {
        let mut buckets: Vec<Vec<f32>> = vec![Vec::new(); Self::BINS];
        for (i, (&d, &l)) in detail0.iter().zip(level).enumerate() {
            if i % 3 == 0 {
                buckets[Self::level_bin(l)].push(d.abs());
            }
        }
        // finest B3 detail of white noise has std 0.890 * sigma
        let mut sigma: Vec<f32> = buckets
            .iter_mut()
            .map(|b| {
                if b.len() < 200 {
                    return f32::NAN;
                }
                let k = b.len() / 2;
                b.select_nth_unstable_by(k, |a, c| a.partial_cmp(c).unwrap());
                1.4826 * b[k] / 0.890
            })
            .collect();
        // fill empty bins from their neighbours
        let known: Vec<(usize, f32)> = sigma.iter().enumerate().filter(|(_, s)| s.is_finite()).map(|(i, &s)| (i, s)).collect();
        if known.is_empty() {
            sigma = vec![0.0; Self::BINS];
        } else {
            for i in 0..Self::BINS {
                if !sigma[i].is_finite() {
                    sigma[i] = known.iter().min_by_key(|(k, _)| k.abs_diff(i)).unwrap().1;
                }
            }
        }
        let levels = (0..Self::BINS).map(|i| 10f32.powf(i as f32 / (Self::BINS - 1) as f32 * 4.0 - 4.0)).collect();
        Self { levels, sigma }
    }

    fn at(&self, level: f32) -> f32 {
        // linear interpolation between bins
        let l = ((level.max(1e-4)).log10() + 4.0) / 4.0 * (Self::BINS - 1) as f32;
        let i = (l.floor() as usize).min(Self::BINS - 2);
        let t = (l - i as f32).clamp(0.0, 1.0);
        let _ = &self.levels;
        self.sigma[i] * (1.0 - t) + self.sigma[i + 1] * t
    }
}

/// Noise std of each B3 à-trous detail level for unit white noise.
const LEVEL_NOISE: [f32; 6] = [0.890, 0.200, 0.086, 0.041, 0.020, 0.010];

/// Wavelet shrinkage of one plane. `level` is the local brightness used for the noise model;
/// `k` scales the thresholds (in noise standard deviations).
fn shrink(plane: &mut [f32], level: &[f32], w: usize, h: usize, k: f32, scales: usize) {
    let mut c = plane.to_vec();
    let mut out = vec![0f32; w * h];
    let mut model: Option<NoiseModel> = None;
    for j in 0..scales {
        let next = b3(&c, w, h, 1 << j);
        let detail: Vec<f32> = c.par_iter().zip(next.par_iter()).map(|(a, b)| a - b).collect();
        if j == 0 {
            model = Some(NoiseModel::estimate(&detail, level));
        }
        let m = model.as_ref().unwrap();
        let wj = LEVEL_NOISE[j.min(LEVEL_NOISE.len() - 1)];
        out.par_iter_mut().zip(detail.par_iter()).zip(level.par_iter()).for_each(|((o, &d), &l)| {
            let t = k * wj * m.at(l);
            *o += d.signum() * (d.abs() - t).max(0.0);
        });
        c = next;
    }
    plane.par_iter_mut().zip(out.par_iter()).zip(c.par_iter()).for_each(|((p, &d), &base)| *p = d + base);
}

/// Removes noise from the linear image. `strength` 0..1 (0 = off).
pub fn denoise(img: &mut Image16, strength: f32) {
    if strength <= 0.0 || img.width < 16 || img.height < 16 {
        return;
    }
    let (w, h) = (img.width, img.height);
    let s = strength.clamp(0.0, 1.0);
    // luminance / chrominance (linear)
    let n = w * h;
    let [r, g, b] = &img.rgb;
    let mut y: Vec<f32> = (0..n).into_par_iter().map(|i| (r[i] + 2.0 * g[i] + b[i]) * 0.25).collect();
    let mut cr: Vec<f32> = (0..n).into_par_iter().map(|i| r[i] - y[i]).collect();
    let mut cb: Vec<f32> = (0..n).into_par_iter().map(|i| b[i] - y[i]).collect();
    let level = b3(&b3(&y, w, h, 1), w, h, 2);
    // Detail: keep edges, remove the noise floor; colour: remove noise much more strongly.
    shrink(&mut y, &level, w, h, 0.5 + 1.0 * s, 3);
    shrink(&mut cr, &level, w, h, 1.0 + 3.0 * s, 5);
    shrink(&mut cb, &level, w, h, 1.0 + 3.0 * s, 5);
    let (rr, gg, bb): (Vec<f32>, Vec<f32>, Vec<f32>) = {
        let rr: Vec<f32> = (0..n).into_par_iter().map(|i| cr[i] + y[i]).collect();
        let bb: Vec<f32> = (0..n).into_par_iter().map(|i| cb[i] + y[i]).collect();
        let gg: Vec<f32> = (0..n).into_par_iter().map(|i| (4.0 * y[i] - rr[i] - bb[i]) * 0.5).collect();
        (rr, gg, bb)
    };
    img.rgb = [rr, gg, bb];
}

/// Residual of each sample against the mean of its neighbours along one axis
/// (excluding itself), or NaN where the two sides differ (an edge in the picture), so that
/// picture edges do not masquerade as sensor offsets.
fn axis_residual(p: &[f32], w: usize, h: usize, horizontal: bool, edge: f32) -> Vec<f32> {
    const R: usize = 6;
    let (n_lines, len) = if horizontal { (h, w) } else { (w, h) };
    let idx = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
    let mut out = vec![f32::NAN; w * h];
    let cols: Vec<Vec<(usize, f32)>> = (0..n_lines)
        .into_par_iter()
        .map(|l| {
            let v: Vec<f32> = (0..len).map(|i| p[idx(l, i)]).collect();
            let mut pre = vec![0f64; len + 1];
            for i in 0..len {
                pre[i + 1] = pre[i] + v[i] as f64;
            }
            let mean = |a: usize, b: usize| ((pre[b] - pre[a]) / (b - a).max(1) as f64) as f32;
            (R..len.saturating_sub(R))
                .filter_map(|i| {
                    let left = mean(i - R, i);
                    let right = mean(i + 1, i + 1 + R);
                    ((left - right).abs() < edge).then(|| (idx(l, i), v[i] - 0.5 * (left + right)))
                })
                .collect()
        })
        .collect();
    for line in cols {
        for (i, r) in line {
            out[i] = r;
        }
    }
    out
}

/// Median of the finite residuals along each line (column if `columns`), 0 if too few.
fn medians(res: &[f32], w: usize, h: usize, columns: bool, limit: f32) -> Vec<f32> {
    let (n, len) = if columns { (w, h) } else { (h, w) };
    (0..n)
        .into_par_iter()
        .map(|l| {
            let mut v: Vec<f32> = (0..len)
                .map(|i| if columns { res[i * w + l] } else { res[l * w + i] })
                .filter(|d| d.is_finite() && d.abs() < limit)
                .collect();
            if v.len() < len / 5 {
                return 0.0;
            }
            let k = v.len() / 2;
            v.select_nth_unstable_by(k, |a, b| a.partial_cmp(b).unwrap());
            v[k]
        })
        .collect()
}

/// Removes column (CCD element) and row (scan line) streaks from one plane.
fn destripe_plane(p: &mut [f32], w: usize, h: usize) {
    // noise level from horizontal first differences
    let mut d: Vec<f32> = p.windows(2).step_by(13).map(|x| (x[1] - x[0]).abs()).collect();
    let k = d.len() / 2;
    d.select_nth_unstable_by(k, |a, b| a.partial_cmp(b).unwrap());
    let sigma = (1.4826 * d[k] / std::f32::consts::SQRT_2).max(1e-6);
    // column offsets: compare each pixel with its left/right neighbours
    let res = axis_residual(p, w, h, true, 3.0 * sigma);
    let col = medians(&res, w, h, true, 4.0 * sigma);
    p.par_chunks_mut(w).for_each(|row| row.iter_mut().zip(&col).for_each(|(v, o)| *v -= o));
    // row offsets: compare with the lines above/below
    let res = axis_residual(p, w, h, false, 3.0 * sigma);
    let row = medians(&res, w, h, false, 4.0 * sigma);
    p.par_chunks_mut(w).zip(row.par_iter()).for_each(|(r, o)| r.iter_mut().for_each(|v| *v -= o));
}

/// Removes line-sensor streaks (in sensor orientation, i.e. before rotation).
pub fn destripe(img: &mut Image16) {
    if img.width < 32 || img.height < 32 {
        return;
    }
    let (w, h) = (img.width, img.height);
    img.rgb.par_iter_mut().for_each(|p| destripe_plane(p, w, h));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng(seed: &mut u64) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        ((*seed >> 11) as f32 / (1u64 << 53) as f32) * 2.0 - 1.0
    }

    #[test]
    fn removes_noise_and_stripes_but_keeps_edges() {
        let (w, h) = (200, 160);
        let mut seed = 0x1234_5678u64;
        let clean: Vec<f32> = (0..w * h).map(|i| if i % w < w / 2 { 0.01 } else { 0.03 }).collect();
        let stripes: Vec<f32> = (0..w).map(|x| if x % 7 == 0 { 0.002 } else { 0.0 }).collect();
        let noisy: Vec<f32> = clean.iter().enumerate().map(|(i, &v)| v + stripes[i % w] + 0.003 * rng(&mut seed)).collect();
        let mut img = Image16 { width: w, height: h, rgb: [noisy.clone(), noisy.clone(), noisy.clone()], ir: None, dpi: 1200 };
        destripe(&mut img);
        denoise(&mut img, 0.6);
        let err = |p: &[f32]| (p.iter().zip(&clean).map(|(a, b)| (a - b) * (a - b)).sum::<f32>() / p.len() as f32).sqrt();
        let before = err(&noisy);
        let after = err(&img.rgb[1]);
        assert!(after < before * 0.5, "rms error {before} -> {after}");
        // the edge between the two halves must stay sharp
        let y = h / 2;
        assert!(img.rgb[1][y * w + w / 2 + 3] - img.rgb[1][y * w + w / 2 - 4] > 0.015);
    }
}
