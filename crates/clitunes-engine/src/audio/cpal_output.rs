//! OS audio output via cpal.
//!
//! This is the final piece of Unit 7: the consumer end of the PCM ring
//! that actually reaches the user's speakers. The decoder
//! ([`crate::sources::symphonia_decode`]) writes `StereoFrame`s into the
//! ring; [`CpalOutput`] drains them and hands them to the host audio API.
//!
//! # Format negotiation
//!
//! The ring runs at whatever rate the device negotiates (probed via
//! [`CpalOutput::probe_device_rate`] before ring creation). Every cpal backend
//! supports a different subset of rates / channel counts / sample
//! formats, so [`select_output_config`] walks the device's advertised
//! `SupportedStreamConfigRange`s with a strict preference order:
//!
//! 1. 48 kHz stereo f32 (fast path — zero conversion)
//! 2. 48 kHz stereo i16 (format only)
//! 3. 48 kHz mono f32 (downmix)
//! 4. 48 kHz mono i16 (downmix + format)
//! 5. 44.1 kHz stereo f32 (resample only)
//! 6. 44.1 kHz stereo i16 (resample + format)
//! 7. 44.1 kHz mono f32 (resample + downmix)
//! 8. 44.1 kHz mono i16 (resample + downmix + format)
//! 9. Whatever `default_output_config()` returns (final fallback)
//!
//! The intent is to pick the cheapest viable config first so the audio
//! thread spends as little time as possible converting. The preference
//! list is a pure function over the advertised ranges — tests can hand
//! it a `Vec<SupportedStreamConfigRange>` without opening a real device.
//!
//! # The audio callback
//!
//! cpal invokes our callback on a real-time thread. It receives a
//! `&mut [T]` sized to N frames of the negotiated format (`T` is the
//! chosen sample type). We:
//!
//! 1. Work out how many ring frames we need (same count if rates match,
//!    more if we must resample).
//! 2. [`PcmRingReader::drain_into`] pulls that many frames; missing
//!    frames become `StereoFrame::SILENCE` and are counted as underruns.
//! 3. If the device rate differs from the ring rate, a tiny linear
//!    resampler converts on the fly.
//! 4. A per-format `write_device_samples_*` helper writes into the cpal
//!    buffer (stereo/mono, f32/i16/u16).
//!
//! Both scratch buffers are pre-allocated at [`CpalOutput::start`] time
//! and sized to an upper bound so the callback never reallocates. If
//! cpal ever hands us a buffer larger than that bound the callback
//! falls back to silence for the overflow and bumps the underrun
//! counter — an observable failure, not memory churn on the audio
//! thread.
//!
//! # Why the stream handle is `!Send` on macOS
//!
//! `cpal::Stream` isn't `Send` on CoreAudio because the AudioUnit object
//! it wraps is thread-bound. The clitunes binary constructs
//! [`CpalOutput`] on the main thread and holds it there until shutdown,
//! so this is fine — but it means `CpalOutput` itself is intentionally
//! not `Send`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use clitunes_core::StereoFrame;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    ChannelCount, Device, SampleFormat, SampleRate, Stream, StreamConfig, SupportedStreamConfig,
    SupportedStreamConfigRange,
};
use tracing::{debug, error, info, trace, warn};

use super::PcmRingReader;

/// Target rate we push into cpal as the first-choice preference. The
/// ring is canonicalised on this rate, so matching it skips the
/// resampler entirely.
pub const PRIMARY_RATE: u32 = 48_000;

/// Fallback rate if the device can't do 48 kHz. 44.1 kHz is the other
/// ubiquitous rate; every cpal backend we target supports one or both.
pub const FALLBACK_RATE: u32 = 44_100;

/// How many frames we reserve in the scratch buffers. cpal backends
/// typically hand us 128–2048 frames per callback; 8192 is a generous
/// upper bound that still fits a few ms of latency headroom even if
/// the backend asks for more.
const SCRATCH_CAPACITY_FRAMES: usize = 8_192;

/// Caller-facing config for [`CpalOutput::start`]. Kept deliberately
/// small — everything else is negotiated from the device's advertised
/// capabilities.
#[derive(Clone, Debug, Default)]
pub struct CpalOutputConfig {
    /// Pick a specific output device by substring match on its name.
    /// `None` uses the host default device.
    pub device_name: Option<String>,
}

/// What we actually negotiated with the device. Exposed so the main
/// binary can log it at boot for diagnostics.
#[derive(Clone, Debug)]
pub struct NegotiatedFormat {
    pub device_name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: SampleFormat,
}

/// Handle to an active cpal output stream. Dropping this stops
/// playback — cpal's `Stream` stops when it's dropped.
pub struct CpalOutput {
    _stream: Stream,
    negotiated: NegotiatedFormat,
    underruns: Arc<AtomicU64>,
}

impl CpalOutput {
    /// Probe the default (or named) output device and return the sample
    /// rate it would negotiate, without opening a stream. This lets the
    /// caller create the PCM ring at the device's native rate so the
    /// audio callback can skip resampling entirely.
    pub fn probe_device_rate(cfg: &CpalOutputConfig) -> u32 {
        let host = cpal::default_host();
        let device = match pick_device(&host, cfg.device_name.as_deref()) {
            Ok(d) => d,
            Err(_) => return PRIMARY_RATE,
        };
        let supported: Vec<SupportedStreamConfigRange> = match device.supported_output_configs() {
            Ok(iter) => iter.collect(),
            Err(_) => return PRIMARY_RATE,
        };
        match select_output_config(&supported) {
            Some(cfg) => cfg.sample_rate().0,
            None => device
                .default_output_config()
                .map(|c| c.sample_rate().0)
                .unwrap_or(PRIMARY_RATE),
        }
    }

    /// Open the host's default (or named) output device, negotiate the
    /// best-matching format for our ring, install an output callback
    /// that drains `reader`, and start the stream.
    ///
    /// `ring_rate` is the sample rate of the PCM ring. When it matches
    /// the device's negotiated rate (the common case after probing),
    /// the callback skips the resampler entirely.
    ///
    /// Errors bubble up through `anyhow::Result` with enough context to
    /// pinpoint which step failed (host / device / config / build /
    /// play). The caller is expected to log the error and fall back to
    /// no-audio mode rather than panic.
    pub fn start(reader: PcmRingReader, cfg: CpalOutputConfig, ring_rate: u32) -> Result<Self> {
        let host = cpal::default_host();
        let device = pick_device(&host, cfg.device_name.as_deref())
            .context("no usable cpal output device")?;
        let device_name = device
            .name()
            .unwrap_or_else(|_| "<unnamed device>".to_string());

        let supported: Vec<SupportedStreamConfigRange> = device
            .supported_output_configs()
            .context("cpal: supported_output_configs failed")?
            .collect();

        let selected = match select_output_config(&supported) {
            Some(s) => s,
            None => {
                warn!(
                    device = %device_name,
                    advertised = supported.len(),
                    "no preferred cpal config — falling back to default_output_config"
                );
                device
                    .default_output_config()
                    .context("cpal: default_output_config fallback failed")?
            }
        };

        let negotiated = NegotiatedFormat {
            device_name: device_name.clone(),
            sample_rate: selected.sample_rate().0,
            channels: selected.channels() as u16,
            sample_format: selected.sample_format(),
        };
        info!(
            device = %negotiated.device_name,
            rate = negotiated.sample_rate,
            channels = negotiated.channels,
            format = ?negotiated.sample_format,
            "cpal output negotiated"
        );

        let stream_config: StreamConfig = selected.config();
        let underruns = Arc::new(AtomicU64::new(0));
        let stream =
            build_stream(&device, &stream_config, &negotiated, reader, &underruns, ring_rate)?;
        stream.play().context("cpal: Stream::play failed")?;

        Ok(Self {
            _stream: stream,
            negotiated,
            underruns,
        })
    }

    pub fn negotiated(&self) -> &NegotiatedFormat {
        &self.negotiated
    }

    /// Monotonic count of ring frames that had to be replaced with
    /// silence because the producer hadn't delivered them in time.
    /// Non-zero at startup is expected (decoder priming); sustained
    /// growth means the decoder can't keep up.
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// Device picking
// ---------------------------------------------------------------------------

fn pick_device(host: &cpal::Host, wanted: Option<&str>) -> Result<Device> {
    if let Some(name) = wanted {
        for dev in host
            .output_devices()
            .context("cpal: output_devices failed")?
        {
            if let Ok(dn) = dev.name() {
                if dn.to_ascii_lowercase().contains(&name.to_ascii_lowercase()) {
                    debug!(match_name = %dn, wanted = %name, "cpal: matched named device");
                    return Ok(dev);
                }
            }
        }
        warn!(wanted = %name, "cpal: named device not found, using default");
    }
    host.default_output_device()
        .ok_or_else(|| anyhow!("cpal: host has no default output device"))
}

// ---------------------------------------------------------------------------
// Config preference walk (pure, testable)
// ---------------------------------------------------------------------------

/// Walk the advertised output configs and pick the one that minimises
/// conversion work on the audio thread. See the module docs for the
/// full preference order.
///
/// Returns `None` if no advertised range can supply 48 kHz or 44.1 kHz
/// at a sample format we can render to. The caller should then fall
/// back to `default_output_config()`.
pub fn select_output_config(
    advertised: &[SupportedStreamConfigRange],
) -> Option<SupportedStreamConfig> {
    // Format preference order. `f32` first because our ring is already
    // f32 — no math required in the common path.
    const PREFERRED_FORMATS: &[SampleFormat] = &[SampleFormat::F32, SampleFormat::I16];
    const PREFERRED_RATES: &[u32] = &[PRIMARY_RATE, FALLBACK_RATE];
    // Prefer stereo (our ring is stereo → zero-conversion path) over
    // mono (requires a downmix). Anything higher than stereo falls
    // through to the final default fallback in the caller.
    const PREFERRED_CHANNELS: &[ChannelCount] = &[2, 1];

    for &rate in PREFERRED_RATES {
        let target = SampleRate(rate);
        for &channels in PREFERRED_CHANNELS {
            for &fmt in PREFERRED_FORMATS {
                for range in advertised {
                    if range.channels() != channels {
                        continue;
                    }
                    if range.sample_format() != fmt {
                        continue;
                    }
                    if let Some(cfg) = (*range).try_with_sample_rate(target) {
                        return Some(cfg);
                    }
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Stream building + callback wiring
// ---------------------------------------------------------------------------

fn build_stream(
    device: &Device,
    config: &StreamConfig,
    negotiated: &NegotiatedFormat,
    reader: PcmRingReader,
    underruns: &Arc<AtomicU64>,
    ring_rate: u32,
) -> Result<Stream> {
    // cpal builds a dedicated stream constructor per sample type, so
    // we branch once here and each arm captures a monomorphised
    // callback. The callback bodies themselves are generic over the
    // sample type via `write_device_samples`.
    let err_cb = |e| error!(error = %e, "cpal stream error");
    let timeout: Option<Duration> = None;

    match negotiated.sample_format {
        SampleFormat::F32 => device
            .build_output_stream::<f32, _, _>(
                config,
                make_callback::<f32>(reader, Arc::clone(underruns), negotiated.clone(), ring_rate),
                err_cb,
                timeout,
            )
            .context("cpal: build_output_stream<f32> failed"),
        SampleFormat::I16 => device
            .build_output_stream::<i16, _, _>(
                config,
                make_callback::<i16>(reader, Arc::clone(underruns), negotiated.clone(), ring_rate),
                err_cb,
                timeout,
            )
            .context("cpal: build_output_stream<i16> failed"),
        SampleFormat::U16 => device
            .build_output_stream::<u16, _, _>(
                config,
                make_callback::<u16>(reader, Arc::clone(underruns), negotiated.clone(), ring_rate),
                err_cb,
                timeout,
            )
            .context("cpal: build_output_stream<u16> failed"),
        other => Err(anyhow!(
            "cpal: unsupported negotiated sample format {other:?}"
        )),
    }
}

/// Trait bridging `StereoFrame` → the device's sample type. Isolates
/// the per-format math so `make_callback` can be generic.
trait DeviceSample: cpal::SizedSample + Send + 'static {
    fn silence() -> Self;
    fn from_stereo(frame: StereoFrame, channels: u16, out: &mut [Self]);
}

impl DeviceSample for f32 {
    #[inline]
    fn silence() -> Self {
        0.0
    }
    #[inline]
    fn from_stereo(frame: StereoFrame, channels: u16, out: &mut [Self]) {
        match channels {
            1 => out[0] = 0.5 * (frame.l + frame.r),
            _ => {
                out[0] = frame.l;
                out[1] = frame.r;
                for slot in out.iter_mut().skip(2) {
                    *slot = 0.0;
                }
            }
        }
    }
}

impl DeviceSample for i16 {
    #[inline]
    fn silence() -> Self {
        0
    }
    #[inline]
    fn from_stereo(frame: StereoFrame, channels: u16, out: &mut [Self]) {
        match channels {
            1 => out[0] = f32_to_i16(0.5 * (frame.l + frame.r)),
            _ => {
                out[0] = f32_to_i16(frame.l);
                out[1] = f32_to_i16(frame.r);
                for slot in out.iter_mut().skip(2) {
                    *slot = 0;
                }
            }
        }
    }
}

impl DeviceSample for u16 {
    #[inline]
    fn silence() -> Self {
        u16::MAX / 2
    }
    #[inline]
    fn from_stereo(frame: StereoFrame, channels: u16, out: &mut [Self]) {
        match channels {
            1 => out[0] = f32_to_u16(0.5 * (frame.l + frame.r)),
            _ => {
                out[0] = f32_to_u16(frame.l);
                out[1] = f32_to_u16(frame.r);
                for slot in out.iter_mut().skip(2) {
                    *slot = u16::MAX / 2;
                }
            }
        }
    }
}

#[inline]
fn f32_to_i16(s: f32) -> i16 {
    let clamped = s.clamp(-1.0, 1.0);
    (clamped * i16::MAX as f32) as i16
}

#[inline]
fn f32_to_u16(s: f32) -> u16 {
    let clamped = s.clamp(-1.0, 1.0);
    let scaled = (clamped + 1.0) * 0.5 * u16::MAX as f32;
    scaled as u16
}

/// Build the per-callback closure. Captures the ring reader, a scratch
/// buffer, the optional resampler, and the underrun counter.
fn make_callback<T>(
    mut reader: PcmRingReader,
    underruns: Arc<AtomicU64>,
    negotiated: NegotiatedFormat,
    ring_rate: u32,
) -> impl FnMut(&mut [T], &cpal::OutputCallbackInfo) + Send + 'static
where
    T: DeviceSample,
{
    // Pre-allocated scratch buffers. `ring_scratch` holds frames
    // drained from the PCM ring (at the ring's native rate).
    // `device_scratch` holds the resampled frames at the device's
    // native rate. When `ring_rate == device_rate` only `ring_scratch`
    // is used.
    let mut ring_scratch: Vec<StereoFrame> = vec![StereoFrame::SILENCE; SCRATCH_CAPACITY_FRAMES];
    let mut device_scratch: Vec<StereoFrame> = vec![StereoFrame::SILENCE; SCRATCH_CAPACITY_FRAMES];
    let needs_resample = negotiated.sample_rate != ring_rate;
    let mut resampler = LinearResampler::new(ring_rate, negotiated.sample_rate);
    let channels = negotiated.channels;

    move |data: &mut [T], _info: &cpal::OutputCallbackInfo| {
        let samples_per_frame = channels.max(1) as usize;
        let device_frames_requested = data.len() / samples_per_frame;
        if device_frames_requested == 0 {
            return;
        }

        // Guard against the device suddenly asking for more frames than
        // our pre-allocated scratch. Fill the overflow with silence and
        // count it as underruns so we can see it in logs.
        let device_frames = if device_frames_requested > SCRATCH_CAPACITY_FRAMES {
            warn!(
                requested = device_frames_requested,
                capacity = SCRATCH_CAPACITY_FRAMES,
                "cpal callback asked for more frames than scratch capacity — clamping"
            );
            underruns.fetch_add(
                (device_frames_requested - SCRATCH_CAPACITY_FRAMES) as u64,
                Ordering::Relaxed,
            );
            let silence_start = SCRATCH_CAPACITY_FRAMES * samples_per_frame;
            for slot in data[silence_start..].iter_mut() {
                *slot = T::silence();
            }
            SCRATCH_CAPACITY_FRAMES
        } else {
            device_frames_requested
        };

        let frames_source: &[StereoFrame] = if needs_resample {
            // Need roughly `device_frames * (ring_rate / device_rate)`
            // ring frames. Round up and add a cushion so the linear
            // resampler has enough input to produce exactly
            // `device_frames` outputs.
            let ratio = ring_rate as f64 / negotiated.sample_rate as f64;
            let need_ring =
                (((device_frames as f64 * ratio).ceil() as usize) + 2).min(SCRATCH_CAPACITY_FRAMES);

            let got = reader.drain_into(&mut ring_scratch[..need_ring]);
            if got < need_ring {
                underruns.fetch_add((need_ring - got) as u64, Ordering::Relaxed);
            }

            // Resample into the device scratch buffer.
            let produced = resampler.resample(
                &ring_scratch[..need_ring],
                &mut device_scratch[..device_frames],
            );
            // Pad any short production with silence so the device gets
            // exactly `device_frames` frames.
            if produced < device_frames {
                let missing = device_frames - produced;
                underruns.fetch_add(missing as u64, Ordering::Relaxed);
                for slot in device_scratch[produced..device_frames].iter_mut() {
                    *slot = StereoFrame::SILENCE;
                }
            }
            &device_scratch[..device_frames]
        } else {
            // Fast path: device rate matches ring rate. Drain straight
            // into `ring_scratch` and feed it to the writer.
            let got = reader.drain_into(&mut ring_scratch[..device_frames]);
            if got < device_frames {
                underruns.fetch_add((device_frames - got) as u64, Ordering::Relaxed);
            }
            &ring_scratch[..device_frames]
        };

        trace!(
            frames = device_frames,
            resample = needs_resample,
            "cpal callback"
        );

        write_device_samples::<T>(frames_source, channels, data);
    }
}

/// Fill `out` with interleaved samples taken from `frames`. `channels`
/// is the device-negotiated channel count (1 or 2 in practice; higher
/// counts get the first two channels copied and the rest zeroed).
fn write_device_samples<T: DeviceSample>(frames: &[StereoFrame], channels: u16, out: &mut [T]) {
    let cs = channels.max(1) as usize;
    let full_frames = frames.len().min(out.len() / cs);
    for (i, frame) in frames.iter().take(full_frames).enumerate() {
        let base = i * cs;
        T::from_stereo(*frame, channels, &mut out[base..base + cs]);
    }
    // Any trailing samples in `out` (shouldn't normally happen because
    // the caller sizes `frames` correctly, but defensive) become
    // silence.
    for slot in out[full_frames * cs..].iter_mut() {
        *slot = T::silence();
    }
}

// ---------------------------------------------------------------------------
// Linear resampler
// ---------------------------------------------------------------------------

/// Minimal linear resampler. Same math as the decoder-side resampler
/// in [`crate::sources::symphonia_decode`], duplicated here because
/// that one is module-private and the two have different lifetimes
/// (the decoder resampler runs in a blocking decode loop; this one
/// runs in a real-time audio callback). Refactor to share if a third
/// caller appears.
struct LinearResampler {
    src: u32,
    dst: u32,
    pos_in: f64,
    prev: StereoFrame,
}

impl LinearResampler {
    fn new(src: u32, dst: u32) -> Self {
        Self {
            src,
            dst,
            pos_in: 0.0,
            prev: StereoFrame::SILENCE,
        }
    }

    /// Resample `input` (at `src` rate) into `out` (at `dst` rate),
    /// producing up to `out.len()` frames. Returns the number of
    /// frames actually produced. The internal cursor is carried
    /// across calls so boundaries don't click.
    fn resample(&mut self, input: &[StereoFrame], out: &mut [StereoFrame]) -> usize {
        if input.is_empty() || out.is_empty() {
            return 0;
        }
        if self.src == self.dst {
            let n = input.len().min(out.len());
            out[..n].copy_from_slice(&input[..n]);
            self.prev = input[n - 1];
            return n;
        }

        let ratio = self.src as f64 / self.dst as f64;
        let mut pos = self.pos_in;
        let input_len = input.len() as f64;
        let mut produced = 0usize;

        while produced < out.len() && pos < input_len {
            let i0 = pos.floor() as isize;
            let frac = (pos - pos.floor()) as f32;
            let from = if i0 <= 0 {
                self.prev
            } else {
                input[(i0 - 1) as usize]
            };
            let to_idx = i0.max(0) as usize;
            if to_idx >= input.len() {
                break;
            }
            let to = input[to_idx];
            out[produced] = StereoFrame {
                l: from.l + (to.l - from.l) * frac,
                r: from.r + (to.r - from.r) * frac,
            };
            produced += 1;
            pos += ratio;
        }
        self.pos_in = pos - input_len;
        if !input.is_empty() {
            self.prev = input[input.len() - 1];
        }
        produced
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::SupportedBufferSize;

    fn range(
        channels: ChannelCount,
        min: u32,
        max: u32,
        fmt: SampleFormat,
    ) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(
            channels,
            SampleRate(min),
            SampleRate(max),
            SupportedBufferSize::Range {
                min: 256,
                max: 4096,
            },
            fmt,
        )
    }

    #[test]
    fn prefers_48k_stereo_f32_when_available() {
        let ads = vec![
            range(2, 44_100, 44_100, SampleFormat::I16),
            range(2, 48_000, 48_000, SampleFormat::F32),
            range(2, 48_000, 48_000, SampleFormat::I16),
        ];
        let got = select_output_config(&ads).expect("a match");
        assert_eq!(got.sample_rate(), SampleRate(48_000));
        assert_eq!(got.channels(), 2);
        assert_eq!(got.sample_format(), SampleFormat::F32);
    }

    #[test]
    fn falls_through_to_i16_when_no_f32() {
        let ads = vec![
            range(2, 44_100, 48_000, SampleFormat::I16),
            range(1, 48_000, 48_000, SampleFormat::I16),
        ];
        let got = select_output_config(&ads).expect("a match");
        assert_eq!(got.sample_rate(), SampleRate(48_000));
        assert_eq!(got.channels(), 2);
        assert_eq!(got.sample_format(), SampleFormat::I16);
    }

    #[test]
    fn falls_through_to_mono_when_no_stereo() {
        let ads = vec![range(1, 48_000, 48_000, SampleFormat::F32)];
        let got = select_output_config(&ads).expect("a match");
        assert_eq!(got.channels(), 1);
        assert_eq!(got.sample_format(), SampleFormat::F32);
    }

    #[test]
    fn falls_through_to_44100_when_no_48000() {
        let ads = vec![
            range(2, 44_100, 44_100, SampleFormat::F32),
            range(2, 44_100, 44_100, SampleFormat::I16),
        ];
        let got = select_output_config(&ads).expect("a match");
        assert_eq!(got.sample_rate(), SampleRate(44_100));
        assert_eq!(got.channels(), 2);
        assert_eq!(got.sample_format(), SampleFormat::F32);
    }

    #[test]
    fn rate_range_covering_48k_matches() {
        // Many backends advertise a contiguous range like 44.1k–48k on
        // a single `SupportedStreamConfigRange`. We should still pick
        // 48k out of it.
        let ads = vec![range(2, 44_100, 48_000, SampleFormat::F32)];
        let got = select_output_config(&ads).expect("a match");
        assert_eq!(got.sample_rate(), SampleRate(48_000));
    }

    #[test]
    fn returns_none_when_nothing_sane_advertised() {
        // 8-channel 96 kHz only — outside our preference list.
        let ads = vec![range(8, 96_000, 96_000, SampleFormat::F32)];
        assert!(select_output_config(&ads).is_none());
    }

    // ---- Sample conversion ----

    #[test]
    fn f32_to_i16_covers_full_scale_and_clamps() {
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(1.0), i16::MAX);
        assert_eq!(f32_to_i16(-1.0), -i16::MAX); // symmetric round-to-zero
        assert_eq!(f32_to_i16(2.0), i16::MAX);
        assert_eq!(f32_to_i16(-2.0), -i16::MAX);
    }

    #[test]
    fn f32_to_u16_mid_is_half_range() {
        // f32 0.0 → u16 midpoint.
        let mid = f32_to_u16(0.0);
        assert!((i64::from(mid) - i64::from(u16::MAX / 2)).abs() <= 1);
        assert_eq!(f32_to_u16(1.0), u16::MAX);
        assert_eq!(f32_to_u16(-1.0), 0);
        assert_eq!(f32_to_u16(5.0), u16::MAX);
    }

    // ---- Frame writer ----

    #[test]
    fn write_device_samples_f32_stereo_is_passthrough() {
        let frames = vec![
            StereoFrame { l: 0.1, r: 0.2 },
            StereoFrame { l: -0.3, r: 0.4 },
        ];
        let mut out = [0.0f32; 4];
        write_device_samples::<f32>(&frames, 2, &mut out);
        assert_eq!(out, [0.1, 0.2, -0.3, 0.4]);
    }

    #[test]
    fn write_device_samples_mono_downmix_averages_channels() {
        let frames = vec![
            StereoFrame { l: 0.5, r: -0.5 },
            StereoFrame { l: 1.0, r: 1.0 },
        ];
        let mut out = [0.0f32; 2];
        write_device_samples::<f32>(&frames, 1, &mut out);
        assert_eq!(out, [0.0, 1.0]);
    }

    #[test]
    fn write_device_samples_i16_conversion() {
        let frames = vec![StereoFrame { l: 1.0, r: -1.0 }];
        let mut out = [0i16; 2];
        write_device_samples::<i16>(&frames, 2, &mut out);
        assert_eq!(out, [i16::MAX, -i16::MAX]);
    }

    #[test]
    fn write_device_samples_short_frames_pads_silence() {
        // Frames shorter than `out` — trailing samples should be silence.
        let frames = vec![StereoFrame { l: 0.5, r: -0.5 }];
        let mut out = [9.9f32; 4];
        write_device_samples::<f32>(&frames, 2, &mut out);
        assert_eq!(out, [0.5, -0.5, 0.0, 0.0]);
    }

    // ---- Resampler ----

    #[test]
    fn resampler_passthrough_on_equal_rates() {
        let mut rs = LinearResampler::new(48_000, 48_000);
        let input: Vec<_> = (0..8)
            .map(|i| StereoFrame {
                l: i as f32,
                r: -(i as f32),
            })
            .collect();
        let mut out = vec![StereoFrame::SILENCE; 8];
        let n = rs.resample(&input, &mut out);
        assert_eq!(n, 8);
        assert_eq!(out, input);
    }

    #[test]
    fn resampler_downsamples_48k_to_44k1_roughly() {
        let mut rs = LinearResampler::new(48_000, 44_100);
        let input: Vec<_> = (0..480)
            .map(|i| StereoFrame {
                l: (i as f32 / 48_000.0).sin(),
                r: (i as f32 / 48_000.0).sin(),
            })
            .collect();
        let mut out = vec![StereoFrame::SILENCE; 441];
        let n = rs.resample(&input, &mut out);
        assert!(n >= 440, "expected ~441 frames, got {n}");
        // No NaN/inf and values stay bounded (linear interp can't
        // exceed [-1, 1] of a sine input).
        for f in &out[..n] {
            assert!(f.l.is_finite() && f.r.is_finite());
            assert!(f.l.abs() <= 1.01 && f.r.abs() <= 1.01);
        }
    }

    #[test]
    fn resampler_state_carries_across_calls() {
        // Two halves of the same input through two pushes should
        // produce (approximately) the same output as a single push.
        let input: Vec<_> = (0..200)
            .map(|i| StereoFrame {
                l: i as f32 / 200.0,
                r: i as f32 / 200.0,
            })
            .collect();

        let mut rs_single = LinearResampler::new(48_000, 44_100);
        let mut single_out = vec![StereoFrame::SILENCE; 184];
        let n_single = rs_single.resample(&input, &mut single_out);

        let mut rs_split = LinearResampler::new(48_000, 44_100);
        let mut split_out_a = vec![StereoFrame::SILENCE; 92];
        let mut split_out_b = vec![StereoFrame::SILENCE; 92];
        let n_a = rs_split.resample(&input[..100], &mut split_out_a);
        let n_b = rs_split.resample(&input[100..], &mut split_out_b);

        // The totals should be within a couple of frames of each
        // other — not bit-identical because the split moment lands
        // on a different subsample position, but there shouldn't be
        // a cliff.
        let single_total = n_single;
        let split_total = n_a + n_b;
        assert!(
            (single_total as isize - split_total as isize).abs() <= 4,
            "single={single_total}, split={split_total}"
        );
    }

    // ---- Underrun accounting ----
    //
    // The callback-level underrun accounting is exercised through an
    // integration test in `tests/pcm_cpal_output_tests.rs` rather than
    // here, because it needs a live `PcmRingReader`.
}
