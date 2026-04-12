//! In-process SPSC PCM ring and the `PcmWriter` abstraction that lets
//! sources write to it (or to a tee that fans out to both this ring and
//! the cross-process SPMC ring).

use std::collections::VecDeque;
use std::sync::Arc;

use clitunes_core::{PcmFormat, StereoFrame};
use parking_lot::Mutex;

pub trait PcmWriter: Send {
    fn write(&mut self, frames: &[StereoFrame]) -> usize;
}

#[derive(Clone)]
pub struct PcmRing {
    inner: Arc<Inner>,
}

struct Inner {
    buf: Mutex<VecDeque<StereoFrame>>,
    capacity: usize,
    format: PcmFormat,
}

impl PcmRing {
    pub fn new(format: PcmFormat, capacity_frames: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                buf: Mutex::new(VecDeque::with_capacity(capacity_frames)),
                capacity: capacity_frames,
                format,
            }),
        }
    }

    pub fn format(&self) -> PcmFormat {
        self.inner.format
    }

    pub fn writer(&self) -> PcmRingWriter {
        PcmRingWriter {
            inner: Arc::clone(&self.inner),
        }
    }

    pub fn reader(&self) -> PcmRingReader {
        PcmRingReader {
            inner: Arc::clone(&self.inner),
        }
    }

    pub fn len(&self) -> usize {
        self.inner.buf.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.buf.lock().is_empty()
    }
}

pub struct PcmRingWriter {
    inner: Arc<Inner>,
}

impl PcmRingWriter {
    pub fn write(&mut self, frames: &[StereoFrame]) -> usize {
        let mut buf = self.inner.buf.lock();
        let cap = self.inner.capacity;
        for &f in frames {
            if buf.len() == cap {
                buf.pop_front();
            }
            buf.push_back(f);
        }
        frames.len()
    }
}

impl PcmWriter for PcmRingWriter {
    fn write(&mut self, frames: &[StereoFrame]) -> usize {
        PcmRingWriter::write(self, frames)
    }
}

pub struct PcmRingReader {
    inner: Arc<Inner>,
}

impl PcmRingReader {
    /// Non-destructive snapshot of the most recent `n` frames. Returns fewer
    /// if the ring isn't full. Used by the visualiser FFT tap.
    pub fn snapshot(&self, n: usize) -> Vec<StereoFrame> {
        let buf = self.inner.buf.lock();
        let take = buf.len().min(n);
        let start = buf.len() - take;
        buf.range(start..).copied().collect()
    }

    /// Destructive drain used by the audio output callback. Removes up to
    /// `out.len()` frames from the head of the ring. Returns the number
    /// consumed; fills the remainder with silence.
    pub fn drain_into(&mut self, out: &mut [StereoFrame]) -> usize {
        let mut buf = self.inner.buf.lock();
        let take = buf.len().min(out.len());
        for slot in out.iter_mut().take(take) {
            *slot = buf.pop_front().unwrap_or(StereoFrame::SILENCE);
        }
        for slot in out.iter_mut().skip(take) {
            *slot = StereoFrame::SILENCE;
        }
        take
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_snapshot_matches() {
        let ring = PcmRing::new(PcmFormat::STUDIO, 1024);
        let mut w = ring.writer();
        let frames: Vec<_> = (0..512)
            .map(|i| StereoFrame {
                l: i as f32,
                r: -(i as f32),
            })
            .collect();
        let wrote = w.write(&frames);
        assert_eq!(wrote, 512);
        assert_eq!(ring.len(), 512);

        let snap = ring.reader().snapshot(256);
        assert_eq!(snap.len(), 256);
        assert_eq!(snap[0].l, 256.0);
        assert_eq!(snap[255].l, 511.0);
    }

    #[test]
    fn write_overrun_drops_oldest() {
        let ring = PcmRing::new(PcmFormat::STUDIO, 4);
        let mut w = ring.writer();
        let frames: Vec<_> = (0..8)
            .map(|i| StereoFrame {
                l: i as f32,
                r: 0.0,
            })
            .collect();
        w.write(&frames);
        assert_eq!(ring.len(), 4);

        let snap = ring.reader().snapshot(4);
        assert_eq!(snap[0].l, 4.0);
        assert_eq!(snap[3].l, 7.0);
    }

    #[test]
    fn drain_fills_silence_when_empty() {
        let ring = PcmRing::new(PcmFormat::STUDIO, 16);
        let mut r = ring.reader();
        let mut out = [StereoFrame::default(); 4];
        let got = r.drain_into(&mut out);
        assert_eq!(got, 0);
        assert!(out.iter().all(|f| *f == StereoFrame::SILENCE));
    }
}
