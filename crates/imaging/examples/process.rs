//! `process <scan.ppm> <out_prefix>`: runs the pipeline on a 16-bit PPM (+ `_ir.pgm`) dump
//! from the driver CLI, with and without dust removal.
use std::path::Path;

use imaging::adjust::{process, Adjustments};
use imaging::{export, Image16};

fn read_pnm(path: &str) -> (usize, usize, Vec<u16>) {
    let d = std::fs::read(path).unwrap();
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while d[i].is_ascii_whitespace() {
            i += 1;
        }
        let s = i;
        while !d[i].is_ascii_whitespace() {
            i += 1;
        }
        fields.push(String::from_utf8_lossy(&d[s..i]).to_string());
    }
    let (w, h) = (fields[1].parse().unwrap(), fields[2].parse().unwrap());
    let px = d[i + 1..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
    (w, h, px)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (w, h, rgb) = read_pnm(&a[1]);
    let (_, _, ir) = read_pnm(&a[1].replace(".ppm", "_ir.pgm"));
    let r: Vec<u16> = rgb.iter().step_by(3).copied().collect();
    let g: Vec<u16> = rgb.iter().skip(1).step_by(3).copied().collect();
    let b: Vec<u16> = rgb.iter().skip(2).step_by(3).copied().collect();
    let img = Image16::from_u16(w, h, &r, &g, &b, Some(&ir), 300);
    let mask = imaging::dust::defect_mask(&img, &Default::default()).unwrap();
    println!("defect pixels: {}", mask.iter().filter(|&&m| m).count());
    for (name, dust) in [("raw", 0.0), ("clean", 0.6)] {
        let adj = Adjustments { dust, ..Default::default() };
        let out = process(&img, &adj);
        export::save_preview_png(&out, Path::new(&format!("{}_{name}.png", a[2]))).unwrap();
    }
}
