use std::io;

use clitunes_core::StereoFrame;

use super::spmc_ring::Overrun;

/// 2^16 = 65 536 frames ≈ 680 ms @ 48 kHz. Enough buffer for a 60 fps
/// visualiser that occasionally stalls 1–2 frames.
pub const DEFAULT_CAPACITY: u32 = 1 << 16;

pub trait PcmProducer: Send {
    fn write_frames(&mut self, frames: &[StereoFrame]) -> usize;
    fn written(&self) -> u64;
}

pub trait PcmConsumer: Send {
    fn read_frames(&mut self, buf: &mut [StereoFrame]) -> Result<usize, Overrun>;
    fn cursor(&self) -> u64;
    fn capacity(&self) -> u32;
}

pub trait PcmBridge: Send {
    type Producer: PcmProducer;
    type Consumer: PcmConsumer;

    fn create(capacity_frames: u32, sample_rate: u32) -> io::Result<(Self, Self::Producer)>
    where
        Self: Sized;

    fn open_consumer(name: &str) -> io::Result<(Self, Self::Consumer)>
    where
        Self: Sized;

    fn open_consumer_from_start(name: &str) -> io::Result<(Self, Self::Consumer)>
    where
        Self: Sized;

    fn shm_name(&self) -> &str;
}
