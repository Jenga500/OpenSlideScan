use std::fmt;

/// SCSI sense data (14 bytes, fixed format).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sense {
    pub key: u8,
    pub asc: u8,
    pub ascq: u8,
}

impl Sense {
    pub fn from_bytes(b: &[u8]) -> Self {
        Self { key: b.get(2).copied().unwrap_or(0) & 0x0f, asc: b.get(12).copied().unwrap_or(0), ascq: b.get(13).copied().unwrap_or(0) }
    }

    /// Lamp warming up / unit becoming ready (02/04/01).
    pub fn is_becoming_ready(&self) -> bool {
        self.key == 0x02 && self.asc == 0x04 && self.ascq == 0x01
    }

    pub fn describe(&self) -> &'static str {
        match (self.key, self.asc, self.ascq) {
            (0x02, 0x04, 0x01) => "lamp warming up",
            (0x05, 0x20, 0x00) => "command not valid in the current state",
            (0x05, 0x26, 0x80) => "command not supported by this model",
            (0x05, 0x26, _) => "invalid parameter",
            (0x06, 0x82, 0x00) => "calibration required",
            (0x09, 0x30, 0x01) => "no slide in the scan position",
            (0x04, 0x81, 0x82) => "slide transport fault (check magazine and slide lift)",
            (0x04, _, _) => "hardware error",
            _ => "scanner error",
        }
    }
}

impl fmt::Display for Sense {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (sense {:02x}/{:02x}/{:02x})", self.describe(), self.key, self.asc, self.ascq)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no supported PIE/Reflecta scanner found on USB")]
    NotFound,
    #[error("no permission to access the scanner (USB {bus:03}/{address:03}); the udev rule is not installed")]
    PermissionDenied { bus: u8, address: u8 },
    #[error("USB error: {0}")]
    Usb(#[from] rusb::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("{0}")]
    Check(Sense),
    #[error("scanner stayed busy too long")]
    Timeout,
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;
