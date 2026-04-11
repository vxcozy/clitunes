//! Audio source trait and implementations. Slice 1 only has the calibration
//! tone source; Slice 2 adds radio, Slice 4 adds local files.

use crate::audio::PcmRingWriter;

pub mod tone_source;

/// A source writes PCM frames into the ring until it is stopped. Sources
/// run on their own thread in Slice 1 (no async). The control layer starts
/// and stops them.
pub trait Source: Send {
    fn name(&self) -> &str;
    fn run(&mut self, writer: &mut PcmRingWriter, stop: &std::sync::atomic::AtomicBool);
}
