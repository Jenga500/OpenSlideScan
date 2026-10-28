//! Hardware test tool: `cli info | state | next [n] | prev [n] | reload | scan <dpi> [16] [noir] [out.ppm]`
use std::io::Write;

use pie_scanner::scanner::{Progress, ScanRequest, Scanner};
use pie_scanner::scsi::GainOffset;

fn main() -> pie_scanner::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut sc = Scanner::open()?;
    let i = &sc.inquiry;
    println!("{} | {} {} fw {} model {:#x} | max {} dpi | IR {}", sc.model_name(), i.vendor, i.product, i.firmware, i.model, i.max_res_x, i.has_infrared());
    match args.first().map(String::as_str).unwrap_or("info") {
        "state" => println!("{:02x?}", sc.state()?.0),
        "next" => sc.move_slides(args.get(1).and_then(|v| v.parse().ok()).unwrap_or(1))?,
        "prev" => sc.move_slides(-args.get(1).and_then(|v| v.parse::<i32>().ok()).unwrap_or(1))?,
        "reload" => sc.reload_magazine()?,
        "scan" => {
            let dpi = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(300);
            let req = ScanRequest {
                resolution: dpi,
                sixteen_bit: args.iter().any(|a| a == "16"),
                infrared: !args.iter().any(|a| a == "noir"),
                frame: None,
                gain_offset: GainOffset::CYBERVIEW_DEFAULT,
                calibrate: true,
                preview: false,
            };
            let out = args.iter().find(|a| a.ends_with(".ppm")).cloned().unwrap_or("scan.ppm".into());
            let scan = sc.scan(&req, &mut |p| match p {
                Progress::Status(s) => println!("{s}"),
                Progress::Lines { done, total } => {
                    print!("\r{done}/{total}");
                    let _ = std::io::stdout().flush();
                }
            }, &|| false)?;
            println!("\n{}x{} IR {}", scan.width, scan.height, scan.infrared.is_some());
            let mut f = std::fs::File::create(&out).unwrap();
            write!(f, "P6 {} {} 65535\n", scan.width, scan.height).unwrap();
            let mut buf = Vec::with_capacity(scan.width * scan.height * 6);
            for p in 0..scan.width * scan.height {
                for v in [scan.red[p], scan.green[p], scan.blue[p]] {
                    buf.extend(v.to_be_bytes());
                }
            }
            f.write_all(&buf).unwrap();
            if let Some(ir) = &scan.infrared {
                let mut f = std::fs::File::create(out.replace(".ppm", "_ir.pgm")).unwrap();
                write!(f, "P5 {} {} 65535\n", scan.width, scan.height).unwrap();
                let b: Vec<u8> = ir.iter().flat_map(|v| v.to_be_bytes()).collect();
                f.write_all(&b).unwrap();
            }
        }
        _ => {}
    }
    println!("state {:02x?}", sc.state()?.0);
    Ok(())
}
