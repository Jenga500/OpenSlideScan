//! `darkcmp <raw_rgb.tif> [raw_ir.tif] <out_prefix>`: pipeline with and without the automatic
//! dark-slide correction, plus the measured darkness.
use std::path::Path;

use imaging::adjust::{process, Adjustments};
use imaging::{denoise, export, Image16};

fn load(p: &str) -> (usize, usize, Vec<u16>) {
    let im = image::open(p).expect("open").into_rgb16();
    (im.width() as usize, im.height() as usize, im.into_raw())
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (w, h, px) = load(&a[1]);
    let ch = |v: &[u16], c: usize| v.iter().skip(c).step_by(3).copied().collect::<Vec<u16>>();
    let ir = (a.len() > 3).then(|| ch(&load(&a[2]).2, 0));
    let prefix = a.last().unwrap();
    let img = Image16::from_u16(w, h, &ch(&px, 0), &ch(&px, 1), &ch(&px, 2), ir.as_deref(), 1200);
    println!("darkness {:.2}", denoise::darkness(&img));
    for (name, auto) in [("off", false), ("auto", true)] {
        let t = std::time::Instant::now();
        let out = process(&img, &Adjustments { auto_dark_denoise: auto, ..Default::default() });
        println!("{name}: {:.1}s", t.elapsed().as_secs_f64());
        export::save_preview_png(&out, Path::new(&format!("{prefix}_{name}.png"))).unwrap();
    }
}
