//! USB transport for PIE scanners.
//!
//! The scanner is an old parallel-port SCSI design behind a Genesys USB bridge. Every SCSI
//! command is tunnelled through single-byte vendor control writes (bRequest 0x0C) to bridge
//! registers, exactly as CyberView's `CUSBInterface` does through usbscan.sys
//! `IOCTL_WRITE_REGISTERS`/`IOCTL_READ_REGISTERS`:
//!
//! * `0x88`/`0x87`: IEEE-1284 data/control lines. A magic prelude selects the target
//!   (`0xE0` = SCSI command, `0x30` = reset, `0x00` = address).
//! * `0x85`: CDB and short data-out bytes, one byte per transfer.
//! * `0x84`: phase/status byte: `1` = data-in, `0` = data-out, `3` = done; the byte after
//!   `3` is the SCSI status (`0` good, `2` check condition, `8` busy).
//! * `0x82`: 8-byte bulk header (bRequest 0x04) `[dir,0,0,0,len LE32]` before bulk data on
//!   EP 0x81 (in) / EP 0x02 (out), in chunks of at most 0xFFF0 bytes.

use std::time::Duration;

use rusb::{Context, DeviceHandle, UsbContext};

use crate::error::{Error, Result};

pub const VENDOR_ID: u16 = 0x05e3;
/// USB product IDs of PIE/Reflecta scanners that use this protocol (PIEScanner.inf).
pub const PRODUCT_IDS: &[(u16, &str)] = &[
    (0x0142, "Reflecta DigitDia 4000/5000/6000 (PIE MS Scanner)"),
    (0x0143, "PIE PowerSlide 4000"),
    (0x0144, "Reflecta RPS 7200"),
    (0x0145, "Reflecta CrystalScan/ProScan 7200"),
    (0x0146, "Reflecta MF5000"),
];

const REQ_REGISTER: u8 = 0x0C;
const REQ_BUFFER: u8 = 0x04;
const REG_BULK_SIZE: u16 = 0x82;
const REG_STATUS: u16 = 0x84;
const REG_SCSI: u16 = 0x85;
const REG_PAR_CTRL: u16 = 0x87;
const REG_PAR_DATA: u16 = 0x88;

const EP_BULK_IN: u8 = 0x81;
const EP_BULK_OUT: u8 = 0x02;
const BULK_CHUNK: usize = 0xFFF0;

const PRELUDE: [u8; 7] = [0xFF, 0xAA, 0x55, 0x00, 0xFF, 0x87, 0x78];
const IEEE_SCSI: u8 = 0xE0;
const IEEE_RESET: u8 = 0x30;
const IEEE_ADDR: u8 = 0x00;

/// The firmware does not service the bridge while its motors run, so control transfers can
/// legitimately take many seconds to complete. Never cancel them early: an aborted transfer
/// leaves the bridge wedged until the scanner is power-cycled.
const CTRL_TIMEOUT: Duration = Duration::from_secs(120);
const BULK_TIMEOUT: Duration = Duration::from_secs(120);

/// Phase byte values read from register 0x84 after the CDB.
mod phase {
    pub const DATA_OUT: u8 = 0;
    pub const DATA_IN: u8 = 1;
    pub const DONE: u8 = 3;
}

/// Logs control transfers that the scanner held for a long time (or that failed): these are
/// the ones at risk of being aborted by USB stacks with short internal timeouts.
fn slow<T>(what: &str, reg: u16, t: std::time::Instant, r: &rusb::Result<T>) {
    let ms = t.elapsed().as_millis();
    if ms > 1000 || r.is_err() {
        log::warn!("control {what} reg {reg:#04x} took {ms} ms: {}", if r.is_ok() { "ok".to_string() } else { format!("{:?}", r.as_ref().err()) });
    }
}

/// Data phase of a SCSI command.
pub enum Data<'a> {
    None,
    Out(&'a [u8]),
    In(&'a mut [u8]),
}

pub struct Transport {
    handle: DeviceHandle<Context>,
    pub product_id: u16,
}

impl Transport {
    /// Opens the first supported scanner on the bus.
    pub fn open() -> Result<Self> {
        let ctx = Context::new()?;
        let mut candidates = Vec::new();
        for dev in ctx.devices()?.iter() {
            let desc = dev.device_descriptor()?;
            if desc.vendor_id() == VENDOR_ID && PRODUCT_IDS.iter().any(|(p, _)| *p == desc.product_id()) {
                candidates.push((dev, desc.product_id()));
            }
        }
        // After a power-cycle some hosts (e.g. VM USB passthrough) keep a stale entry for the
        // old connection; addresses grow with each enumeration, so try the newest first.
        candidates.sort_by_key(|(d, _)| std::cmp::Reverse((d.bus_number(), d.address())));
        let mut last_err = Error::NotFound;
        for (dev, pid) in candidates {
            let handle = match dev.open() {
                Ok(h) => h,
                // Present on the bus but not accessible: the udev rule is missing.
                Err(rusb::Error::Access) => return Err(Error::PermissionDenied { bus: dev.bus_number(), address: dev.address() }),
                Err(e) => {
                    log::warn!("cannot open scanner at USB {:03}/{:03}: {e}", dev.bus_number(), dev.address());
                    last_err = Error::Usb(e);
                    continue;
                }
            };
            let _ = handle.set_auto_detach_kernel_driver(true);
            if let Err(e) = handle.claim_interface(0) {
                last_err = Error::Usb(e);
                continue;
            }
            log::info!("using scanner at USB {:03}/{:03}", dev.bus_number(), dev.address());
            return Ok(Self { handle, product_id: pid });
        }
        Err(last_err)
    }

    fn write_reg(&self, reg: u16, value: u8) -> Result<()> {
        self.write_reg_idx(reg, 0, value)
    }

    fn write_reg_idx(&self, reg: u16, index: u16, value: u8) -> Result<()> {
        let rt = rusb::request_type(rusb::Direction::Out, rusb::RequestType::Vendor, rusb::Recipient::Device);
        let t = std::time::Instant::now();
        let r = self.handle.write_control(rt, REQ_REGISTER, reg, index, &[value], CTRL_TIMEOUT);
        slow("write", reg, t, &r);
        r?;
        Ok(())
    }

    fn read_reg(&self, reg: u16) -> Result<u8> {
        let rt = rusb::request_type(rusb::Direction::In, rusb::RequestType::Vendor, rusb::Recipient::Device);
        let mut b = [0u8; 1];
        let t = std::time::Instant::now();
        let r = self.handle.read_control(rt, REQ_REGISTER, reg, 0, &mut b, CTRL_TIMEOUT);
        slow("read", reg, t, &r);
        r?;
        Ok(b[0])
    }

    fn bulk_header(&self, len: usize, out: bool) -> Result<()> {
        let rt = rusb::request_type(rusb::Direction::Out, rusb::RequestType::Vendor, rusb::Recipient::Device);
        let n = len as u32;
        let mut hdr = [0u8; 8];
        hdr[0] = out as u8;
        hdr[4..8].copy_from_slice(&n.to_le_bytes());
        self.handle.write_control(rt, REQ_BUFFER, REG_BULK_SIZE, 0, &hdr, CTRL_TIMEOUT)?;
        Ok(())
    }

    fn ieee(&self, command: u8) -> Result<()> {
        for b in PRELUDE {
            self.write_reg(REG_PAR_DATA, b)?;
        }
        self.write_reg(REG_PAR_DATA, command)?;
        std::thread::sleep(Duration::from_millis(3));
        self.write_reg(REG_PAR_CTRL, 0x05)?;
        self.write_reg(REG_PAR_CTRL, 0x04)?;
        self.write_reg(REG_PAR_DATA, 0xFF)
    }

    /// Connect sequence observed from CyberView (Windows usbmon capture).
    pub fn initialize(&self) -> Result<()> {
        self.write_reg(REG_PAR_CTRL, 0x04)?;
        self.ieee(IEEE_RESET)?;
        self.write_reg(REG_PAR_CTRL, 0x04)?;
        self.ieee(IEEE_RESET)?;
        self.ieee(IEEE_ADDR)
    }

    /// Bridge sanity check CyberView performs after INQUIRY.
    pub fn bridge_check(&self) -> Result<()> {
        for _ in 0..2 {
            self.write_reg_idx(0x8c, 0x10, 0x84)?;
            self.read_reg(0x8e)?;
        }
        self.ieee(IEEE_RESET)?;
        self.ieee(IEEE_ADDR)
    }

    /// Recovery used by DD6000.dll (`FUN_1002d6f0`) after a failed transaction.
    pub fn soft_reset(&self) -> Result<()> {
        self.write_reg(REG_PAR_CTRL, 0x04)?;
        self.ieee(IEEE_RESET)
    }

    fn bulk_in(&self, buf: &mut [u8]) -> Result<()> {
        for chunk in buf.chunks_mut(BULK_CHUNK) {
            self.bulk_header(chunk.len(), false)?;
            let mut got = 0;
            while got < chunk.len() {
                got += self.handle.read_bulk(EP_BULK_IN, &mut chunk[got..], BULK_TIMEOUT)?;
            }
        }
        Ok(())
    }

    fn bulk_out(&self, buf: &[u8]) -> Result<()> {
        for chunk in buf.chunks(BULK_CHUNK) {
            self.bulk_header(chunk.len(), true)?;
            let mut sent = 0;
            while sent < chunk.len() {
                sent += self.handle.write_bulk(EP_BULK_OUT, &chunk[sent..], BULK_TIMEOUT)?;
            }
        }
        Ok(())
    }

    /// Executes one SCSI command and returns the SCSI status byte.
    /// Mirrors DD6000.dll `FUN_10030d80`.
    pub fn execute(&self, cdb: &[u8; 6], data: Data<'_>) -> Result<u8> {
        self.execute_settled(cdb, data, Duration::ZERO)
    }

    /// Like [`execute`](Self::execute), but waits `settle` after sending the CDB before
    /// reading the phase byte. For commands the firmware executes before answering (lamp
    /// ignition), so that no control request is left pending meanwhile: some USB stacks
    /// (VM passthrough) abort requests pending for more than 5 s, which wedges the bridge.
    pub fn execute_settled(&self, cdb: &[u8; 6], data: Data<'_>, settle: Duration) -> Result<u8> {
        log::trace!("CDB {cdb:02x?} ->");
        self.ieee(IEEE_SCSI)?;
        for &b in cdb {
            self.write_reg(REG_SCSI, b)?;
        }
        if !settle.is_zero() {
            std::thread::sleep(settle);
        }
        let mut ph = self.read_reg(REG_STATUS)?;
        match (ph, data) {
            (phase::DATA_IN, Data::In(buf)) => {
                self.bulk_in(buf)?;
                ph = self.read_reg(REG_STATUS)?;
            }
            (phase::DATA_OUT, Data::Out(buf)) => {
                if buf.len() < 0x100 {
                    for &b in buf {
                        self.write_reg(REG_SCSI, b)?;
                    }
                } else {
                    self.bulk_out(buf)?;
                }
                ph = self.read_reg(REG_STATUS)?;
            }
            (phase::DATA_OUT, _) => {
                ph = self.read_reg(REG_STATUS)?;
            }
            _ => {}
        }
        if ph != phase::DONE {
            let _ = self.soft_reset();
            return Err(Error::Protocol(format!("unexpected phase {ph} after CDB {cdb:02x?}")));
        }
        let status = self.read_reg(REG_STATUS)?;
        log::debug!("CDB {cdb:02x?} -> status {status}");
        Ok(status)
    }
}
