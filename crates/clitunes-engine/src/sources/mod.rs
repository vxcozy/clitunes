//! Audio source trait and implementations. Slice 1 only has the calibration
//! tone source; Slice 2 adds radio, Slice 4 adds local files.

use crate::audio::ring::PcmWriter;

pub mod tone_source;

#[cfg(feature = "radio")]
pub mod radio;

#[cfg(feature = "decode")]
pub mod symphonia_decode;

#[cfg(feature = "local")]
pub mod local;

pub trait Source: Send {
    fn name(&self) -> &str;
    fn run(&mut self, writer: &mut dyn PcmWriter, stop: &std::sync::atomic::AtomicBool);
}
