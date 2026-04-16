//! Bridge between librespot's `Sink` trait and clitunes' `PcmWriter` trait.
//!
//! [`SpotifySink`] receives decoded PCM from librespot at 44100 Hz,
//! resamples to the daemon's target sample rate via rubato, and pushes
//! `StereoFrame` slices through an `mpsc::SyncSender` to whoever is
//! currently *bound* to the sink via [`SpotifySinkHandle::bind`].
//!
//! # Why the sink is rebindable
//!
//! librespot's `Player::new` consumes a sink via an `FnOnce` builder, so
//! the sink is a singleton for the Player's lifetime. clitunes shares a
//! single `Arc<Player>` across tracks (and, in v1.2, across OAuth-URI
//! playback and Spotify Connect), but each playback has its own
//! `PcmWriter`. The sink therefore holds an `Arc<Mutex<Option<_>>>`
//! that the active playback *binds* for its duration and *unbinds* when
//! it ends. PCM produced while no binding is active is silently
//! discarded — this is the normal gap between tracks.
//!
//! # Sample rate
//!
//! The target rate is chosen once, when the daemon has probed the audio
//! device and first builds the Player. All playbacks use the same rate,
//! which is fine because the daemon's PCM ring runs at that one rate.
//! On 44.1 kHz hardware the sink is a near-identity pass (rubato handles
//! 44100→44100 correctly); on 48 kHz hardware it upsamples. A stale
//! 48 kHz target on a 44.1 kHz device caused the pitch-shift regression
//! that motivated making this configurable in v1.1.

use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};

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
use tokio::sync::Notify;

use clitunes_core::StereoFrame;

/// Default chunk size for the fixed-input resampler.
///
/// 1024 frames is large enough to amortise per-call overhead while keeping
/// latency well under one video frame (~21 ms at 48 kHz).
const DEFAULT_CHUNK_SIZE: usize = 1024;

/// Bounded channel capacity for back-pressure between the sink and the
/// consumer thread. When full, `SyncSender::send` blocks, preventing OOM.
const CHANNEL_CAPACITY: usize = 32;

/// librespot delivers decoded PCM at 44.1 kHz for Ogg Vorbis tracks
/// (the codec Spotify streams to its premium clients). The sink
/// resamples from this rate to the caller's `target_rate`.
const SPOTIFY_SOURCE_RATE: u32 = 44_100;

/// What the sink currently routes PCM to. Cloned out of the shared
/// mutex for each chunk so the mutex is never held across the blocking
/// `SyncSender::send`.
#[derive(Clone)]
struct SinkBinding {
    tx: SyncSender<Vec<StereoFrame>>,
    notify: Arc<Notify>,
}

/// Shared slot the sink writes into and the consumer swaps via
/// [`SpotifySinkHandle`]. `std::sync::Mutex` (not `tokio::sync::Mutex`)
/// because the sink runs on librespot's dedicated thread, outside tokio,
/// and the critical section is a ~nanosecond option swap.
type SharedBinding = Arc<Mutex<Option<SinkBinding>>>;

/// Build a new [`SpotifySink`] plus a clonable handle for binding
/// consumers to it. The sink is intended to be moved into
/// `Player::new`'s sink-builder closure; the handle stays with the
/// caller so every playback can rebind freely.
pub fn new_sink(target_rate: u32) -> (SpotifySink, SpotifySinkHandle) {
    let bound: SharedBinding = Arc::new(Mutex::new(None));
    let sink = SpotifySink::new(target_rate, Arc::clone(&bound));
    let handle = SpotifySinkHandle { bound };
    (sink, handle)
}

/// Caller-side handle used to attach and detach PCM consumers from the
/// shared [`SpotifySink`]. Cheap to clone; all clones share one slot.
#[derive(Clone)]
pub struct SpotifySinkHandle {
    bound: SharedBinding,
}

impl SpotifySinkHandle {
    /// Attach a fresh PCM channel to the sink and return the consumer
    /// end. The sink will start routing decoded, resampled frames to
    /// this channel immediately. Any previous binding is replaced.
    pub fn bind(&self) -> (mpsc::Receiver<Vec<StereoFrame>>, Arc<Notify>) {
        let (tx, rx) = mpsc::sync_channel(CHANNEL_CAPACITY);
        let notify = Arc::new(Notify::new());
        *self.bound.lock().expect("sink binding mutex poisoned") = Some(SinkBinding {
            tx,
            notify: Arc::clone(&notify),
        });
        (rx, notify)
    }

    /// Detach the current consumer. Subsequent decoded frames are
    /// silently dropped until someone calls [`Self::bind`] again.
    pub fn unbind(&self) {
        *self.bound.lock().expect("sink binding mutex poisoned") = None;
    }

    /// Whether `self` and `other` address the same underlying sink
    /// binding slot. Two clones of the same handle return `true`;
    /// handles produced by separate [`new_sink`] calls return `false`.
    ///
    /// Used by Spotify Connect to detect that [`ConnectRuntime`] has
    /// rotated the sink (fresh Session/Player on re-pair) so
    /// [`ConnectSource`] can stop draining the old sink and let the
    /// source pipeline re-enter Connect against the new one. An
    /// identity check is necessary because the old sink's tx is held
    /// alive by the still-bound `SpotifySinkHandle`, so receivers on
    /// the old channel do *not* observe `Disconnected` when the old
    /// Player drops — a naïve "wait for Disconnected" loop would hang.
    ///
    /// [`ConnectRuntime`]: super::ConnectRuntime
    /// [`ConnectSource`]: super::ConnectSource
    #[cfg(feature = "connect")]
    pub fn points_to_same_slot(&self, other: &SpotifySinkHandle) -> bool {
        Arc::ptr_eq(&self.bound, &other.bound)
    }
}

pub struct SpotifySink {
    bound: SharedBinding,
    resampler: Option<Async<f64>>,
    accum_l: Vec<f64>,
    accum_r: Vec<f64>,
    /// Reusable per-channel input buffers for the resampler (avoids 3 allocs/chunk).
    channel_bufs: Vec<Vec<f64>>,
    /// Reusable output buffer for resampled frames.
    output_buf: Vec<StereoFrame>,
    chunk_size: usize,
    /// Output rate the resampler is configured for. Matches the daemon
    /// PCM ring and audio device rate — see module docs.
    target_rate: u32,
}

impl SpotifySink {
    fn new(target_rate: u32, bound: SharedBinding) -> Self {
        Self {
            bound,
            resampler: None,
            accum_l: Vec::new(),
            accum_r: Vec::new(),
            channel_bufs: vec![Vec::new(), Vec::new()],
            output_buf: Vec::new(),
            chunk_size: DEFAULT_CHUNK_SIZE,
            target_rate,
        }
    }

    /// Build the rubato `Async` sinc resampler (44100 Hz → `target_rate` Hz,
    /// stereo). When `target_rate == 44_100` the ratio is 1:1 and rubato
    /// still produces a correct identity pass.
    fn build_resampler(chunk_size: usize, target_rate: u32) -> Result<Async<f64>, SinkError> {
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };
        Async::<f64>::new_sinc(
            target_rate as f64 / SPOTIFY_SOURCE_RATE as f64,
            2.0,
            &params,
            chunk_size,
            2,
            FixedAsync::Input,
        )
        .map_err(|e| SinkError::InvalidParams(format!("rubato init failed: {e}")))
    }

    /// Snapshot the current binding. Cloning outside the lock keeps the
    /// blocking `SyncSender::send` off the critical path and lets the
    /// consumer `unbind()` at any moment without deadlocking the sink.
    fn current_binding(&self) -> Option<SinkBinding> {
        self.bound
            .lock()
            .expect("sink binding mutex poisoned")
            .clone()
    }

    /// Deliver the filled `output_buf` to whoever is bound, clearing
    /// accumulators if the receiver has gone away. Unbound state is
    /// normal between tracks — we simply drop the PCM on the floor.
    fn deliver_output(&mut self) -> SinkResult<()> {
        if self.output_buf.is_empty() {
            return Ok(());
        }
        let Some(binding) = self.current_binding() else {
            // No consumer right now — between-tracks gap. Discard
            // silently; librespot will keep decoding until it hits
            // EndOfTrack / Stop.
            self.output_buf.clear();
            return Ok(());
        };

        if binding
            .tx
            .send(std::mem::take(&mut self.output_buf))
            .is_err()
        {
            // Receiver dropped — the current playback is tearing down.
            // Clear accumulators so the next binding starts clean, and
            // swallow the error rather than surfacing it to librespot
            // (which would log it as an audio-sink failure).
            self.accum_l.clear();
            self.accum_r.clear();
            return Ok(());
        }
        binding.notify.notify_one();
        Ok(())
    }

    /// Push one complete chunk (of exactly `chunk_size` frames per channel)
    /// through the resampler and send the resulting `StereoFrame`s.
    fn resample_and_send(&mut self, left: &[f64], right: &[f64]) -> SinkResult<()> {
        let resampler = self
            .resampler
            .as_mut()
            .ok_or_else(|| SinkError::NotConnected("resampler not initialised".into()))?;

        self.channel_bufs[0].clear();
        self.channel_bufs[0].extend_from_slice(left);
        self.channel_bufs[1].clear();
        self.channel_bufs[1].extend_from_slice(right);
        let input = SequentialSliceOfVecs::new(self.channel_bufs.as_slice(), 2, left.len())
            .map_err(|e| SinkError::OnWrite(format!("audioadapter wrap failed: {e}")))?;

        let output = resampler
            .process(&input, 0, None)
            .map_err(|e| SinkError::OnWrite(format!("rubato resample failed: {e}")))?;

        let out_frames = output.frames();
        self.output_buf.clear();
        self.output_buf.reserve(out_frames);
        for i in 0..out_frames {
            let l = output.read_sample(0, i).unwrap_or(0.0) as f32;
            let r = output.read_sample(1, i).unwrap_or(0.0) as f32;
            self.output_buf.push(StereoFrame { l, r });
        }

        self.deliver_output()
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

        self.output_buf.clear();
        self.output_buf.reserve(out_written);
        for i in 0..out_written {
            let l = buffer_out.read_sample(0, i).unwrap_or(0.0) as f32;
            let r = buffer_out.read_sample(1, i).unwrap_or(0.0) as f32;
            self.output_buf.push(StereoFrame { l, r });
        }

        self.deliver_output()
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
    /// Required by the librespot `Open` trait but not used in our path
    /// — callers construct the sink via [`new_sink`] with an explicit
    /// target rate and a shared binding slot. librespot never calls
    /// this in our embedding, but a default is compile-required. Picks
    /// 48 kHz and an orphan binding; any real use always goes through
    /// `new_sink()`.
    fn open(_device: Option<String>, _format: LibrespotAudioFormat) -> Self {
        Self::new(48_000, Arc::new(Mutex::new(None)))
    }
}

impl Sink for SpotifySink {
    fn start(&mut self) -> SinkResult<()> {
        self.resampler = Some(Self::build_resampler(self.chunk_size, self.target_rate)?);
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

    /// Helper: build a sink + handle, bind a receiver, call `start()`.
    /// Uses the historical 48 kHz target so the existing ratio tests
    /// still exercise real resampling rather than a 1:1 pass.
    fn test_sink() -> (SpotifySink, mpsc::Receiver<Vec<StereoFrame>>) {
        test_sink_at(48_000)
    }

    fn test_sink_at(target_rate: u32) -> (SpotifySink, mpsc::Receiver<Vec<StereoFrame>>) {
        let (mut sink, handle) = new_sink(target_rate);
        let (rx, _notify) = handle.bind();
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
    // 5. 44.1 kHz target is a near-identity pass (pitch-shift regression
    //    guard): with target_rate == 44100, output frame count ≈ input.
    //    If the resampler were still wired for a fixed 48 kHz output we'd
    //    see ~8.8% more frames, which on a 44.1 kHz device plays ~1
    //    semitone high.
    // ---------------------------------------------------------------
    #[test]
    fn target_rate_44100_is_identity_pass() {
        let (mut sink, rx) = test_sink_at(44_100);
        let mut converter = test_converter();

        let num_input_frames: usize = 4096;
        let samples: Vec<f64> = (0..num_input_frames * 2)
            .map(|i| {
                let frame_idx = i / 2;
                let t = frame_idx as f64 / 44_100.0;
                (t * 440.0 * std::f64::consts::TAU).sin() * 0.5
            })
            .collect();

        sink.write(AudioPacket::Samples(samples), &mut converter)
            .expect("write should succeed");
        sink.stop().expect("stop should succeed");

        let output = drain_rx(&rx);
        // rubato sinc resamplers have a small group-delay priming cost,
        // but the drained total (including flush) should land within a
        // few frames of the input count.
        let diff = (output.len() as isize - num_input_frames as isize).unsigned_abs();
        assert!(
            diff <= 4,
            "44.1 kHz target should be identity: got {} frames vs {} input (diff = {})",
            output.len(),
            num_input_frames,
            diff,
        );
    }

    // ---------------------------------------------------------------
    // 6. Start/stop lifecycle: no panic when no writes occur.
    // ---------------------------------------------------------------
    #[test]
    fn start_stop_no_panic() {
        let (mut sink, _rx) = test_sink();
        sink.stop().expect("stop without write should not panic");

        // Second cycle.
        sink.start().expect("re-start should succeed");
        sink.stop().expect("re-stop should succeed");
    }

    // ---------------------------------------------------------------
    // 7. Rebinding: unbind then re-bind routes frames to the new
    //    consumer. The previous consumer's receiver goes to Disconnected
    //    the next time the sink touches it (if at all), and the fresh
    //    receiver starts clean.
    // ---------------------------------------------------------------
    #[test]
    fn rebind_routes_to_new_consumer() {
        let (mut sink, handle) = new_sink(48_000);
        let mut converter = test_converter();
        sink.start().expect("start should succeed");

        // First binding — drain enough to confirm routing.
        let (rx1, _notify1) = handle.bind();
        let samples: Vec<f64> = vec![0.0; DEFAULT_CHUNK_SIZE * 2];
        sink.write(AudioPacket::Samples(samples), &mut converter)
            .expect("write should succeed");
        assert!(
            !drain_rx(&rx1).is_empty(),
            "first binding should receive frames"
        );

        // Unbind and emit a chunk into the void — should not error.
        handle.unbind();
        let samples: Vec<f64> = vec![0.0; DEFAULT_CHUNK_SIZE * 2];
        sink.write(AudioPacket::Samples(samples), &mut converter)
            .expect("write should succeed while unbound (frames discarded)");
        assert!(drain_rx(&rx1).is_empty(), "rx1 should see no new frames");

        // Re-bind and drain — the new receiver gets subsequent output.
        let (rx2, _notify2) = handle.bind();
        let samples: Vec<f64> = vec![0.0; DEFAULT_CHUNK_SIZE * 2];
        sink.write(AudioPacket::Samples(samples), &mut converter)
            .expect("write should succeed");
        assert!(
            !drain_rx(&rx2).is_empty(),
            "re-bound receiver should get frames"
        );
    }

    // ---------------------------------------------------------------
    // 7b. Identity: clones of one handle compare equal via
    //     points_to_same_slot; handles from separate new_sink calls
    //     compare unequal. Used by ConnectSource to detect sink rotation.
    // ---------------------------------------------------------------
    #[cfg(feature = "connect")]
    #[test]
    fn points_to_same_slot_distinguishes_sinks() {
        let (_sink_a, handle_a) = new_sink(48_000);
        let (_sink_b, handle_b) = new_sink(48_000);
        let handle_a_clone = handle_a.clone();

        assert!(
            handle_a.points_to_same_slot(&handle_a_clone),
            "clones of the same handle address the same slot"
        );
        assert!(
            !handle_a.points_to_same_slot(&handle_b),
            "handles from separate new_sink() calls address different slots"
        );
    }

    // ---------------------------------------------------------------
    // 7c. Old rx does NOT observe Disconnected when the old Player/Sink
    //     drops while some SpotifySinkHandle clone remains bound. This
    //     is the failure mode that motivated points_to_same_slot: a
    //     ConnectSource waiting on Disconnected would hang forever.
    // ---------------------------------------------------------------
    #[test]
    fn rx_stays_connected_while_other_handle_clone_lives() {
        let (sink, handle) = new_sink(48_000);
        let (rx, _notify) = handle.bind();

        // Drop the sink — simulates old Player teardown on re-pair.
        drop(sink);

        // A separate handle clone survives — simulates the clone held
        // by ConnectSource's Unbinder.
        let _surviving_clone = handle.clone();
        drop(handle);

        match rx.try_recv() {
            Err(mpsc::TryRecvError::Empty) => {} // expected — tx still alive via surviving clone
            Err(mpsc::TryRecvError::Disconnected) => panic!(
                "rx disconnected while a SpotifySinkHandle clone still lives — \
                 the premise of points_to_same_slot is wrong"
            ),
            Ok(_) => panic!("unexpected frames on empty sink"),
        }
    }

    // ---------------------------------------------------------------
    // 8. Dropped receiver mid-playback is a silent no-op for the sink
    //    (mirrors the PR #29 shutdown-deadlock fix: we never surface
    //    SinkError::NotConnected for a legitimately torn-down binding).
    // ---------------------------------------------------------------
    #[test]
    fn dropped_receiver_does_not_error() {
        let (mut sink, handle) = new_sink(48_000);
        let mut converter = test_converter();
        sink.start().expect("start should succeed");

        let (rx, _notify) = handle.bind();
        drop(rx);

        // write after rx drop must not error — librespot would log it
        // as an audio-sink failure, which is misleading during normal
        // playback teardown.
        let samples: Vec<f64> = vec![0.0; DEFAULT_CHUNK_SIZE * 2];
        sink.write(AudioPacket::Samples(samples), &mut converter)
            .expect("write after rx drop should not surface SinkError");
    }
}
