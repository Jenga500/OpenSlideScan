//! High-level scanner operations: the per-scan command sequence CyberView X5 uses on the
//! DigitDia 4000 (taken from a usbmon capture), magazine transport and data decoding.

use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::scsi::{self, depth, passes, Frame, GainOffset, Inquiry, Mode, Scsi, ShadingParams, State};
use crate::transport::Transport;

/// Model numbers from INQUIRY (DD6000.dll `FUN_10018ba0`).
pub mod model {
    pub const DIGITDIA_4000: u16 = 0x2f;
    pub const DIGITDIA_6000: u16 = 0x3a;
    pub const DIGITDIA_5000: u16 = 0x55;
}

/// SLIDE (0xD1) actions.
pub mod slide {
    /// Load the slide at the current magazine position into the scanner (with focus value).
    pub const LOAD: u8 = 0x10;
    pub const NEXT: u8 = 0x04;
    pub const PREVIOUS: u8 = 0x05;
    pub const RELOAD_MAGAZINE: u8 = 0x40;
}

/// After SCAN the bridge NAKs control transfers for several seconds while the slide is
/// pulled in and the lamp settles. A control transfer pending for more than ~5 s gets
/// aborted by some USB stacks (e.g. VM passthrough), which wedges the bridge, so stay off
/// the bus for this long before polling.
const POST_SCAN_QUIET: Duration = Duration::from_secs(15);
/// CyberView sends SCAN 0.5 s after SLIDE LOAD; waiting much longer makes the firmware
/// return the slide to the magazine.
const LOAD_TO_SCAN: Duration = Duration::from_millis(500);
/// Lamp warm-up after power-save; the tray capture shows about 50 s of "warming up".
const LAMP_MAX_WARMUP: Duration = Duration::from_secs(180);
const LAMP_IGNITION_QUIET: Duration = Duration::from_secs(30);
/// Maximum lines per READ (CyberView `FUN_10033430`).
const MAX_BLOCK_LINES: usize = 255;

#[derive(Clone, Debug)]
pub struct ScanRequest {
    pub resolution: u16,
    pub sixteen_bit: bool,
    pub infrared: bool,
    /// Scan window in optical-resolution units; `None` = whole slide area.
    pub frame: Option<Frame>,
    pub gain_offset: GainOffset,
    /// Collect a new white/dark shading reference (first scan of a session or on request).
    pub calibrate: bool,
    /// Preview scans are not marked as final (MODE byte 12).
    pub preview: bool,
}

/// One colour plane: row-major samples normalised to 16 bit.
pub type Plane = Vec<u16>;

#[derive(Clone, Debug)]
pub struct RawScan {
    pub width: usize,
    pub height: usize,
    pub resolution: u16,
    pub red: Plane,
    pub green: Plane,
    pub blue: Plane,
    pub infrared: Option<Plane>,
}

/// White reference collected during calibration, per channel R, G, B, I (16 bit),
/// indexed by CCD element.
#[derive(Clone, Debug, Default)]
pub struct Shading {
    pub width: usize,
    pub reference: [Vec<f32>; 4],
    pub mean: [f32; 4],
}

#[derive(Clone, Debug)]
pub enum Progress {
    Status(String),
    Lines { done: usize, total: usize },
}

pub struct Scanner {
    scsi: Scsi,
    pub inquiry: Inquiry,
    shading_params: ShadingParams,
    shading: Option<Shading>,
    ccd_mask: Vec<u8>,
}

fn never() -> bool {
    false
}

impl Scanner {
    pub fn open() -> Result<Self> {
        let t = Transport::open()?;
        t.initialize()?;
        let scsi = Scsi { t };
        let inquiry = scsi.inquiry()?;
        scsi.t.bridge_check()?;
        scsi.wait_ready(&never)?;
        let shading_params = scsi.shading_parameters()?;
        log::info!("opened {} {} fw {} model {:#x}", inquiry.vendor, inquiry.product, inquiry.firmware, inquiry.model);
        Ok(Self { scsi, inquiry, shading_params, shading: None, ccd_mask: Vec::new() })
    }

    pub fn model_name(&self) -> &'static str {
        match self.inquiry.model {
            model::DIGITDIA_4000 => "Reflecta DigitDia 4000",
            model::DIGITDIA_5000 => "Reflecta DigitDia 5000",
            model::DIGITDIA_6000 => "Reflecta DigitDia 6000",
            _ => "PIE slide scanner",
        }
    }

    /// Resolutions offered by CyberView for this model (DD6000.dll `FUN_10018ba0`).
    pub fn resolutions(&self) -> Vec<u16> {
        match self.inquiry.model {
            model::DIGITDIA_4000 => vec![300, 600, 900, 1200, 1800, 2400, 3600],
            _ => vec![250, 500, 1000, 2000, 2500, 5000],
        }
    }

    /// Focus position sent with SLIDE LOAD. The DigitDia 4000 has fixed focus (0x73).
    pub fn focus(&self) -> u16 {
        if self.inquiry.model == model::DIGITDIA_4000 { 0x73 } else { 0x80 }
    }

    pub fn has_calibration(&self) -> bool {
        self.shading.is_some()
    }

    pub fn state(&self) -> Result<State> {
        self.scsi.read_state()
    }

    /// Moves the magazine by `count` slides (negative = backwards).
    pub fn move_slides(&self, count: i32) -> Result<()> {
        if count == 0 {
            return Ok(());
        }
        let action = if count > 0 { slide::NEXT } else { slide::PREVIOUS };
        self.scsi.slide(action, count.unsigned_abs() as u16)?;
        self.wait_transport()
    }

    pub fn reload_magazine(&self) -> Result<()> {
        self.scsi.slide(slide::RELOAD_MAGAZINE, 0)?;
        self.wait_transport()
    }

    /// Waits until the slide transport is idle. READ STATE returns BUSY while it moves.
    fn wait_transport(&self) -> Result<()> {
        std::thread::sleep(Duration::from_secs(4));
        let start = Instant::now();
        loop {
            let st = self.scsi.read_state()?;
            if !st.busy() {
                return Ok(());
            }
            if start.elapsed() > Duration::from_secs(120) {
                return Err(Error::Timeout);
            }
            std::thread::sleep(Duration::from_millis(1500));
        }
    }

    /// Default scan window: the image area of a framed 24x36 slide, as CyberView crops it.
    pub fn default_frame(&self) -> Frame {
        let full = self.inquiry.max_res_x as f64;
        let s = |v: f64| (v * full / 3600.0) as u16;
        Frame { x0: s(1170.0), y0: s(140.0), x1: s(4360.0), y1: s(5070.0) }
    }

    /// Low-resolution RGB(I) preview of the current slide with CyberView's base exposure.
    pub fn preview(&mut self, infrared: bool, progress: &mut dyn FnMut(Progress), cancel: &dyn Fn() -> bool) -> Result<RawScan> {
        let req = ScanRequest {
            resolution: self.inquiry.preview_res.max(300),
            sixteen_bit: false,
            infrared,
            frame: Some(Frame { x0: 0, y0: 0, x1: self.inquiry.max_width, y1: self.inquiry.max_height }),
            gain_offset: crate::exposure::PREVIEW,
            calibrate: false,
            preview: true,
        };
        self.scan(&req, progress, cancel)
    }

    /// Loads the slide and starts the scan.
    ///
    /// After ~15-20 minutes idle the scanner puts the lamp into power-save (READ STATE byte 8).
    /// Loading a slide then fails: the lamp needs about a minute to warm up, but the transport
    /// gives up on a waiting slide after ~25 s and latches a fault (04/81/82). So, like
    /// CyberView, the lamp is switched on and allowed to warm up *before* loading.
    fn load_and_start(&self, progress: &mut dyn FnMut(Progress), cancel: &dyn Fn() -> bool) -> Result<()> {
        let s = &self.scsi;
        self.ensure_lamp(progress, cancel)?;
        progress(Progress::Status("Loading slide".into()));
        s.slide(slide::LOAD, self.focus())?;
        std::thread::sleep(LOAD_TO_SCAN);
        s.wait_ready(cancel)?;
        s.start_scan()
    }

    /// CyberView `FUN_10019320`: if the lamp is in power-save, wait 2 s, send LAMP ON, then
    /// poll READ STATE every ~3 s while the scanner reports busy (lamp warming up).
    fn ensure_lamp(&self, progress: &mut dyn FnMut(Progress), cancel: &dyn Fn() -> bool) -> Result<()> {
        let s = &self.scsi;
        if !s.read_state()?.lamp_off() {
            return Ok(());
        }
        log::info!("lamp is in power-save: switching it on before loading the slide");
        progress(Progress::Status("Lamp warming up…".into()));
        std::thread::sleep(Duration::from_secs(2));
        let start = Instant::now();
        // The scanner only answers LAMP ON once the lamp has ignited; read the answer late
        // instead of leaving a request pending (see `Transport::execute_settled`).
        s.lamp_on(LAMP_IGNITION_QUIET)?;
        loop {
            std::thread::sleep(Duration::from_millis(3100));
            if cancel() {
                return Err(Error::Cancelled);
            }
            if let Some(st) = s.poll_state()? {
                if !st.busy() && !st.lamp_off() {
                    break;
                }
            }
            if start.elapsed() > LAMP_MAX_WARMUP {
                return Err(Error::Timeout);
            }
        }
        log::info!("lamp ready after {:.0} s", start.elapsed().as_secs_f64());
        std::thread::sleep(Duration::from_secs(2));
        Ok(())
    }

    /// Idle keep-alive (CyberView polls READ STATE continuously while open, which keeps the
    /// scanner from switching its lamp off). `Ok(None)` while busy or warming up.
    pub fn poll_state(&self) -> Result<Option<State>> {
        self.scsi.poll_state()
    }

    /// Scans the slide at the current magazine position.
    pub fn scan(&mut self, req: &ScanRequest, progress: &mut dyn FnMut(Progress), cancel: &dyn Fn() -> bool) -> Result<RawScan> {
        let s = &self.scsi;
        let calibrate = req.calibrate || self.shading.is_none();
        progress(Progress::Status("Preparing scanner".into()));
        s.wait_ready(cancel)?;
        for f in [2, 4, 8] {
            s.set_exposure(f, 100)?;
        }
        for f in [2, 4, 8] {
            s.set_highlight_shadow(f, 100)?;
        }
        let frame = req.frame.unwrap_or(Frame { x0: 0, y0: 0, x1: self.inquiry.max_width, y1: self.inquiry.max_height });
        s.set_frame(frame)?;
        s.cmd17(1)?;
        s.wait_ready(cancel)?;
        let _ = s.read_gain_offset()?;
        s.write_gain_offset(&req.gain_offset)?;
        s.wait_ready(cancel)?;
        let ir = req.infrared && self.inquiry.has_infrared();
        let mode = Mode {
            resolution: req.resolution,
            passes: if ir { passes::RGBI } else { passes::RGB },
            depth: if req.sixteen_bit { depth::D16 } else { depth::D8 },
            skip_shading: !calibrate,
            final_scan: !req.preview,
        };
        s.set_mode(&mode)?;
        s.wait_ready(cancel)?;

        self.load_and_start(progress, cancel)?;
        progress(Progress::Status("Scanning".into()));
        std::thread::sleep(POST_SCAN_QUIET);
        let result = self.receive(req, &mode, calibrate, progress, cancel);
        if result.is_err() {
            let _ = self.scsi.stop_scan();
        }
        let _ = self.wait_transport();
        result
    }

    fn receive(&mut self, req: &ScanRequest, mode: &Mode, calibrate: bool, progress: &mut dyn FnMut(Progress), cancel: &dyn Fn() -> bool) -> Result<RawScan> {
        let s = &self.scsi;
        s.wait_ready(cancel)?;
        let colours = if mode.passes == passes::RGBI { 4 } else { 3 };
        if calibrate {
            progress(Progress::Status("Calibrating".into()));
            let width = self.shading_params.width as usize;
            let line = 2 * width + 2;
            let total = self.shading_params.lines as usize * 4;
            let _ = s.read_gain_offset()?;
            s.write_gain_offset(&req.gain_offset)?;
            let mut raw = s.read_lines(4, 4 * line)?;
            s.wait_ready(cancel)?;
            raw.extend(s.read_lines((total - 8) as u16, (total - 8) * line)?);
            let _ = s.read_gain_offset()?;
            s.write_gain_offset(&req.gain_offset)?;
            raw.extend(s.read_lines(4, 4 * line)?);
            self.shading = Some(compute_shading(&raw, width, total));
        }
        let s = &self.scsi;
        s.wait_ready(cancel)?;
        self.ccd_mask = s.ccd_mask(self.shading_params.width)?;
        let p = s.parameters()?;
        s.wait_ready(cancel)?;
        let width = p.width as usize;
        let height = p.lines as usize;
        let bpl = p.bytes_per_line as usize;
        let total = height * colours;
        let mut planes: [Vec<u16>; 4] = Default::default();
        let mut done = 0;
        while done < total {
            if cancel() {
                return Err(Error::Cancelled);
            }
            let n = (total - done).min(MAX_BLOCK_LINES);
            let block = s.read_lines(n as u16, n * (bpl + 2))?;
            for line in block.chunks_exact(bpl + 2) {
                let idx = match line[0] {
                    b'R' => 0,
                    b'G' => 1,
                    b'B' => 2,
                    b'I' => 3,
                    t => return Err(Error::Protocol(format!("unexpected line tag {t:#x}"))),
                };
                let px = &line[2..];
                if req.sixteen_bit {
                    planes[idx].extend(px.chunks_exact(2).take(width).map(|c| u16::from_le_bytes([c[0], c[1]])));
                } else {
                    planes[idx].extend(px.iter().take(width).map(|&v| (v as u16) << 8 | v as u16));
                }
            }
            done += n;
            progress(Progress::Lines { done, total });
        }
        let [r, g, b, i] = planes;
        let h = [&r, &g, &b].iter().map(|p| p.len() / width.max(1)).min().unwrap_or(0).min(height);
        let crop = |mut p: Vec<u16>| {
            p.truncate(h * width);
            p
        };
        let mut scan = RawScan {
            width,
            height: h,
            resolution: req.resolution,
            red: crop(r),
            green: crop(g),
            blue: crop(b),
            infrared: if colours == 4 { Some(crop(i)) } else { None },
        };
        if let Some(sh) = &self.shading {
            apply_shading(&mut scan, sh, &self.ccd_mask);
        }
        Ok(scan)
    }
}

/// Averages the white-reference lines per channel (pieusb `pieusb_calculate_shading`).
fn compute_shading(raw: &[u8], width: usize, total: usize) -> Shading {
    let line = 2 * width + 2;
    let mut sum = [vec![0f64; width], vec![0f64; width], vec![0f64; width], vec![0f64; width]];
    let mut n = [0usize; 4];
    for l in raw.chunks_exact(line).take(total) {
        let c = match l[0] {
            b'R' => 0,
            b'G' => 1,
            b'B' => 2,
            b'I' => 3,
            _ => continue,
        };
        n[c] += 1;
        for (x, v) in l[2..].chunks_exact(2).enumerate() {
            sum[c][x] += u16::from_le_bytes([v[0], v[1]]) as f64;
        }
    }
    let mut sh = Shading { width, ..Default::default() };
    for c in 0..4 {
        let k = n[c].max(1) as f64;
        sh.reference[c] = sum[c].iter().map(|v| (v / k) as f32).collect();
        sh.mean[c] = sh.reference[c].iter().sum::<f32>() / width.max(1) as f32;
    }
    sh
}

/// Flat-field correction: each output column maps to the CCD element whose mask byte is 0.
fn apply_shading(scan: &mut RawScan, sh: &Shading, mask: &[u8]) {
    let cols: Vec<usize> = mask.iter().enumerate().filter(|(_, &m)| m == 0).map(|(i, _)| i).collect();
    if cols.len() < scan.width {
        return;
    }
    let w = scan.width;
    let fix = |plane: &mut Vec<u16>, c: usize| {
        let gains: Vec<f32> = (0..w).map(|x| {
            let r = sh.reference[c].get(cols[x]).copied().unwrap_or(0.0);
            if r > 64.0 { sh.mean[c] / r } else { 1.0 }
        }).collect();
        for row in plane.chunks_exact_mut(w) {
            for (v, g) in row.iter_mut().zip(&gains) {
                *v = ((*v as f32) * g).round().clamp(0.0, 65535.0) as u16;
            }
        }
    };
    fix(&mut scan.red, 0);
    fix(&mut scan.green, 1);
    fix(&mut scan.blue, 2);
    if let Some(i) = scan.infrared.as_mut() {
        fix(i, 3);
    }
}

pub use scsi::Frame as ScanFrame;
