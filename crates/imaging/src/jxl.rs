//! JPEG XL encoding through the system libjxl (BSD licensed; the C API is identical in
//! libjxl 0.11 and 0.12, so this links against whichever the system or Flatpak runtime has).

use std::ffi::c_void;
use std::os::raw::c_int;

use crate::Image16;

type JxlBool = c_int;
type Status = c_int;
const ENC_SUCCESS: Status = 0;
const ENC_NEED_MORE_OUTPUT: Status = 2;
const TYPE_UINT8: c_int = 2;
const TYPE_UINT16: c_int = 3;
const NATIVE_ENDIAN: c_int = 0;
const FRAME_SETTING_EFFORT: c_int = 0;

#[repr(C)]
struct BasicInfo {
    have_container: JxlBool,
    xsize: u32,
    ysize: u32,
    bits_per_sample: u32,
    exponent_bits_per_sample: u32,
    intensity_target: f32,
    min_nits: f32,
    relative_to_max_display: JxlBool,
    linear_below: f32,
    uses_original_profile: JxlBool,
    have_preview: JxlBool,
    have_animation: JxlBool,
    orientation: c_int,
    num_color_channels: u32,
    num_extra_channels: u32,
    alpha_bits: u32,
    alpha_exponent_bits: u32,
    alpha_premultiplied: JxlBool,
    preview: [u32; 2],
    animation: [u32; 4],
    intrinsic_xsize: u32,
    intrinsic_ysize: u32,
    padding: [u8; 100],
}

#[repr(C)]
struct ColorEncoding {
    color_space: c_int,
    white_point: c_int,
    white_point_xy: [f64; 2],
    primaries: c_int,
    primaries_red_xy: [f64; 2],
    primaries_green_xy: [f64; 2],
    primaries_blue_xy: [f64; 2],
    transfer_function: c_int,
    gamma: f64,
    rendering_intent: c_int,
}

#[repr(C)]
struct PixelFormat {
    num_channels: u32,
    data_type: c_int,
    endianness: c_int,
    align: usize,
}

type ParallelRunner = unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void, *const c_void, u32, u32) -> c_int;

#[link(name = "jxl")]
extern "C" {
    fn JxlEncoderCreate(memory_manager: *const c_void) -> *mut c_void;
    fn JxlEncoderDestroy(enc: *mut c_void);
    fn JxlEncoderSetParallelRunner(enc: *mut c_void, runner: ParallelRunner, opaque: *mut c_void) -> Status;
    fn JxlEncoderInitBasicInfo(info: *mut BasicInfo);
    fn JxlEncoderSetBasicInfo(enc: *mut c_void, info: *const BasicInfo) -> Status;
    fn JxlColorEncodingSetToSRGB(color: *mut ColorEncoding, is_gray: JxlBool);
    fn JxlEncoderSetColorEncoding(enc: *mut c_void, color: *const ColorEncoding) -> Status;
    fn JxlEncoderFrameSettingsCreate(enc: *mut c_void, source: *const c_void) -> *mut c_void;
    fn JxlEncoderSetFrameDistance(settings: *mut c_void, distance: f32) -> Status;
    fn JxlEncoderDistanceFromQuality(quality: f32) -> f32;
    fn JxlEncoderFrameSettingsSetOption(settings: *mut c_void, option: c_int, value: i64) -> Status;
    fn JxlEncoderAddImageFrame(settings: *const c_void, format: *const PixelFormat, buffer: *const c_void, size: usize) -> Status;
    fn JxlEncoderCloseInput(enc: *mut c_void);
    fn JxlEncoderProcessOutput(enc: *mut c_void, next_out: *mut *mut u8, avail_out: *mut usize) -> Status;
}

#[link(name = "jxl_threads")]
extern "C" {
    fn JxlThreadParallelRunner(runner_opaque: *mut c_void, jpegxl_opaque: *mut c_void, init: *const c_void, func: *const c_void, start_range: u32, end_range: u32) -> c_int;
    fn JxlThreadParallelRunnerCreate(memory_manager: *const c_void, num_worker_threads: usize) -> *mut c_void;
    fn JxlThreadParallelRunnerDestroy(runner_opaque: *mut c_void);
    fn JxlThreadParallelRunnerDefaultNumWorkerThreads() -> usize;
}

/// Frees the encoder and thread pool on every exit path.
struct Encoder {
    enc: *mut c_void,
    runner: *mut c_void,
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            JxlEncoderDestroy(self.enc);
            JxlThreadParallelRunnerDestroy(self.runner);
        }
    }
}

fn check(s: Status, what: &str) -> Result<(), String> {
    if s == ENC_SUCCESS { Ok(()) } else { Err(format!("JPEG XL encoder: {what} failed")) }
}

/// Encodes an sRGB image lossily (a more efficient JPEG) with 8 or 16 bits per sample.
/// `quality` uses the libjxl scale, which roughly matches JPEG quality (90 ≈ visually
/// lossless); it is capped at 99 because 100 would switch libjxl to lossless mode.
pub fn encode(img: &Image16, quality: u8, sixteen_bit: bool) -> Result<Vec<u8>, String> {
    let (bytes, bits, data_type) = if sixteen_bit {
        let p = img.to_rgb16();
        (p.iter().flat_map(|v| v.to_ne_bytes()).collect::<Vec<u8>>(), 16, TYPE_UINT16)
    } else {
        (img.to_rgb8(), 8, TYPE_UINT8)
    };
    let quality = quality.clamp(1, 99);
    unsafe {
        let enc = JxlEncoderCreate(std::ptr::null());
        if enc.is_null() {
            return Err("JPEG XL encoder could not be created".into());
        }
        let runner = JxlThreadParallelRunnerCreate(std::ptr::null(), JxlThreadParallelRunnerDefaultNumWorkerThreads());
        let e = Encoder { enc, runner };
        check(JxlEncoderSetParallelRunner(e.enc, std::mem::transmute::<_, ParallelRunner>(JxlThreadParallelRunner as *const ()), e.runner), "threading")?;

        let mut info: BasicInfo = std::mem::zeroed();
        JxlEncoderInitBasicInfo(&mut info);
        info.xsize = img.width as u32;
        info.ysize = img.height as u32;
        info.bits_per_sample = bits;
        info.num_color_channels = 3;
        check(JxlEncoderSetBasicInfo(e.enc, &info), "image header")?;

        let mut color: ColorEncoding = std::mem::zeroed();
        JxlColorEncodingSetToSRGB(&mut color, 0);
        check(JxlEncoderSetColorEncoding(e.enc, &color), "colour profile")?;

        let fs = JxlEncoderFrameSettingsCreate(e.enc, std::ptr::null());
        check(JxlEncoderFrameSettingsSetOption(fs, FRAME_SETTING_EFFORT, 7), "effort")?;
        check(JxlEncoderSetFrameDistance(fs, JxlEncoderDistanceFromQuality(quality as f32)), "quality")?;
        let format = PixelFormat { num_channels: 3, data_type, endianness: NATIVE_ENDIAN, align: 0 };
        check(JxlEncoderAddImageFrame(fs, &format, bytes.as_ptr() as *const c_void, bytes.len()), "adding the image")?;
        JxlEncoderCloseInput(e.enc);

        let mut out = vec![0u8; 1 << 20];
        let mut used = 0usize;
        loop {
            let mut next = out.as_mut_ptr().add(used);
            let mut avail = out.len() - used;
            let s = JxlEncoderProcessOutput(e.enc, &mut next, &mut avail);
            used = out.len() - avail;
            match s {
                ENC_SUCCESS => break,
                ENC_NEED_MORE_OUTPUT => out.resize(out.len() * 2, 0),
                _ => return Err("JPEG XL encoding failed".into()),
            }
        }
        out.truncate(used);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_and_is_smaller_than_raw() {
        let (w, h) = (64, 48);
        let ramp: Vec<f32> = (0..w * h).map(|i| (i % w) as f32 / w as f32).collect();
        let img = Image16 { width: w, height: h, rgb: [ramp.clone(), ramp.clone(), ramp], ir: None, dpi: 300 };
        for (q, sixteen) in [(70, true), (90, true), (100, true), (90, false)] {
            let data = encode(&img, q, sixteen).unwrap();
            assert!(data.len() < w * h * 6 / 4, "q{q}: {} bytes", data.len());
            // bare codestream signature FF 0A or container signature
            assert!(data.starts_with(&[0xff, 0x0a]) || data.starts_with(&[0, 0, 0, 0x0c, b'J', b'X', b'L', b' ']), "q{q}: {:02x?}", &data[..8]);
        }
    }
}
