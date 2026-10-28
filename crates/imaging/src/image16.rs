/// 16-bit RGB image with an optional infrared plane, stored as planar `f32` in 0..1 for
/// processing.
#[derive(Clone, Debug, Default)]
pub struct Image16 {
    pub width: usize,
    pub height: usize,
    /// R, G, B planes, row-major, values 0..=1.
    pub rgb: [Vec<f32>; 3],
    pub ir: Option<Vec<f32>>,
    pub dpi: u16,
}

impl Image16 {
    pub fn from_u16(width: usize, height: usize, r: &[u16], g: &[u16], b: &[u16], ir: Option<&[u16]>, dpi: u16) -> Self {
        let conv = |p: &[u16]| p.iter().map(|&v| v as f32 / 65535.0).collect::<Vec<f32>>();
        Self { width, height, rgb: [conv(r), conv(g), conv(b)], ir: ir.map(conv), dpi }
    }

    pub fn len(&self) -> usize {
        self.width * self.height
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Interleaved RGB, 16 bit.
    pub fn to_rgb16(&self) -> Vec<u16> {
        let mut out = Vec::with_capacity(self.len() * 3);
        for i in 0..self.len() {
            for c in 0..3 {
                out.push((self.rgb[c][i].clamp(0.0, 1.0) * 65535.0 + 0.5) as u16);
            }
        }
        out
    }

    /// Interleaved RGB, 8 bit (sRGB-encoded values are assumed already applied).
    pub fn to_rgb8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len() * 3);
        for i in 0..self.len() {
            for c in 0..3 {
                out.push((self.rgb[c][i].clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
        }
        out
    }

    /// Box-downsampled copy for previews / thumbnails.
    pub fn downscale(&self, max_side: usize) -> Image16 {
        let f = ((self.width.max(self.height) + max_side - 1) / max_side).max(1);
        if f == 1 {
            return self.clone();
        }
        let (w, h) = (self.width / f, self.height / f);
        let shrink = |p: &Vec<f32>| {
            let mut o = vec![0f32; w * h];
            for y in 0..h {
                for x in 0..w {
                    let mut s = 0.0;
                    for dy in 0..f {
                        let row = (y * f + dy) * self.width;
                        for dx in 0..f {
                            s += p[row + x * f + dx];
                        }
                    }
                    o[y * w + x] = s / (f * f) as f32;
                }
            }
            o
        };
        Image16 {
            width: w,
            height: h,
            rgb: [shrink(&self.rgb[0]), shrink(&self.rgb[1]), shrink(&self.rgb[2])],
            ir: self.ir.as_ref().map(shrink),
            dpi: (self.dpi as usize / f) as u16,
        }
    }

    pub fn rotate90(&self, clockwise: bool) -> Image16 {
        let (w, h) = (self.width, self.height);
        let rot = |p: &Vec<f32>| {
            let mut o = vec![0f32; w * h];
            for y in 0..h {
                for x in 0..w {
                    let (nx, ny) = if clockwise { (h - 1 - y, x) } else { (y, w - 1 - x) };
                    o[ny * h + nx] = p[y * w + x];
                }
            }
            o
        };
        Image16 { width: h, height: w, rgb: [rot(&self.rgb[0]), rot(&self.rgb[1]), rot(&self.rgb[2])], ir: self.ir.as_ref().map(rot), dpi: self.dpi }
    }

    pub fn flip(&self, horizontal: bool) -> Image16 {
        let (w, h) = (self.width, self.height);
        let fl = |p: &Vec<f32>| {
            let mut o = vec![0f32; w * h];
            for y in 0..h {
                for x in 0..w {
                    let (sx, sy) = if horizontal { (w - 1 - x, y) } else { (x, h - 1 - y) };
                    o[y * w + x] = p[sy * w + sx];
                }
            }
            o
        };
        Image16 { width: w, height: h, rgb: [fl(&self.rgb[0]), fl(&self.rgb[1]), fl(&self.rgb[2])], ir: self.ir.as_ref().map(fl), dpi: self.dpi }
    }

    pub fn crop(&self, x: usize, y: usize, w: usize, h: usize) -> Image16 {
        let w = w.min(self.width.saturating_sub(x));
        let h = h.min(self.height.saturating_sub(y));
        let cp = |p: &Vec<f32>| {
            let mut o = Vec::with_capacity(w * h);
            for row in y..y + h {
                o.extend_from_slice(&p[row * self.width + x..row * self.width + x + w]);
            }
            o
        };
        Image16 { width: w, height: h, rgb: [cp(&self.rgb[0]), cp(&self.rgb[1]), cp(&self.rgb[2])], ir: self.ir.as_ref().map(cp), dpi: self.dpi }
    }
}
