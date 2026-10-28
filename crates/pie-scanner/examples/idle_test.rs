//! Lamp power-save test: `idle_test <seconds> [poll]` keeps the scanner idle for the given
//! time (polling READ STATE every 3 s when `poll` is given, like the app's keep-alive), then
//! takes a preview to see whether the lamp is still on. With `until-lamp-off` it stops idling
//! as soon as the scanner reports lamp power-save, to test the lamp wake-up before loading.
use std::time::{Duration, Instant};

use pie_scanner::scanner::{Progress, Scanner};

struct Stderr;
impl log::Log for Stderr {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
        eprintln!("{t:.3} {} {}", r.level(), r.args());
    }
    fn flush(&self) {}
}

fn main() -> pie_scanner::Result<()> {
    log::set_logger(&Stderr).ok();
    log::set_max_level(log::LevelFilter::Debug);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let secs: u64 = args.first().and_then(|v| v.parse().ok()).unwrap_or(1500);
    let poll = args.iter().any(|a| a == "poll");
    let mut sc = Scanner::open()?;
    println!("opened; idling {secs} s, keep-alive polling: {poll}");
    let start = Instant::now();
    let until_lamp_off = args.iter().any(|a| a == "until-lamp-off");
    while start.elapsed() < Duration::from_secs(secs) {
        std::thread::sleep(Duration::from_secs(3));
        if poll {
            match sc.poll_state()? {
                Some(st) => {
                    if until_lamp_off && st.lamp_off() {
                        println!("{:>5} s lamp went into power-save: {:02x?}", start.elapsed().as_secs(), st.0);
                        break;
                    }
                    if start.elapsed().as_secs() % 300 < 3 {
                        println!("{:>5} s state {:02x?}", start.elapsed().as_secs(), st.0);
                    }
                }
                None => println!("{:>5} s busy / warming", start.elapsed().as_secs()),
            }
        }
    }
    println!("idle done, previewing");
    let t = Instant::now();
    let scan = sc.preview(false, &mut |p| if let Progress::Status(s) = p { println!("  {s}") }, &|| false)?;
    println!("preview ok: {}x{} in {:.0} s", scan.width, scan.height, t.elapsed().as_secs_f64());
    Ok(())
}
