//! `formats <in.tif> <outdir>`: writes a 16-bit TIFF in every export format for comparison.
use std::path::Path;

use imaging::export::{save, Format};
use imaging::Image16;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let img = image::open(&a[1]).expect("open").into_rgb16();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let px = img.into_raw();
    let ch = |c: usize| px.iter().skip(c).step_by(3).copied().collect::<Vec<u16>>();
    let im = Image16::from_u16(w, h, &ch(0), &ch(1), &ch(2), None, 3600);
    for (f, q) in [(Format::Jpeg, 92), (Format::JpegXl, 92), (Format::JpegXl8, 92), (Format::JpegXl, 80)] {
        let p = Path::new(&a[2]).join(format!("{}_q{q}.{}", f.label().replace(' ', "_"), f.extension()));
        let t = std::time::Instant::now();
        save(&im, &p, f, q).unwrap();
        println!("{:8} q{q}: {:>6.1} MB in {:.1}s", f.label(), std::fs::metadata(&p).unwrap().len() as f64 / 1e6, t.elapsed().as_secs_f64());
    }
}
