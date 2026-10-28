//! Minimal file logger: `~/.cache/diascan/diascan.log` (also stderr with `DIASCAN_LOG`).
//! Level from `DIASCAN_LOG` (error/warn/info/debug/trace), default `debug`, which records
//! every SCSI command sent to the scanner.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;

struct Logger {
    file: Mutex<Option<File>>,
    stderr: bool,
    level: log::LevelFilter,
}

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= self.level
    }

    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let line = format!("{t:.3} {:5} {}: {}\n", r.level(), r.target(), r.args());
        if let Some(f) = self.file.lock().unwrap().as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
        if self.stderr {
            eprint!("{line}");
        }
    }

    fn flush(&self) {}
}

pub fn init(cache_dir: &std::path::Path) {
    let env = std::env::var("DIASCAN_LOG").ok();
    let level = env.as_deref().and_then(|v| v.parse().ok()).unwrap_or(log::LevelFilter::Debug);
    let _ = std::fs::create_dir_all(cache_dir);
    let path = cache_dir.join("diascan.log");
    // keep the previous session's log
    let _ = std::fs::rename(&path, cache_dir.join("diascan.log.1"));
    let file = OpenOptions::new().create(true).append(true).open(&path).ok();
    let logger = Logger { file: Mutex::new(file), stderr: env.is_some(), level };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(level);
    }
}
