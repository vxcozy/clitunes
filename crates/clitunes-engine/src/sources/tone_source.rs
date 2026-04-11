use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use clitunes_core::{PcmFormat, StereoFrame};

use crate::audio::{CalibrationTone, PcmRingWriter};

use super::Source;

pub struct ToneSource {
    tone: CalibrationTone,
    buf: Vec<StereoFrame>,
}

impl ToneSource {
    pub fn new(format: PcmFormat, buf_frames: usize) -> Self {
        Self {
            tone: CalibrationTone::new(format),
            buf: vec![StereoFrame::SILENCE; buf_frames],
        }
    }
}

impl Source for ToneSource {
    fn name(&self) -> &str {
        "calibration-tone"
    }

    fn run(&mut self, writer: &mut PcmRingWriter, stop: &AtomicBool) {
        let sr = self.tone.format().sample_rate as f32;
        let block_frames = self.buf.len();
        // Each block carries `block_frames / sr` seconds. Sleep a little less
        // so the ring never under-runs.
        let block_dur = Duration::from_secs_f32(block_frames as f32 / sr);
        let sleep_dur = block_dur.mul_f32(0.8);
        while !stop.load(Ordering::Relaxed) {
            self.tone.fill(&mut self.buf);
            writer.write(&self.buf);
            thread::sleep(sleep_dur);
        }
    }
}
