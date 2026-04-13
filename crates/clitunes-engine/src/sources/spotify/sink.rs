//! Bridge between librespot's `Sink` trait and clitunes' `PcmWriter` trait.
//!
//! [`SpotifySink`] receives decoded PCM from librespot at 44100 Hz,
//! resamples to 48000 Hz via rubato, and pushes `StereoFrame` slices
//! through an `mpsc::SyncSender` to the blocking source thread.

use std::sync::mpsc::SyncSender;

use librespot_playback::audio_backend::{Open, Sink, SinkError, SinkResult};
use librespot_playback::config::AudioFormat as LibrespotAudioFormat;
use librespot_playback::convert::Converter;
use librespot_playback::decoder::AudioPacket;
use rubato::audioadapter::Adapter;
use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{
    Async, FixedAsync, Indexing, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

use clitunes_core::StereoFrame;

/// Default chunk size for the fixed-input resampler.
///
/// 1024 frames is large enough to amortise per-call overhead while keeping
/// latency well under one video frame (~21 ms at 48 kHz).
const DEFAULT_CHUNK_SIZE: usize = 1024;

/// Bounded channel capacity for back-pressure between the sink and the source
/// thread. When full, `SyncSender::send` blocks, preventing OOM.
const CHANNEL_CAPACITY: usize = 32;

/// Creates a new `(SpotifySink, std::sync::mpsc::Receiver<Vec<StereoFrame>>)` pair.
///
/// The receiver should be consumed by the source thread that feeds the daemon
/// audio pipeline. The sink is handed to librespot's player.
pub fn channel() -> (SpotifySink, std::sync::mpsc::Receiver<Vec<StereoFrame>>) {
    let (tx, rx) = std::sync::mpsc::sync_channel(CHANNEL_CAPACITY);
    let mut sink = SpotifySink::new();
    sink.set_sender(tx);
    (sink, rx)
}

pub struct SpotifySink {
    tx: Option<SyncSender<Vec<StereoFrame>>>,
    resampler: Option<Async<f64>>,
    accum_l: Vec<f64>,
    accum_r: Vec<f64>,
    chunk_size: usize,
}

impl SpotifySink {
    fn new() -> Self {
        Self {
            tx: None,
            resampler: None,
            accum_l: Vec::new(),
            accum_r: Vec::new(),
            chunk_size: DEFAULT_CHUNK_SIZE,
        }
    }

    /// Inject the bounded sender before playback starts.
    pub fn set_sender(&mut self, tx: SyncSender<Vec<StereoFrame>>) {
        self.tx = Some(tx);
    }

    /// Build the rubato `Async` sinc resampler (44100 -> 48000 Hz, stereo).
    fn build_resampler(chunk_size: usize) -> Result<Async<f64>, SinkError> {
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };
        Async::<f64>::new_sinc(
            48_000.0 / 44_100.0,
            2.0,
            &params,
            chunk_size,
            2,
            FixedAsync::Input,
        )
        .map_err(|e| SinkError::InvalidParams(format!("rubato init failed: {e}")))
    }

    /// Push one complete chunk (of exactly `chunk_size` frames per channel)
    /// through the resampler and send the resulting `StereoFrame`s.
    fn resample_and_send(&mut self, left: &[f64], right: &[f64]) -> SinkResult<()> {
        let resampler = self
            .resampler
            .as_mut()
            .ok_or_else(|| SinkError::NotConnected("resampler not initialised".into()))?;
        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| SinkError::NotConnected("sender not set".into()))?;

        let channels: Vec<Vec<f64>> = vec![left.to_vec(), right.to_vec()];
        let input = SequentialSliceOfVecs::new(channels.as_slice(), 2, left.len())
            .map_err(|e| SinkError::OnWrite(format!("audioadapter wrap failed: {e}")))?;

        let output = resampler
            .process(&input, 0, None)
            .map_err(|e| SinkError::OnWrite(format!("rubato resample failed: {e}")))?;

        let out_frames = output.frames();
        let mut frames = Vec::with_capacity(out_frames);
        for i in 0..out_frames {
            let l = output.read_sample(0, i).unwrap_or(0.0) as f32;
            let r = output.read_sample(1, i).unwrap_or(0.0) as f32;
            frames.push(StereoFrame { l, r });
        }

        if !frames.is_empty() {
            tx.send(frames)
                .map_err(|_| SinkError::NotConnected("receiver dropped".into()))?;
        }

        Ok(())
    }

    /// Flush a partial chunk (fewer than `chunk_size` frames) through the
    /// resampler using `partial_len` in the `Indexing` struct, then send.
    fn flush_partial(&mut self) -> SinkResult<()> {
        let remaining = self.accum_l.len();
        if remaining == 0 {
            return Ok(());
        }

        let resampler = self
            .resampler
            .as_mut()
            .ok_or_else(|| SinkError::NotConnected("resampler not initialised".into()))?;
        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| SinkError::NotConnected("sender not set".into()))?;

        // Pad accumulation buffers to chunk_size so the audioadapter wrapper
        // has enough frames. rubato will only read `remaining` via partial_len.
        let chunk_size = self.chunk_size;
        let mut left = std::mem::take(&mut self.accum_l);
        let mut right = std::mem::take(&mut self.accum_r);
        left.resize(chunk_size, 0.0);
        right.resize(chunk_size, 0.0);

        let channels: Vec<Vec<f64>> = vec![left, right];
        let input = SequentialSliceOfVecs::new(channels.as_slice(), 2, chunk_size)
            .map_err(|e| SinkError::OnWrite(format!("audioadapter wrap failed: {e}")))?;

        let indexing = Indexing {
            input_offset: 0,
            output_offset: 0,
            partial_len: Some(remaining),
            active_channels_mask: None,
        };

        let out_frames = resampler.output_frames_next();
        let out_channels = resampler.nbr_channels();
        let mut buffer_out = rubato::audioadapter_buffers::owned::InterleavedOwned::<f64>::new(
            0.0,
            out_channels,
            out_frames,
        );

        let (_in_used, out_written) = resampler
            .process_into_buffer(&input, &mut buffer_out, Some(&indexing))
            .map_err(|e| SinkError::OnWrite(format!("rubato flush failed: {e}")))?;

        let mut frames = Vec::with_capacity(out_written);
        for i in 0..out_written {
            let l = buffer_out.read_sample(0, i).unwrap_or(0.0) as f32;
            let r = buffer_out.read_sample(1, i).unwrap_or(0.0) as f32;
            frames.push(StereoFrame { l, r });
        }

        if !frames.is_empty() {
            // Best-effort send on stop; don't error if the receiver is gone.
            let _ = tx.send(frames);
        }

        Ok(())
    }

    /// Drain accumulation buffers in chunk_size increments.
    fn drain_full_chunks(&mut self) -> SinkResult<()> {
        while self.accum_l.len() >= self.chunk_size {
            let left: Vec<f64> = self.accum_l.drain(..self.chunk_size).collect();
            let right: Vec<f64> = self.accum_r.drain(..self.chunk_size).collect();
            self.resample_and_send(&left, &right)?;
        }
        Ok(())
    }
}

impl Open for SpotifySink {
    fn open(_device: Option<String>, _format: LibrespotAudioFormat) -> Self {
        Self::new()
    }
}

impl Sink for SpotifySink {
    fn start(&mut self) -> SinkResult<()> {
        self.resampler = Some(Self::build_resampler(self.chunk_size)?);
        self.accum_l.clear();
        self.accum_r.clear();
        Ok(())
    }

    fn stop(&mut self) -> SinkResult<()> {
        self.flush_partial()?;
        self.resampler = None;
        self.accum_l.clear();
        self.accum_r.clear();
        Ok(())
    }

    fn write(&mut self, packet: AudioPacket, _converter: &mut Converter) -> SinkResult<()> {
        let samples = match packet {
            AudioPacket::Samples(s) => s,
            AudioPacket::Raw(_) => {
                return Err(SinkError::OnWrite(
                    "SpotifySink does not support raw packets".into(),
                ));
            }
        };

        // De-interleave stereo f64 into per-channel accumulators.
        let frame_count = samples.len() / 2;
        self.accum_l.reserve(frame_count);
        self.accum_r.reserve(frame_count);
        for chunk in samples.chunks_exact(2) {
            self.accum_l.push(chunk[0]);
            self.accum_r.push(chunk[1]);
        }

        self.drain_full_chunks()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// Helper: create a sink wired to a receiver, with start() already called.
    fn test_sink() -> (SpotifySink, mpsc::Receiver<Vec<StereoFrame>>) {
        let (tx, rx) = mpsc::sync_channel(CHANNEL_CAPACITY);
        let mut sink = SpotifySink::new();
        sink.set_sender(tx);
        sink.start().expect("start should succeed");
        (sink, rx)
    }

    /// Helper: build a Converter with no dithering.
    fn test_converter() -> Converter {
        Converter::new(None)
    }

    /// Collect all available frames from the receiver without blocking.
    fn drain_rx(rx: &mpsc::Receiver<Vec<StereoFrame>>) -> Vec<StereoFrame> {
        let mut out = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            out.extend(chunk);
        }
        out
    }

    // ---------------------------------------------------------------
    // 1. Resample ratio: output count ~= input * 48000/44100
    // ---------------------------------------------------------------
    #[test]
    fn resample_ratio() {
        let (mut sink, rx) = test_sink();
        let mut converter = test_converter();

        // Send enough frames to produce output (several chunks worth).
        let num_input_frames: usize = 4096;
        let samples: Vec<f64> = (0..num_input_frames * 2)
            .map(|i| {
                let t = i as f64 / (44_100.0 * 2.0);
                (t * 440.0 * std::f64::consts::TAU).sin() * 0.5
            })
            .collect();

        sink.write(AudioPacket::Samples(samples), &mut converter)
            .expect("write should succeed");
        sink.stop().expect("stop should succeed");

        let output = drain_rx(&rx);
        let expected = (num_input_frames as f64 * 48_000.0 / 44_100.0).round() as usize;

        let diff = (output.len() as isize - expected as isize).unsigned_abs();
        assert!(
            diff <= 2,
            "output frame count {actual} should be within 2 of expected {expected} \
             (diff = {diff})",
            actual = output.len(),
        );
    }

    // ---------------------------------------------------------------
    // 2. Sample range: max-amplitude f64 maps to [-1.0, 1.0] f32
    // ---------------------------------------------------------------
    #[test]
    fn sample_range() {
        let (mut sink, rx) = test_sink();
        let mut converter = test_converter();

        let num_input_frames: usize = 4096;
        // A 440 Hz sine wave at full amplitude — a realistic max-level signal.
        // (Alternating +1/-1 is a Nyquist-frequency signal that causes ringing
        // overshoot with any sinc resampler, so we use a proper tone instead.)
        let samples: Vec<f64> = (0..num_input_frames * 2)
            .map(|i| {
                let frame_idx = i / 2;
                let t = frame_idx as f64 / 44_100.0;
                (t * 440.0 * std::f64::consts::TAU).sin()
            })
            .collect();

        sink.write(AudioPacket::Samples(samples), &mut converter)
            .expect("write should succeed");
        sink.stop().expect("stop should succeed");

        let output = drain_rx(&rx);
        assert!(!output.is_empty(), "should have output frames");
        for (i, frame) in output.iter().enumerate() {
            assert!(
                frame.l >= -1.0 && frame.l <= 1.0,
                "frame {i}: l={} out of range",
                frame.l
            );
            assert!(
                frame.r >= -1.0 && frame.r <= 1.0,
                "frame {i}: r={} out of range",
                frame.r
            );
        }
    }

    // ---------------------------------------------------------------
    // 3. Accumulation: a small packet produces no output until the
    //    chunk is filled.
    // ---------------------------------------------------------------
    #[test]
    fn accumulation() {
        let (mut sink, rx) = test_sink();
        let mut converter = test_converter();

        // A packet smaller than DEFAULT_CHUNK_SIZE.
        let small_count = DEFAULT_CHUNK_SIZE / 4;
        let samples: Vec<f64> = vec![0.0; small_count * 2];
        sink.write(AudioPacket::Samples(samples), &mut converter)
            .expect("write should succeed");

        // Nothing emitted yet.
        let output = drain_rx(&rx);
        assert!(
            output.is_empty(),
            "no output expected for sub-chunk packet, got {} frames",
            output.len()
        );

        // Now fill the rest to exceed one chunk.
        let remaining = DEFAULT_CHUNK_SIZE - small_count + 1;
        let samples2: Vec<f64> = vec![0.0; remaining * 2];
        sink.write(AudioPacket::Samples(samples2), &mut converter)
            .expect("write should succeed");

        let output = drain_rx(&rx);
        assert!(
            !output.is_empty(),
            "output expected after filling a full chunk"
        );
    }

    // ---------------------------------------------------------------
    // 4. Flush: partial chunk is flushed on stop()
    // ---------------------------------------------------------------
    #[test]
    fn flush_on_stop() {
        let (mut sink, rx) = test_sink();
        let mut converter = test_converter();

        // Write less than one full chunk.
        let small_count = DEFAULT_CHUNK_SIZE / 2;
        let samples: Vec<f64> = (0..small_count * 2)
            .map(|i| (i as f64 * 0.001).sin())
            .collect();
        sink.write(AudioPacket::Samples(samples), &mut converter)
            .expect("write should succeed");

        // Nothing emitted yet.
        assert!(
            drain_rx(&rx).is_empty(),
            "no output before stop for partial chunk"
        );

        // stop() should flush the remainder.
        sink.stop().expect("stop should succeed");

        let output = drain_rx(&rx);
        assert!(
            !output.is_empty(),
            "flush on stop should produce output for partial chunk"
        );
    }

    // ---------------------------------------------------------------
    // 5. Start/stop lifecycle: no panic when no writes occur.
    // ---------------------------------------------------------------
    #[test]
    fn start_stop_no_panic() {
        let (mut sink, _rx) = test_sink();
        sink.stop().expect("stop without write should not panic");

        // Second cycle.
        sink.start().expect("re-start should succeed");
        sink.stop().expect("re-stop should succeed");
    }
}
