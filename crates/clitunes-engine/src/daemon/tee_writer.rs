use clitunes_core::StereoFrame;

use crate::audio::ring::PcmWriter;
use crate::audio::PcmRingWriter;
use crate::pcm::cross_process_api::PcmProducer;

pub struct TeeWriter {
    ring: PcmRingWriter,
    spmc: Box<dyn PcmProducer>,
}

impl TeeWriter {
    pub fn new(ring: PcmRingWriter, spmc: Box<dyn PcmProducer>) -> Self {
        Self { ring, spmc }
    }
}

impl PcmWriter for TeeWriter {
    fn write(&mut self, frames: &[StereoFrame]) -> usize {
        let n = self.ring.write(frames);
        self.spmc.write_frames(frames);
        n
    }
}
