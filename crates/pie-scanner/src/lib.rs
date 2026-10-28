//! Driver for PIE/Reflecta USB slide scanners (Reflecta DigitDia 4000/5000/6000, PIE
//! PowerSlide and relatives), reverse-engineered from CyberView X5 (`DD6000.dll`) and
//! verified against USB captures of CyberView driving a DigitDia 4000.

pub mod error;
pub mod exposure;
pub mod scsi;
pub mod scanner;
pub mod transport;

pub use error::{Error, Result, Sense};
