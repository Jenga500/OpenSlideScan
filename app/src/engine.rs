//! Application logic independent of Qt: owns the scanner on a worker thread, renders
//! previews on a second thread and reports state changes through an [`Event`] callback.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;
use std::sync::{Arc, Mutex};

use imaging::adjust::{auto_crop, process, Adjustments};
use imaging::export::{self, Format};
use imaging::film::FilmType;
use imaging::Image16;
use pie_scanner::exposure;
use pie_scanner::scanner::{Progress, RawScan, ScanRequest, Scanner};
use pie_scanner::scsi::{Frame, GainOffset};

#[derive(Clone, Debug)]
pub enum Event {
    Status(String),
    Device { info: String, resolutions: Vec<u16>, infrared: bool },
    Disconnected,
    Busy(bool),
    Progress(f64),
    Position(u8),
    Preview(PathBuf),
    Saved(PathBuf),
    Error(String),
    /// Scanner found but not accessible: the udev rule needs to be installed.
    NeedsPermission,
    /// Result of an udev rule installation attempt (`None` = success).
    PermissionInstalled(Option<String>),
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub resolution: u16,
    pub sixteen_bit: bool,
    pub adjust: Adjustments,
    pub out_dir: PathBuf,
    pub base_name: String,
    pub format: Format,
    pub jpeg_quality: u8,
    pub auto_crop: bool,
    pub exposure_bias: f64,
    pub skip_empty: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let pictures = std::env::var_os("XDG_PICTURES_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Pictures")))
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            resolution: 3600,
            sixteen_bit: true,
            adjust: Adjustments::default(),
            out_dir: pictures.join("Slides"),
            base_name: "slide".into(),
            format: Format::Tiff16,
            jpeg_quality: 92,
            auto_crop: true,
            exposure_bias: 0.0,
            skip_empty: true,
        }
    }
}

pub enum Command {
    Connect,
    Preview,
    Scan,
    Move(i32),
    Goto(u8),
    Reload,
    Batch { count: u32 },
    InstallUdevRules,
}

/// Preview of the slide at one magazine position, kept to derive final-scan settings.
struct PreviewData {
    position: u8,
    raw: RawScan,
    image: Image16,
    gain: GainOffset,
    crop: Option<[f32; 4]>,
}

pub struct Engine {
    commands: Sender<Command>,
    render: Sender<()>,
    pub cancel: Arc<AtomicBool>,
    pub settings: Arc<Mutex<Settings>>,
}

type Emit = Arc<dyn Fn(Event) + Send + Sync>;

/// Idle polling interval (CyberView polls READ STATE every 1-3 s while open).
const KEEPALIVE: Duration = Duration::from_secs(3);

impl Engine {
    pub fn start(emit: impl Fn(Event) + Send + Sync + 'static) -> Self {
        let emit: Emit = Arc::new(emit);
        let (tx, rx) = channel();
        let (rtx, rrx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let settings = Arc::new(Mutex::new(Settings::default()));
        let preview: Arc<Mutex<Option<PreviewData>>> = Arc::new(Mutex::new(None));
        let generation = Arc::new(AtomicU64::new(0));
        {
            let (emit, settings, preview, generation) = (emit.clone(), settings.clone(), preview.clone(), generation.clone());
            std::thread::spawn(move || render_loop(rrx, emit, settings, preview, generation));
        }
        {
            let worker = Worker { emit, settings: settings.clone(), preview, cancel: cancel.clone(), scanner: None, render: rtx.clone(), warming: false, lamp_saving: false };
            std::thread::spawn(move || worker.run(rx));
        }
        Self { commands: tx, render: rtx, cancel, settings }
    }

    pub fn send(&self, c: Command) {
        self.cancel.store(false, Ordering::SeqCst);
        let _ = self.commands.send(c);
    }

    pub fn rerender(&self) {
        let _ = self.render.send(());
    }
}

pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir());
    let d = base.join("diascan");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn to_image(raw: &RawScan) -> Image16 {
    Image16::from_u16(raw.width, raw.height, &raw.red, &raw.green, &raw.blue, raw.infrared.as_deref(), raw.resolution)
}

fn render_loop(rx: Receiver<()>, emit: Emit, settings: Arc<Mutex<Settings>>, preview: Arc<Mutex<Option<PreviewData>>>, generation: Arc<AtomicU64>) {
    while rx.recv().is_ok() {
        while rx.try_recv().is_ok() {} // coalesce bursts of slider changes
        let (img, mut adj, crop) = {
            let p = preview.lock().unwrap();
            let Some(p) = p.as_ref() else { continue };
            (p.image.clone(), settings.lock().unwrap().adjust.clone(), p.crop)
        };
        if adj.crop.is_none() && settings.lock().unwrap().auto_crop {
            adj.crop = crop.map(|c| rotate_crop(c, adj.rotation));
        }
        let out = process(&img, &adj);
        let n = generation.fetch_add(1, Ordering::SeqCst);
        let path = cache_dir().join(format!("preview_{}.png", n % 4));
        match export::save_preview_png(&out, &path) {
            Ok(()) => emit(Event::Preview(path)),
            Err(e) => emit(Event::Error(e.to_string())),
        }
    }
}

/// Converts a crop of the unrotated image into the frame of the rotated one.
fn rotate_crop(c: [f32; 4], quarter_turns: u8) -> [f32; 4] {
    let [mut x, mut y, mut w, mut h] = c;
    for _ in 0..quarter_turns % 4 {
        // clockwise: (x, y) -> (1 - y - h, x)
        (x, y, w, h) = (1.0 - y - h, x, h, w);
    }
    [x, y, w, h]
}

struct Worker {
    emit: Emit,
    settings: Arc<Mutex<Settings>>,
    preview: Arc<Mutex<Option<PreviewData>>>,
    cancel: Arc<AtomicBool>,
    scanner: Option<Scanner>,
    render: Sender<()>,
    warming: bool,
    lamp_saving: bool,
}

impl Worker {
    fn run(mut self, rx: Receiver<Command>) {
        loop {
            let cmd = match rx.recv_timeout(KEEPALIVE) {
                Ok(cmd) => cmd,
                Err(RecvTimeoutError::Timeout) => {
                    self.keep_alive();
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return,
            };
            (self.emit)(Event::Busy(true));
            let result = self.handle(cmd);
            if let Err(e) = result {
                let msg = e.to_string();
                log::error!("{msg}");
                if matches!(e, pie_scanner::Error::Usb(_) | pie_scanner::Error::NotFound | pie_scanner::Error::PermissionDenied { .. }) {
                    self.scanner = None;
                    (self.emit)(Event::Disconnected);
                }
                if matches!(e, pie_scanner::Error::PermissionDenied { .. }) {
                    (self.emit)(Event::NeedsPermission);
                    self.status("The scanner needs a one-time permission setup");
                } else {
                    (self.emit)(Event::Error(msg));
                }
            }
            (self.emit)(Event::Progress(0.0));
            (self.emit)(Event::Busy(false));
        }
    }

    /// Polls the idle scanner like CyberView does; this keeps the lamp from going into
    /// power-save (which otherwise breaks the next slide load) and keeps the UI current.
    fn keep_alive(&mut self) {
        let Some(sc) = &self.scanner else { return };
        match sc.poll_state() {
            Ok(Some(st)) => {
                if self.warming {
                    self.warming = false;
                    self.status("Scanner ready");
                }
                if st.lamp_off() != self.lamp_saving {
                    self.lamp_saving = st.lamp_off();
                    if self.lamp_saving {
                        self.status("Lamp in power-save mode (it warms up automatically before the next scan)");
                    }
                }
                (self.emit)(Event::Position(st.position().saturating_add(1)));
            }
            Ok(None) => {
                if !self.warming {
                    self.warming = true;
                    self.status("Lamp warming up…");
                }
            }
            Err(e) => {
                log::warn!("keep-alive: {e}");
                if matches!(e, pie_scanner::Error::Usb(_)) {
                    self.scanner = None;
                    (self.emit)(Event::Disconnected);
                    self.status("Scanner disconnected");
                }
            }
        }
    }

    fn status(&self, s: impl Into<String>) {
        let s = s.into();
        log::info!("{s}");
        (self.emit)(Event::Status(s));
    }

    fn scanner(&mut self) -> pie_scanner::Result<&mut Scanner> {
        if self.scanner.is_none() {
            self.status("Connecting to scanner…");
            let sc = Scanner::open()?;
            let i = &sc.inquiry;
            let info = format!("{} — firmware {} — {} dpi{}", sc.model_name(), i.firmware, i.max_res_x, if i.has_infrared() { " — infrared" } else { "" });
            (self.emit)(Event::Device { info, resolutions: sc.resolutions(), infrared: i.has_infrared() });
            self.scanner = Some(sc);
            self.report_position();
        }
        Ok(self.scanner.as_mut().unwrap())
    }

    fn report_position(&mut self) {
        if let Some(sc) = &self.scanner {
            if let Ok(st) = sc.state() {
                (self.emit)(Event::Position(st.position().saturating_add(1)));
            }
        }
    }

    fn handle(&mut self, cmd: Command) -> pie_scanner::Result<()> {
        match cmd {
            Command::Connect => {
                self.scanner = None;
                self.scanner()?;
                self.status("Scanner ready");
            }
            Command::Move(n) => {
                self.status(if n > 0 { "Advancing magazine…" } else { "Moving magazine back…" });
                self.scanner()?.move_slides(n)?;
                self.report_position();
                self.status("Ready");
            }
            Command::Goto(target) => {
                // magazine positions are 0-based in the firmware, 1-based in the UI
                let cur = self.scanner()?.state()?.position() as i32 + 1;
                let delta = target as i32 - cur;
                self.status(format!("Moving to slide {target}…"));
                self.scanner()?.move_slides(delta)?;
                self.report_position();
                self.status("Ready");
            }
            Command::Reload => {
                self.status("Reloading magazine…");
                self.scanner()?.reload_magazine()?;
                self.report_position();
                self.status("Magazine reloaded");
            }
            Command::Preview => {
                self.do_preview()?;
                self.status("Preview ready");
            }
            Command::Scan => {
                let path = self.do_scan(None)?;
                self.status(format!("Saved {}", path.display()));
            }
            Command::Batch { count } => self.do_batch(count)?,
            Command::InstallUdevRules => {
                self.status("Installing USB permission rule…");
                match crate::udev::install() {
                    Ok(()) => {
                        log::info!("udev rule installed");
                        (self.emit)(Event::PermissionInstalled(None));
                        // give udev a moment to update the device node's permissions
                        std::thread::sleep(std::time::Duration::from_millis(1500));
                        self.scanner = None;
                        self.scanner()?;
                        self.status("Scanner ready");
                    }
                    Err(e) => {
                        log::error!("udev rule installation failed: {e}");
                        self.status("Permission setup was not completed");
                        (self.emit)(Event::PermissionInstalled(Some(e)));
                    }
                }
            }
        }
        Ok(())
    }

    fn progress_cb(emit: Emit, base: f64, span: f64) -> impl FnMut(Progress) {
        move |p| match p {
            Progress::Status(s) => {
                log::info!("{s}");
                emit(Event::Status(s))
            }
            Progress::Lines { done, total } => emit(Event::Progress(base + span * done as f64 / total.max(1) as f64)),
        }
    }

    fn do_preview(&mut self) -> pie_scanner::Result<bool> {
        let (bias, use_ir) = {
            let s = self.settings.lock().unwrap();
            (s.exposure_bias, s.adjust.dust > 0.0 && s.adjust.film.supports_ir_cleaning())
        };
        let cancel = self.cancel.clone();
        let emit = self.emit.clone();
        let sc = self.scanner()?;
        let raw = sc.preview(use_ir, &mut Self::progress_cb(emit, 0.0, 1.0), &|| cancel.load(Ordering::SeqCst))?;
        let position = sc.state()?.position();
        let empty = exposure::looks_empty(&raw);
        let gain = exposure::from_preview(&raw, bias);
        let image = to_image(&raw);
        let film = self.settings.lock().unwrap().adjust.film;
        let crop = auto_crop(&image, film);
        *self.preview.lock().unwrap() = Some(PreviewData { position, raw, image, gain, crop });
        let _ = self.render.send(());
        (self.emit)(Event::Position(position.saturating_add(1)));
        Ok(empty)
    }

    /// Final scan of the current slide; reuses the preview if it shows the same slide.
    fn do_scan(&mut self, index: Option<u32>) -> pie_scanner::Result<PathBuf> {
        let position = self.scanner()?.state()?.position();
        let fresh = self.preview.lock().unwrap().as_ref().map(|p| p.position == position).unwrap_or(false);
        if !fresh {
            self.do_preview()?;
        }
        let s = self.settings.lock().unwrap().clone();
        let (gain, frame) = {
            let p = self.preview.lock().unwrap();
            let p = p.as_ref().unwrap();
            let sc = self.scanner.as_ref().unwrap();
            let frame = if s.auto_crop { p.crop.and_then(|c| crop_to_frame(c, &p.raw, sc)) } else { None };
            (p.gain.clone(), frame.unwrap_or_else(|| sc.default_frame()))
        };
        let ir = s.adjust.dust > 0.0 && s.adjust.film.supports_ir_cleaning();
        let req = ScanRequest { resolution: s.resolution, sixteen_bit: s.sixteen_bit, infrared: ir, frame: Some(frame), gain_offset: gain, calibrate: false, preview: false };
        let cancel = self.cancel.clone();
        let emit = self.emit.clone();
        let sc = self.scanner()?;
        let raw = sc.scan(&req, &mut Self::progress_cb(emit, 0.0, 0.9), &|| cancel.load(Ordering::SeqCst))?;
        let img = to_image(&raw);
        let dark = imaging::denoise::darkness(&img);
        if s.adjust.auto_dark_denoise && dark > 0.0 {
            log::info!("dark slide detected (darkness {dark:.2})");
            self.status("Dark slide detected: removing sensor noise and streaks…");
        } else {
            self.status("Processing image (dust removal, colour)…");
        }
        // DIASCAN_SAVE_RAW=<dir>: also keep the linear, shading-corrected scan (diagnostics)
        if let Some(dir) = std::env::var_os("DIASCAN_SAVE_RAW") {
            let dir = PathBuf::from(dir);
            let _ = std::fs::create_dir_all(&dir);
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let _ = export::save(&img, &dir.join(format!("raw_{stamp}_rgb.tif")), Format::Tiff16, 100);
            if let Some(ir) = &img.ir {
                let gray = Image16 { width: img.width, height: img.height, rgb: [ir.clone(), ir.clone(), ir.clone()], ir: None, dpi: img.dpi };
                let _ = export::save(&gray, &dir.join(format!("raw_{stamp}_ir.tif")), Format::Tiff16, 100);
            }
            log::info!("saved raw scan to {}", dir.display());
        }
        let mut adj = s.adjust.clone();
        adj.crop = None; // the scan frame already is the crop
        let out = process(&img, &adj);
        let _ = std::fs::create_dir_all(&s.out_dir);
        let n = index.unwrap_or(position as u32 + 1);
        let path = unique_path(&s.out_dir, &s.base_name, n, s.format.extension());
        export::save(&out, &path, s.format, s.jpeg_quality).map_err(|e| pie_scanner::Error::Protocol(format!("saving {}: {e}", path.display())))?;
        (self.emit)(Event::Progress(1.0));
        (self.emit)(Event::Saved(path.clone()));
        Ok(path)
    }

    fn do_batch(&mut self, count: u32) -> pie_scanner::Result<()> {
        let skip_empty = self.settings.lock().unwrap().skip_empty;
        let mut empty_run = 0;
        for i in 0..count {
            if self.cancel.load(Ordering::SeqCst) {
                return Err(pie_scanner::Error::Cancelled);
            }
            let pos = self.scanner()?.state()?.position() as u32 + 1;
            self.status(format!("Magazine: slide {} ({}/{count}) — preview", pos, i + 1));
            let empty = self.do_preview()?;
            if empty && skip_empty {
                empty_run += 1;
                self.status(format!("Slot {pos} is empty — skipped"));
                if empty_run >= 3 {
                    self.status("Three empty slots in a row: end of magazine");
                    return Ok(());
                }
            } else {
                empty_run = 0;
                self.status(format!("Magazine: slide {} ({}/{count}) — scanning", pos, i + 1));
                self.do_scan(None)?;
            }
            if i + 1 < count {
                self.scanner()?.move_slides(1)?;
                self.report_position();
            }
        }
        self.status(format!("Magazine scan finished ({count} slots)"));
        Ok(())
    }
}

/// Maps a relative crop of the full-area preview into optical-resolution scan coordinates.
fn crop_to_frame(c: [f32; 4], raw: &RawScan, sc: &Scanner) -> Option<Frame> {
    let scale = sc.inquiry.max_res_x as f32 / raw.resolution as f32;
    let (pw, ph) = (raw.width as f32 * scale, raw.height as f32 * scale);
    let f = Frame { x0: (c[0] * pw) as u16, y0: (c[1] * ph) as u16, x1: ((c[0] + c[2]) * pw) as u16, y1: ((c[1] + c[3]) * ph) as u16 };
    (f.x1 > f.x0 + 500 && f.y1 > f.y0 + 500).then_some(f)
}

fn unique_path(dir: &Path, base: &str, n: u32, ext: &str) -> PathBuf {
    let mut p = dir.join(format!("{base}_{n:03}.{ext}"));
    let mut k = 1;
    while p.exists() {
        p = dir.join(format!("{base}_{n:03}-{k}.{ext}"));
        k += 1;
    }
    p
}

pub fn film_from_index(i: i32) -> FilmType {
    FilmType::ALL.get(i as usize).copied().unwrap_or_default()
}

pub fn format_from_index(i: i32) -> Format {
    Format::ALL.get(i as usize).copied().unwrap_or(Format::Tiff16)
}
