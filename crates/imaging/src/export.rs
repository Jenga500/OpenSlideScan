//! Writing scans to disk.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};
use tiff::encoder::{colortype, Rational, TiffEncoder};
use tiff::tags::ResolutionUnit;

use crate::Image16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Tiff16,
    Tiff8,
    Jpeg,
    Png16,
    /// Lossy JPEG XL (a more efficient JPEG), 16 bits per sample.
    JpegXl,
    /// Lossy JPEG XL, 8 bits per sample.
    JpegXl8,
}

impl Format {
    pub const ALL: [Format; 6] = [Format::Tiff16, Format::Tiff8, Format::Jpeg, Format::Png16, Format::JpegXl, Format::JpegXl8];

    pub fn extension(self) -> &'static str {
        match self {
            Format::Tiff16 | Format::Tiff8 => "tif",
            Format::Jpeg => "jpg",
            Format::Png16 => "png",
            Format::JpegXl | Format::JpegXl8 => "jxl",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Tiff16 => "TIFF 48-bit",
            Format::Tiff8 => "TIFF 24-bit",
            Format::Jpeg => "JPEG",
            Format::Png16 => "PNG 48-bit",
            Format::JpegXl => "JPEG XL 16-bit",
            Format::JpegXl8 => "JPEG XL 8-bit",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Image(#[from] image::ImageError),
    #[error("{0}")]
    Tiff(#[from] tiff::TiffError),
    #[error("{0}")]
    Jxl(String),
}

/// `quality` (1..=100) applies to JPEG and JPEG XL.
pub fn save(img: &Image16, path: &Path, format: Format, quality: u8) -> Result<(), ExportError> {
    let (w, h) = (img.width as u32, img.height as u32);
    let out = BufWriter::new(File::create(path)?);
    match format {
        Format::Tiff16 | Format::Tiff8 => {
            let mut enc = TiffEncoder::new(out)?;
            let dpi = Rational { n: img.dpi.max(1) as u32, d: 1 };
            if format == Format::Tiff16 {
                let mut im = enc.new_image::<colortype::RGB16>(w, h)?;
                im.resolution(ResolutionUnit::Inch, dpi);
                im.write_data(&img.to_rgb16())?;
            } else {
                let mut im = enc.new_image::<colortype::RGB8>(w, h)?;
                im.resolution(ResolutionUnit::Inch, dpi);
                im.write_data(&img.to_rgb8())?;
            }
        }
        Format::Jpeg => {
            JpegEncoder::new_with_quality(out, quality.clamp(1, 100)).write_image(&img.to_rgb8(), w, h, ExtendedColorType::Rgb8)?;
        }
        Format::JpegXl | Format::JpegXl8 => {
            let data = crate::jxl::encode(img, quality.clamp(1, 100), format == Format::JpegXl).map_err(ExportError::Jxl)?;
            let mut out = out;
            std::io::Write::write_all(&mut out, &data)?;
        }
        Format::Png16 => {
            let be: Vec<u8> = img.to_rgb16().iter().flat_map(|v| v.to_be_bytes()).collect();
            PngEncoder::new(out).write_image(&be, w, h, ExtendedColorType::Rgb16)?;
        }
    }
    Ok(())
}

/// Writes an 8-bit PNG (used for on-screen previews).
pub fn save_preview_png(img: &Image16, path: &Path) -> Result<(), ExportError> {
    let out = BufWriter::new(File::create(path)?);
    PngEncoder::new(out).write_image(&img.to_rgb8(), img.width as u32, img.height as u32, ExtendedColorType::Rgb8)?;
    Ok(())
}
