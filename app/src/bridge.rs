//! QML-facing backend object.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, status)]
        #[qproperty(QString, device_info)]
        #[qproperty(bool, connected)]
        #[qproperty(bool, busy)]
        #[qproperty(bool, infrared)]
        #[qproperty(f64, progress)]
        #[qproperty(i32, position)]
        #[qproperty(QString, preview_url)]
        #[qproperty(QString, resolutions)]
        #[qproperty(QString, saved_files)]
        #[qproperty(QString, last_error)]
        #[qproperty(bool, needs_permission)]
        #[qproperty(QString, permission_error)]
        #[qproperty(QString, permission_help)]
        type Backend = super::BackendRust;

        /// Starts the worker threads and connects to the scanner.
        #[qinvokable]
        fn start(self: Pin<&mut Backend>);
        #[qinvokable]
        fn reconnect(self: Pin<&mut Backend>);
        #[qinvokable]
        fn prescan(self: Pin<&mut Backend>);
        #[qinvokable]
        fn scan(self: Pin<&mut Backend>);
        #[qinvokable]
        fn scan_magazine(self: Pin<&mut Backend>, count: i32);
        #[qinvokable]
        fn move_slides(self: Pin<&mut Backend>, count: i32);
        #[qinvokable]
        fn goto_slide(self: Pin<&mut Backend>, position: i32);
        #[qinvokable]
        fn reload_magazine(self: Pin<&mut Backend>);
        #[qinvokable]
        fn cancel(self: Pin<&mut Backend>);
        /// Opens the desktop's native folder chooser (XDG portal); answers with
        /// `folderChosen`, or `folderPortalUnavailable` if Qt's own dialog must be used.
        #[qinvokable]
        fn browse_folder(self: Pin<&mut Backend>, title: &QString, current: &QString);
        #[qsignal]
        fn folder_chosen(self: Pin<&mut Backend>, path: QString);
        #[qsignal]
        fn folder_portal_unavailable(self: Pin<&mut Backend>);

        /// Installs the udev rule via polkit, then reconnects.
        #[qinvokable]
        fn install_permissions(self: Pin<&mut Backend>);
        /// Sets a numeric option (see `apply_param`).
        #[qinvokable]
        fn set_param(self: Pin<&mut Backend>, name: &QString, value: f64);
        #[qinvokable]
        fn set_text(self: Pin<&mut Backend>, name: &QString, value: &QString);
    }

    impl cxx_qt::Threading for Backend {}
}

use core::pin::Pin;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;

use crate::engine::{film_from_index, format_from_index, Command, Engine, Event};

#[derive(Default)]
pub struct BackendRust {
    status: QString,
    device_info: QString,
    connected: bool,
    busy: bool,
    infrared: bool,
    progress: f64,
    position: i32,
    preview_url: QString,
    resolutions: QString,
    saved_files: QString,
    last_error: QString,
    needs_permission: bool,
    permission_error: QString,
    permission_help: QString,
    engine: Option<Engine>,
}

fn file_url(p: &std::path::Path) -> QString {
    QString::from(&format!("file://{}", p.display()))
}

impl qobject::Backend {
    fn engine(&self) -> Option<&Engine> {
        self.rust().engine.as_ref()
    }

    fn send(&self, c: Command) {
        if let Some(e) = self.engine() {
            e.send(c);
        }
    }

    pub fn start(mut self: Pin<&mut Self>) {
        if self.rust().engine.is_some() {
            return;
        }
        let qt = self.qt_thread();
        let engine = Engine::start(move |ev| {
            let _ = qt.queue(move |mut o| apply_event(o.as_mut(), ev));
        });
        engine.send(Command::Connect);
        self.as_mut().rust_mut().engine = Some(engine);
        self.as_mut().set_permission_help(QString::from(&crate::udev::manual_instructions()));
    }

    pub fn browse_folder(self: Pin<&mut Self>, title: &QString, current: &QString) {
        let qt = self.qt_thread();
        let title = title.to_string();
        let current = std::path::PathBuf::from(current.to_string().trim_start_matches("file://"));
        // The portal call blocks until the dialog closes, so keep it off the GUI thread.
        std::thread::spawn(move || {
            let result = crate::portal::pick_folder(&title, &current);
            let _ = qt.queue(move |mut o| match result {
                crate::portal::Pick::Selected(p) => o.as_mut().folder_chosen(QString::from(&p.to_string_lossy().to_string())),
                crate::portal::Pick::Cancelled => {}
                crate::portal::Pick::Unavailable(e) => {
                    log::warn!("folder portal unavailable ({e}), using Qt dialog");
                    o.as_mut().folder_portal_unavailable();
                }
            });
        });
    }

    pub fn install_permissions(mut self: Pin<&mut Self>) {
        self.as_mut().set_permission_error(QString::default());
        self.send(Command::InstallUdevRules);
    }

    pub fn reconnect(self: Pin<&mut Self>) {
        self.send(Command::Connect);
    }

    pub fn prescan(self: Pin<&mut Self>) {
        self.send(Command::Preview);
    }

    pub fn scan(self: Pin<&mut Self>) {
        self.send(Command::Scan);
    }

    pub fn scan_magazine(self: Pin<&mut Self>, count: i32) {
        self.send(Command::Batch { count: count.max(1) as u32 });
    }

    pub fn move_slides(self: Pin<&mut Self>, count: i32) {
        self.send(Command::Move(count));
    }

    pub fn goto_slide(self: Pin<&mut Self>, position: i32) {
        self.send(Command::Goto(position.clamp(0, 255) as u8));
    }

    pub fn reload_magazine(self: Pin<&mut Self>) {
        self.send(Command::Reload);
    }

    pub fn cancel(self: Pin<&mut Self>) {
        if let Some(e) = self.engine() {
            e.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    pub fn set_param(self: Pin<&mut Self>, name: &QString, value: f64) {
        let Some(e) = self.engine() else { return };
        let name = name.to_string();
        let v = value as f32;
        let rerender = {
            let mut s = e.settings.lock().unwrap();
            let a = &mut s.adjust;
            match name.as_str() {
                "resolution" => { s.resolution = value as u16; false }
                "sixteenBit" => { s.sixteen_bit = value != 0.0; false }
                "format" => { s.format = format_from_index(value as i32); false }
                "jpegQuality" => { s.jpeg_quality = value as u8; false }
                "exposureBias" => { s.exposure_bias = value; false }
                "skipEmpty" => { s.skip_empty = value != 0.0; false }
                "autoCrop" => { s.auto_crop = value != 0.0; true }
                "film" => { a.film = film_from_index(value as i32); true }
                "dust" => { a.dust = v; true }
                "exposure" => { a.exposure = v; true }
                "brightness" => { a.brightness = v; true }
                "contrast" => { a.contrast = v; true }
                "saturation" => { a.saturation = v; true }
                "cyanRed" => { a.balance[0] = v; true }
                "magentaGreen" => { a.balance[1] = v; true }
                "yellowBlue" => { a.balance[2] = v; true }
                "black" => { a.black = v; true }
                "white" => { a.white = v; true }
                "gamma" => { a.gamma = v; true }
                "autoLevels" => { a.auto_levels = value != 0.0; true }
                "autoColor" => { a.auto_color = value != 0.0; true }
                "sharpen" => { a.sharpen = v; true }
                "grain" => { a.grain = v; true }
                "autoDarkDenoise" => { a.auto_dark_denoise = value != 0.0; true }
                "rotation" => { a.rotation = (value as i32).rem_euclid(4) as u8; true }
                "flipH" => { a.flip_h = value != 0.0; true }
                "flipV" => { a.flip_v = value != 0.0; true }
                _ => false,
            }
        };
        if rerender {
            e.rerender();
        }
    }

    pub fn set_text(self: Pin<&mut Self>, name: &QString, value: &QString) {
        let Some(e) = self.engine() else { return };
        let v = value.to_string();
        let mut s = e.settings.lock().unwrap();
        match name.to_string().as_str() {
            "outDir" => s.out_dir = v.trim_start_matches("file://").into(),
            "baseName" => s.base_name = if v.trim().is_empty() { "slide".into() } else { v.trim().to_string() },
            _ => {}
        }
    }
}

fn apply_event(mut o: Pin<&mut qobject::Backend>, ev: Event) {
    match ev {
        Event::Status(s) => o.as_mut().set_status(QString::from(&s)),
        Event::Device { info, resolutions, infrared } => {
            o.as_mut().set_device_info(QString::from(&info));
            let r: Vec<String> = resolutions.iter().map(u16::to_string).collect();
            o.as_mut().set_resolutions(QString::from(&r.join(",")));
            o.as_mut().set_infrared(infrared);
            o.as_mut().set_connected(true);
            o.as_mut().set_needs_permission(false);
        }
        Event::Disconnected => {
            o.as_mut().set_connected(false);
            o.as_mut().set_device_info(QString::from("No scanner connected"));
        }
        Event::Busy(b) => o.as_mut().set_busy(b),
        Event::Progress(p) => o.as_mut().set_progress(p),
        Event::Position(p) => o.as_mut().set_position(p as i32),
        Event::Preview(path) => {
            // a unique query string makes QML reload the file
            let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
            o.as_mut().set_preview_url(QString::from(&format!("{}?{n}", file_url(&path))));
        }
        Event::Saved(path) => {
            let mut list = o.saved_files().to_string();
            if !list.is_empty() {
                list.push('\n');
            }
            list.push_str(&file_url(&path).to_string());
            o.as_mut().set_saved_files(QString::from(&list));
        }
        Event::NeedsPermission => o.as_mut().set_needs_permission(true),
        Event::PermissionInstalled(None) => {
            o.as_mut().set_needs_permission(false);
            o.as_mut().set_permission_error(QString::default());
        }
        Event::PermissionInstalled(Some(e)) => o.as_mut().set_permission_error(QString::from(&e)),
        Event::Error(e) => {
            o.as_mut().set_last_error(QString::from(&e));
            o.as_mut().set_status(QString::from(&format!("Error: {e}")));
        }
    }
}
