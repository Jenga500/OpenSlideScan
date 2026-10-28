//! SCSI command set of PIE scanners, as issued by CyberView (DD6000.dll) and confirmed in a
//! usbmon capture of CyberView X5 driving a DigitDia 4000.

use std::time::{Duration, Instant};

use crate::error::{Error, Result, Sense};
use crate::transport::{Data, Transport};

pub mod op {
    pub const TEST_UNIT_READY: u8 = 0x00;
    pub const REQUEST_SENSE: u8 = 0x03;
    pub const READ: u8 = 0x08;
    pub const WRITE: u8 = 0x0A;
    pub const PARAM: u8 = 0x0F;
    pub const INQUIRY: u8 = 0x12;
    pub const MODE_SELECT: u8 = 0x15;
    pub const COPY: u8 = 0x18; // CCD mask ("GetControlBitList")
    pub const MODE_SENSE: u8 = 0x1A;
    pub const SCAN: u8 = 0x1B;
    pub const SLIDE: u8 = 0xD1;
    pub const SET_SCAN_HEAD: u8 = 0xD2;
    pub const READ_GAIN_OFFSET: u8 = 0xD7;
    pub const WRITE_GAIN_OFFSET: u8 = 0xDC;
    pub const READ_STATE: u8 = 0xDD;
    /// Lamp on (wake from power-save); CyberView `FUN_10019320`.
    pub const LAMP_ON: u8 = 0xF2;
}

/// Sub-codes of WRITE (0x0A) parameter blocks.
pub mod block {
    pub const SCAN_FRAME: u16 = 0x12;
    pub const EXPOSURE: u16 = 0x13;
    pub const HIGHLIGHT_SHADOW: u16 = 0x14;
    pub const CALIBRATION_INFO: u16 = 0x15;
    pub const CMD_17: u16 = 0x17;
}

/// SCSI status bytes.
const GOOD: u8 = 0x00;
const CHECK_CONDITION: u8 = 0x02;
const BUSY: u8 = 0x08;

pub fn cdb(op: u8, len: u16) -> [u8; 6] {
    [op, 0, 0, (len >> 8) as u8, len as u8, 0]
}

fn le16(v: u16) -> [u8; 2] {
    v.to_le_bytes()
}

fn get16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

pub struct Scsi {
    pub t: Transport,
}

impl Scsi {
    /// One command, translating CHECK CONDITION into its sense data.
    /// Returns `Ok(false)` when the scanner reported BUSY.
    fn once(&self, c: &[u8; 6], data: Data<'_>) -> Result<bool> {
        match self.t.execute(c, data)? {
            GOOD => Ok(true),
            BUSY => Ok(false),
            CHECK_CONDITION => Err(Error::Check(self.request_sense()?)),
            s => Err(Error::Protocol(format!("SCSI status {s:#x} for {c:02x?}"))),
        }
    }

    pub fn request_sense(&self) -> Result<Sense> {
        let mut b = [0u8; 14];
        self.t.execute(&cdb(op::REQUEST_SENSE, 14), Data::In(&mut b))?;
        Ok(Sense::from_bytes(&b))
    }

    /// Runs a command, retrying while the scanner is busy or warming up
    /// (CyberView `FUN_1000e9a0`/`FUN_1000ece0`: 1 s between retries).
    pub fn run(&self, c: &[u8; 6], mut data: Data<'_>) -> Result<()> {
        let start = Instant::now();
        loop {
            let d = match &mut data {
                Data::None => Data::None,
                Data::Out(b) => Data::Out(b),
                Data::In(b) => Data::In(b),
            };
            match self.once(c, d) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(Error::Check(s)) if s.is_becoming_ready() => {}
                Err(e) => return Err(e),
            }
            if start.elapsed() > Duration::from_secs(180) {
                return Err(Error::Timeout);
            }
            std::thread::sleep(Duration::from_millis(1500));
        }
    }

    pub fn test_unit_ready(&self) -> Result<bool> {
        match self.once(&cdb(op::TEST_UNIT_READY, 0), Data::None) {
            Ok(r) => Ok(r),
            Err(Error::Check(s)) if s.is_becoming_ready() => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Polls TEST UNIT READY every 1.5 s, as CyberView does.
    pub fn wait_ready(&self, cancel: &dyn Fn() -> bool) -> Result<()> {
        let start = Instant::now();
        while !self.test_unit_ready()? {
            if cancel() {
                return Err(Error::Cancelled);
            }
            if start.elapsed() > Duration::from_secs(300) {
                return Err(Error::Timeout);
            }
            std::thread::sleep(Duration::from_millis(1500));
        }
        Ok(())
    }

    pub fn inquiry(&self) -> Result<Inquiry> {
        let mut b = [0u8; 0xA4];
        self.run(&cdb(op::INQUIRY, 0xA4), Data::In(&mut b))?;
        Ok(Inquiry::parse(&b))
    }

    pub fn read_state(&self) -> Result<State> {
        let mut b = [0u8; 12];
        self.run(&cdb(op::READ_STATE, 12), Data::In(&mut b))?;
        Ok(State(b))
    }

    fn write_block(&self, data: &[u8]) -> Result<()> {
        self.run(&cdb(op::WRITE, data.len() as u16), Data::Out(data))
    }

    /// Per-channel exposure in percent (100 = calibrated value). Filter: 2=R, 4=G, 8=B.
    pub fn set_exposure(&self, filter: u16, percent: u16) -> Result<()> {
        let mut d = Vec::with_capacity(8);
        d.extend(le16(block::EXPOSURE));
        d.extend(le16(4));
        d.extend(le16(filter));
        d.extend(le16(percent));
        self.write_block(&d)
    }

    pub fn set_highlight_shadow(&self, filter: u16, percent: u16) -> Result<()> {
        let mut d = Vec::with_capacity(8);
        d.extend(le16(block::HIGHLIGHT_SHADOW));
        d.extend(le16(4));
        d.extend(le16(filter));
        d.extend(le16(percent));
        self.write_block(&d)
    }

    pub fn shading_parameters(&self) -> Result<ShadingParams> {
        self.write_block(&[0x95, 0, 0, 0, 0, 0])?;
        let mut b = [0u8; 0x80];
        self.run(&cdb(op::READ, 0x80), Data::In(&mut b))?;
        Ok(ShadingParams { lines: b[11] as u16, width: get16(&b, 12) })
    }

    /// Scan window in units of the maximum (optical) resolution.
    pub fn set_frame(&self, f: Frame) -> Result<()> {
        let mut d = Vec::with_capacity(14);
        for v in [block::SCAN_FRAME, 10, 0x80, f.x0, f.y0, f.x1, f.y1] {
            d.extend(le16(v));
        }
        self.write_block(&d)
    }

    /// CMD_17; the DigitDia 4000 rejects it (05/26/80) and CyberView ignores that.
    pub fn cmd17(&self, value: u16) -> Result<()> {
        let mut d = Vec::with_capacity(6);
        d.extend(le16(block::CMD_17));
        d.extend(le16(2));
        d.extend(le16(value));
        match self.write_block(&d) {
            Err(Error::Check(s)) if s.key == 0x05 => Ok(()),
            r => r,
        }
    }

    pub fn read_gain_offset(&self) -> Result<[u8; 103]> {
        let mut b = [0u8; 103];
        self.run(&cdb(op::READ_GAIN_OFFSET, 103), Data::In(&mut b))?;
        Ok(b)
    }

    pub fn write_gain_offset(&self, g: &GainOffset) -> Result<()> {
        let d = g.encode();
        self.run(&cdb(op::WRITE_GAIN_OFFSET, d.len() as u16), Data::Out(&d))
    }

    pub fn set_mode(&self, m: &Mode) -> Result<()> {
        let d = m.encode();
        self.run(&cdb(op::MODE_SELECT, 16), Data::Out(&d))
    }

    pub fn slide(&self, action: u8, arg: u16) -> Result<()> {
        let d = [action, arg as u8, (arg >> 8) as u8, 0];
        self.run(&cdb(op::SLIDE, 4), Data::Out(&d))
    }

    /// Starts the scan. Does not retry on "lamp warming up": with a slide loaded, repeated
    /// SCAN attempts make the firmware give up on the slide and latch a transport fault.
    /// Switches the lamp on after the scanner put it into power-save (READ STATE byte 8).
    /// The firmware ignites the lamp before answering (> 5 s), so the answer is only read
    /// after `settle`.
    pub fn lamp_on(&self, settle: Duration) -> Result<()> {
        match self.t.execute_settled(&cdb(op::LAMP_ON, 6), Data::None, settle)? {
            GOOD | BUSY => Ok(()),
            CHECK_CONDITION => Err(Error::Check(self.request_sense()?)),
            st => Err(Error::Protocol(format!("SCSI status {st:#x} for LAMP ON"))),
        }
    }

    pub fn start_scan(&self) -> Result<()> {
        loop {
            match self.once(&cdb(op::SCAN, 1), Data::None)? {
                true => return Ok(()),
                false => std::thread::sleep(Duration::from_millis(1000)),
            }
        }
    }

    /// One READ STATE attempt: `Ok(None)` while the scanner is busy or the lamp warms up.
    pub fn poll_state(&self) -> Result<Option<State>> {
        let mut b = [0u8; 12];
        match self.once(&cdb(op::READ_STATE, 12), Data::In(&mut b)) {
            Ok(true) => Ok(Some(State(b))),
            Ok(false) => Ok(None),
            Err(Error::Check(s)) if s.is_becoming_ready() => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn stop_scan(&self) -> Result<()> {
        self.run(&cdb(op::SCAN, 0), Data::None)
    }

    pub fn ccd_mask(&self, width: u16) -> Result<Vec<u8>> {
        let mut b = vec![0u8; width as usize];
        self.run(&cdb(op::COPY, width), Data::In(&mut b))?;
        Ok(b)
    }

    pub fn parameters(&self) -> Result<ScanParams> {
        let mut b = [0u8; 18];
        self.run(&cdb(op::PARAM, 18), Data::In(&mut b))?;
        Ok(ScanParams { width: get16(&b, 0), lines: get16(&b, 2), bytes_per_line: get16(&b, 4) })
    }

    /// READ of `count` lines (shading or image), `bytes` total.
    pub fn read_lines(&self, count: u16, bytes: usize) -> Result<Vec<u8>> {
        let mut b = vec![0u8; bytes];
        self.run(&cdb(op::READ, count), Data::In(&mut b))?;
        Ok(b)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Inquiry {
    pub vendor: String,
    pub product: String,
    pub revision: String,
    pub max_res_x: u16,
    pub max_res_y: u16,
    pub max_width: u16,
    pub max_height: u16,
    pub filters: u8,
    pub depths: u8,
    pub preview_res: u16,
    pub firmware: String,
    pub min_exposure: u16,
    pub max_exposure: u16,
    /// Transparency (slide) area in optical-resolution units.
    pub area: Frame,
    pub model: u16,
    pub production: String,
    pub timestamp: String,
}

impl Inquiry {
    fn parse(b: &[u8]) -> Self {
        let s = |r: std::ops::Range<usize>| String::from_utf8_lossy(&b[r]).trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string();
        Self {
            vendor: s(8..16),
            product: s(16..32),
            revision: s(32..36),
            max_res_x: get16(b, 36),
            max_res_y: get16(b, 38),
            max_width: get16(b, 40),
            max_height: get16(b, 42),
            filters: b[44],
            depths: b[45],
            preview_res: get16(b, 54),
            firmware: s(96..100),
            max_exposure: get16(b, 104),
            min_exposure: get16(b, 106),
            area: Frame { x0: get16(b, 108), y0: get16(b, 110), x1: get16(b, 112), y1: get16(b, 114) },
            model: get16(b, 116),
            production: s(120..124),
            timestamp: s(124..144),
        }
    }

    pub fn has_infrared(&self) -> bool {
        self.filters & 0x10 != 0
    }
}

/// READ STATE (0xDD) reply.
#[derive(Clone, Copy, Debug, Default)]
pub struct State(pub [u8; 12]);

impl State {
    pub fn button_pressed(&self) -> bool {
        self.0[0] != 0
    }
    /// Current magazine position (0 = none selected).
    pub fn position(&self) -> u8 {
        self.0[2]
    }
    pub fn focus(&self) -> u8 {
        self.0[3]
    }
    pub fn busy(&self) -> bool {
        self.0[5] == 1
    }
    /// Magazine transport present and ready (`(state[6] & 7) == 5` in CyberView).
    pub fn magazine_ready(&self) -> bool {
        self.0[6] & 7 == 5
    }
    pub fn scanning(&self) -> bool {
        self.0[6] & 0x40 != 0
    }
    /// The scanner switched the lamp off after ~15-20 min idle (CyberView then sends
    /// LAMP ON and waits before scanning).
    pub fn lamp_off(&self) -> bool {
        self.0[8] == 1
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub x0: u16,
    pub y0: u16,
    pub x1: u16,
    pub y1: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct ShadingParams {
    pub lines: u16,
    pub width: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct ScanParams {
    pub width: u16,
    pub lines: u16,
    pub bytes_per_line: u16,
}

/// Colour passes (MODE SELECT byte 4).
pub mod passes {
    pub const RGB: u8 = 0x80;
    pub const RGBI: u8 = 0x90;
}

/// Colour depth bits (MODE SELECT byte 5).
pub mod depth {
    pub const D8: u8 = 0x04;
    pub const D16: u8 = 0x20;
}

#[derive(Clone, Debug)]
pub struct Mode {
    pub resolution: u16,
    pub passes: u8,
    pub depth: u8,
    /// Skip the shading (white reference) collection; CyberView does this after the first scan.
    pub skip_shading: bool,
    /// MODE byte 12, set by CyberView for final (non-preview) scans.
    pub final_scan: bool,
}

impl Mode {
    pub fn encode(&self) -> [u8; 16] {
        let mut d = [0u8; 16];
        d[1] = 0x0f;
        d[2..4].copy_from_slice(&le16(self.resolution));
        d[4] = self.passes;
        d[5] = self.depth;
        d[6] = 0x04; // indexed lines ("RR"/"GG"/"BB"/"II" prefixed)
        d[8] = 0x01; // Intel byte order
        d[9] = if self.skip_shading { 0x08 } else { 0x00 };
        d[12] = self.final_scan as u8;
        d[13] = 0x80;
        d[14] = 0x10;
        d
    }
}

/// WRITE GAIN/OFFSET block (29 bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct GainOffset {
    pub exposure: [u16; 4],
    pub offset: [u8; 4],
    pub gain: [u8; 4],
    pub light: u8,
}

impl GainOffset {
    /// Values CyberView sent for its first scan of a session on the DigitDia 4000.
    pub const CYBERVIEW_DEFAULT: GainOffset = GainOffset {
        exposure: [0x7ac2, 0x6e78, 0x4daf, 0x177e],
        offset: [0x0d, 0x11, 0x0f, 0x11],
        gain: [0x19, 0x19, 0x19, 0x35],
        light: 6,
    };

    pub fn encode(&self) -> [u8; 29] {
        let mut d = [0u8; 29];
        for c in 0..3 {
            d[c * 2..c * 2 + 2].copy_from_slice(&le16(self.exposure[c]));
            d[6 + c] = self.offset[c];
            d[12 + c] = self.gain[c].min(0x3f);
        }
        d[15] = self.light;
        d[18..20].copy_from_slice(&le16(self.exposure[3]));
        d[20] = self.offset[3];
        d[22] = self.gain[3].min(0x3f);
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_offset_matches_capture() {
        let want = "c27a786eaf4d0d110f000000191919060000" .to_string() + "7e17110035" + "000000000000";
        let got: String = GainOffset::CYBERVIEW_DEFAULT.encode().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn mode_matches_capture() {
        let m = Mode { resolution: 300, passes: passes::RGBI, depth: depth::D8, skip_shading: false, final_scan: false };
        assert_eq!(m.encode().to_vec(), [0x00, 0x0f, 0x2c, 0x01, 0x90, 0x04, 0x04, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x80, 0x10, 0x00]);
        let m = Mode { resolution: 900, passes: passes::RGB, depth: depth::D16, skip_shading: true, final_scan: true };
        assert_eq!(m.encode().to_vec(), [0x00, 0x0f, 0x84, 0x03, 0x80, 0x20, 0x04, 0x00, 0x01, 0x08, 0x00, 0x00, 0x01, 0x80, 0x10, 0x00]);
    }
}
