//! Symphonia-backed audio decoder.
//!
//! Unit 7a in the Slice 2 plan: a pure, blocking decoder function that
//! turns any [`MediaSource`] into a stream of [`StereoFrame`]s at a
//! caller-chosen sample rate. It does **no I/O** and **no async** — the
//! caller provides the `MediaSource` (a `Cursor<Vec<u8>>`, a `File`, or
//! Unit 7b's async→sync bridge) and a callback that receives contiguous
//! blocks of decoded frames.
//!
//! The scope boundary matters: Unit 7a is testable against an in-memory
//! WAV fixture with no network, no symphonia plumbing in the radio source,
//! and no `RadioSource::run` changes. Unit 7b later wires it to reqwest.
//!
//! # Downmix
//!
//! Symphonia can hand us mono, stereo, or surround. We only emit stereo:
//! - **1 ch (mono)**: duplicate to both L/R.
//! - **2 ch (stereo)**: passthrough.
//! - **3+ ch**: best-effort — take channels 0 and 1. A proper 5.1→stereo
//!   downmix matrix is future work; most internet radio is stereo or mono.
//!
//! # Resampling
//!
//! Between symphonia's source rate and the caller's target rate we run a
//! cheap linear interpolation with sample history carried across packets
//! so there's no click at packet boundaries. This is *intentionally*
//! minimal; Unit 7b's spec calls for a rubato upgrade if and when it
//! becomes audible. Linear is fine for 44.1→48 kHz at the bitrates radio
//! streams typically use.
//!
//! # Errors
//!
//! The decoder loop treats `DecodeError` as "skip this packet, keep going"
//! (a single corrupted frame shouldn't kill a multi-hour radio stream)
//! but surfaces `ResetRequired` and unrecoverable `IoError` to the caller
//! so they can reopen the upstream. `UnexpectedEof` from
//! `format.next_packet` is the normal end-of-stream signal and returns
//! `Ok(stats)`.

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{anyhow, Context, Result};
use clitunes_core::StereoFrame;
use symphonia::core::audio::{AudioBufferRef, SampleBuffer};
use symphonia::core::codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use tracing::{debug, trace, warn};

/// Metrics returned from a successful decode run. Tests and the radio
/// source use these to sanity-check that bytes actually flowed.
#[derive(Clone, Copy, Debug, Default)]
pub struct DecodeStats {
    pub packets_decoded: u64,
    pub packets_skipped: u64,
    pub frames_emitted: u64,
    pub source_sample_rate: u32,
    pub source_channels: u16,
}

/// Caller-visible config. Keeps the function signature small.
#[derive(Clone, Copy, Debug)]
pub struct DecodeConfig {
    /// Target sample rate the callback will receive. When the source
    /// matches this, no resampling is performed.
    pub target_sample_rate: u32,
    /// Optional file-extension hint to help the probe pick a demuxer
    /// (e.g. `"wav"`, `"mp3"`). `None` lets symphonia auto-detect.
    pub extension_hint: Option<&'static str>,
    /// Optional MIME-type hint to help the probe pick a demuxer
    /// (e.g. `"audio/mpeg"`). `None` is fine for container formats with
    /// a clean magic-byte signature.
    pub mime_hint: Option<&'static str>,
}

impl DecodeConfig {
    pub fn at_rate(target_sample_rate: u32) -> Self {
        Self {
            target_sample_rate,
            extension_hint: None,
            mime_hint: None,
        }
    }
}

/// Decode `source` to `on_frames`, running on the calling thread. Returns
/// [`DecodeStats`] on clean end-of-stream or when `stop` flips to `true`.
/// Surfaces `Err` only for unrecoverable errors (probe failure, reset
/// required, fatal IO).
///
/// The callback receives **non-empty** slices of stereo frames at
/// `cfg.target_sample_rate`. The decoder owns the resample state, so
/// slices across callback calls stitch together seamlessly.
pub fn decode_stream<M, F>(
    source: M,
    cfg: DecodeConfig,
    stop: &AtomicBool,
    mut on_frames: F,
) -> Result<DecodeStats>
where
    M: MediaSource + 'static,
    F: FnMut(&[StereoFrame]),
{
    let mss = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());

    let mut hint = Hint::new();
    if let Some(ext) = cfg.extension_hint {
        hint.with_extension(ext);
    }
    if let Some(mime) = cfg.mime_hint {
        hint.mime_type(mime);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .context("symphonia probe failed to identify format")?;

    let mut format: Box<dyn FormatReader> = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow!("no decodable audio track in stream"))?;
    let track_id = track.id;
    let source_rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| anyhow!("track has no sample rate"))?;
    let source_channels = track
        .codec_params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(2);

    let mut decoder: Box<dyn Decoder> = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .context("symphonia failed to construct decoder")?;

    debug!(
        source_rate,
        source_channels,
        target_rate = cfg.target_sample_rate,
        "decoder ready"
    );

    let mut stats = DecodeStats {
        source_sample_rate: source_rate,
        source_channels,
        ..Default::default()
    };

    let mut resampler = LinearResampler::new(source_rate, cfg.target_sample_rate);
    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    let mut stereo_scratch: Vec<StereoFrame> = Vec::with_capacity(4096);
    let mut out_scratch: Vec<StereoFrame> = Vec::with_capacity(4096);

    loop {
        if stop.load(Ordering::Relaxed) {
            debug!(?stats, "decode_stream: stop requested");
            return Ok(stats);
        }

        let packet = match format.next_packet() {
            Ok(pkt) => pkt,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                debug!(?stats, "decode_stream: clean EOF");
                // Final flush of any tail sample left in the resampler.
                resampler.flush(&mut out_scratch);
                if !out_scratch.is_empty() {
                    stats.frames_emitted += out_scratch.len() as u64;
                    on_frames(&out_scratch);
                    out_scratch.clear();
                }
                return Ok(stats);
            }
            Err(SymphoniaError::ResetRequired) => {
                return Err(anyhow!("symphonia requested decoder reset"));
            }
            Err(e) => return Err(anyhow!("format.next_packet failed: {e}")),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded: AudioBufferRef = match decoder.decode(&packet) {
            Ok(b) => b,
            Err(SymphoniaError::DecodeError(e)) => {
                warn!(error = %e, "decode error, skipping packet");
                stats.packets_skipped += 1;
                continue;
            }
            Err(SymphoniaError::IoError(e)) => {
                return Err(anyhow!("decoder IO error: {e}"));
            }
            Err(SymphoniaError::ResetRequired) => {
                return Err(anyhow!("symphonia requested decoder reset"));
            }
            Err(e) => return Err(anyhow!("decoder fatal error: {e}")),
        };

        if sample_buf.is_none() {
            let spec = *decoded.spec();
            let duration = decoded.capacity() as u64;
            sample_buf = Some(SampleBuffer::<f32>::new(duration, spec));
        }
        let sb = sample_buf.as_mut().expect("sample_buf populated above");
        sb.copy_interleaved_ref(decoded);
        let interleaved = sb.samples();
        if interleaved.is_empty() {
            continue;
        }

        downmix_to_stereo(interleaved, source_channels, &mut stereo_scratch);
        resampler.push(&stereo_scratch, &mut out_scratch);
        stereo_scratch.clear();

        stats.packets_decoded += 1;
        if !out_scratch.is_empty() {
            stats.frames_emitted += out_scratch.len() as u64;
            trace!(
                packet = stats.packets_decoded,
                frames = out_scratch.len(),
                "decode_stream: emitting frames"
            );
            on_frames(&out_scratch);
            out_scratch.clear();
        }
    }
}

/// Collapse an interleaved `[ch0, ch1, .., ch0, ch1, ..]` buffer into
/// [`StereoFrame`]s. Mono → duplicate. Stereo → passthrough. Surround →
/// front-L/front-R only (intentionally lossy — see module docs).
fn downmix_to_stereo(interleaved: &[f32], channels: u16, out: &mut Vec<StereoFrame>) {
    let channels = channels.max(1) as usize;
    if channels == 1 {
        out.reserve(interleaved.len());
        for &s in interleaved {
            out.push(StereoFrame { l: s, r: s });
        }
        return;
    }

    let frames = interleaved.len() / channels;
    out.reserve(frames);
    for frame in 0..frames {
        let base = frame * channels;
        let l = interleaved[base];
        let r = interleaved[base + 1];
        out.push(StereoFrame { l, r });
    }
}

/// Linear-interpolation resampler with cross-packet state. When
/// `src == dst` this is a pure passthrough (no allocation, no math).
///
/// The math: we walk an output cursor `pos_in` in source-sample space
/// (`pos_in += src/dst` per output frame). For each output frame we
/// interpolate between `prev` (the last sample of the *previous* push)
/// and the current input buffer. The trailing sample of the current
/// input becomes `prev` for the next push, guaranteeing phase continuity.
struct LinearResampler {
    src: u32,
    dst: u32,
    /// Subsample fraction of the next output index in source space,
    /// measured from the start of the *next* push. `0.0 <= pos_in < 1.0`
    /// once initialised; a larger value means we skip input frames.
    pos_in: f64,
    /// The trailing frame of the last push, used as the `from` endpoint
    /// of the first interpolation in the next push. `None` before the
    /// first push.
    prev: Option<StereoFrame>,
}

impl LinearResampler {
    fn new(src: u32, dst: u32) -> Self {
        Self {
            src,
            dst,
            pos_in: 0.0,
            prev: None,
        }
    }

    fn ratio(&self) -> f64 {
        self.src as f64 / self.dst as f64
    }

    fn push(&mut self, input: &[StereoFrame], out: &mut Vec<StereoFrame>) {
        if input.is_empty() {
            return;
        }
        if self.src == self.dst {
            out.extend_from_slice(input);
            self.prev = input.last().copied();
            return;
        }

        let ratio = self.ratio();
        // `pos` is measured in source-space indices where index 0 is
        // `self.prev` and index 1 is `input[0]`, 2 is `input[1]`, etc.
        // Start position carries across pushes via `self.pos_in`.
        let mut pos = self.pos_in;
        let input_len = input.len() as f64;
        // Upper bound is `input_len` (inclusive-of-last-sample). When we
        // reach that we stop and stash the leftover fraction for the next
        // push.
        while pos < input_len {
            let i0 = pos.floor() as isize;
            let frac = (pos - pos.floor()) as f32;
            let from = if i0 <= 0 {
                // Source index 0 is `prev`. If we have no prev yet, seed
                // it from input[0] so we emit silence-free output.
                self.prev.unwrap_or(input[0])
            } else {
                input[(i0 - 1) as usize]
            };
            let to_idx = i0.max(0) as usize;
            if to_idx >= input.len() {
                break;
            }
            let to = input[to_idx];
            out.push(StereoFrame {
                l: from.l + (to.l - from.l) * frac,
                r: from.r + (to.r - from.r) * frac,
            });
            pos += ratio;
        }
        // Stash leftover for the next push: relative to the end of the
        // current input, so we subtract input_len.
        self.pos_in = pos - input_len;
        self.prev = input.last().copied();
    }

    /// Flush a single trailing sample if the caller is mid-interpolation
    /// at EOF. Currently a no-op because `push` never holds decoded
    /// frames — the leftover is purely a fractional cursor. Kept as a
    /// hook in case a higher-quality resampler lands here.
    fn flush(&mut self, _out: &mut [StereoFrame]) {}
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read, Seek, SeekFrom};
    use std::sync::atomic::AtomicBool;

    /// Synthesize a 16-bit PCM WAV file in memory. Writes a single-chunk
    /// RIFF file containing `samples` interleaved by `channels` at
    /// `sample_rate`. Uses s16le because it's the most widely supported
    /// and symphonia's `pcm` feature decodes it out of the box.
    fn build_wav_s16(sample_rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let bits_per_sample: u16 = 16;
        let byte_rate = sample_rate * channels as u32 * bits_per_sample as u32 / 8;
        let block_align = channels * bits_per_sample / 8;
        let data_size: u32 = (samples.len() * 2) as u32;
        let riff_size: u32 = 36 + data_size;

        let mut out = Vec::with_capacity(44 + data_size as usize);
        // RIFF header
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&riff_size.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        // fmt chunk
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM format
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&byte_rate.to_le_bytes());
        out.extend_from_slice(&block_align.to_le_bytes());
        out.extend_from_slice(&bits_per_sample.to_le_bytes());
        // data chunk
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_size.to_le_bytes());
        for s in samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    /// Build a sine-wave s16 sample buffer at `freq` Hz for `duration`
    /// seconds. `channels == 2` duplicates the sine on both channels,
    /// `channels == 1` writes mono.
    fn sine_samples(sample_rate: u32, channels: u16, freq: f32, duration_s: f32) -> Vec<i16> {
        let total = (sample_rate as f32 * duration_s) as usize;
        let mut out = Vec::with_capacity(total * channels as usize);
        let two_pi = std::f32::consts::TAU;
        for n in 0..total {
            let t = n as f32 / sample_rate as f32;
            let v = (two_pi * freq * t).sin() * 0.5;
            let q = (v * i16::MAX as f32) as i16;
            for _ in 0..channels {
                out.push(q);
            }
        }
        out
    }

    /// Cursor newtype that implements `Read + Seek` and therefore
    /// `MediaSource`. Required because `Cursor<Vec<u8>>` itself isn't
    /// blanket `MediaSource` in symphonia 0.5.
    struct MemMedia(Cursor<Vec<u8>>);
    impl Read for MemMedia {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(buf)
        }
    }
    impl Seek for MemMedia {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.0.seek(pos)
        }
    }
    impl MediaSource for MemMedia {
        fn is_seekable(&self) -> bool {
            true
        }
        fn byte_len(&self) -> Option<u64> {
            Some(self.0.get_ref().len() as u64)
        }
    }

    /// Root-mean-square of a stereo frame slice, averaged across L and R.
    fn rms(frames: &[StereoFrame]) -> f32 {
        if frames.is_empty() {
            return 0.0;
        }
        let mut acc = 0.0f64;
        for f in frames {
            acc += (f.l as f64).powi(2) + (f.r as f64).powi(2);
        }
        ((acc / (frames.len() as f64 * 2.0)) as f32).sqrt()
    }

    #[test]
    fn decodes_stereo_48k_wav_at_native_rate() {
        let wav = build_wav_s16(48_000, 2, &sine_samples(48_000, 2, 440.0, 0.2));
        let media = MemMedia(Cursor::new(wav));

        let stop = AtomicBool::new(false);
        let mut collected = Vec::<StereoFrame>::new();
        let stats = decode_stream(
            media,
            DecodeConfig {
                target_sample_rate: 48_000,
                extension_hint: Some("wav"),
                mime_hint: None,
            },
            &stop,
            |frames| collected.extend_from_slice(frames),
        )
        .expect("decode");

        assert_eq!(stats.source_sample_rate, 48_000);
        assert_eq!(stats.source_channels, 2);
        assert!(
            stats.frames_emitted as usize >= 48_000 / 5 - 100,
            "expected roughly {} frames, got {}",
            48_000 / 5,
            stats.frames_emitted
        );
        assert!(rms(&collected) > 0.1, "rms too low: {}", rms(&collected));
        // No NaNs, no infs.
        for (i, f) in collected.iter().enumerate() {
            assert!(f.l.is_finite() && f.r.is_finite(), "bad frame {i}: {f:?}");
        }
    }

    #[test]
    fn resamples_44k1_stereo_to_48k() {
        let wav = build_wav_s16(44_100, 2, &sine_samples(44_100, 2, 440.0, 0.25));
        let media = MemMedia(Cursor::new(wav));

        let stop = AtomicBool::new(false);
        let mut collected = Vec::<StereoFrame>::new();
        let stats = decode_stream(
            media,
            DecodeConfig {
                target_sample_rate: 48_000,
                extension_hint: Some("wav"),
                mime_hint: None,
            },
            &stop,
            |frames| collected.extend_from_slice(frames),
        )
        .expect("decode");

        assert_eq!(stats.source_sample_rate, 44_100);
        let expected = (0.25f32 * 48_000.0) as usize;
        // Linear resampler + packet boundaries → ±50 frames slop.
        assert!(
            (collected.len() as i64 - expected as i64).abs() < 200,
            "expected ~{} frames, got {}",
            expected,
            collected.len()
        );
        assert!(rms(&collected) > 0.1);
        for f in &collected {
            assert!(f.l.is_finite() && f.r.is_finite());
        }
    }

    #[test]
    fn mono_source_duplicates_to_both_channels() {
        let wav = build_wav_s16(48_000, 1, &sine_samples(48_000, 1, 220.0, 0.1));
        let media = MemMedia(Cursor::new(wav));

        let stop = AtomicBool::new(false);
        let mut collected = Vec::<StereoFrame>::new();
        let stats = decode_stream(
            media,
            DecodeConfig {
                target_sample_rate: 48_000,
                extension_hint: Some("wav"),
                mime_hint: None,
            },
            &stop,
            |frames| collected.extend_from_slice(frames),
        )
        .expect("decode");

        assert_eq!(stats.source_channels, 1);
        assert!(!collected.is_empty());
        // Every frame has identical L/R since mono is duplicated.
        for f in &collected {
            assert_eq!(f.l, f.r);
        }
    }

    #[test]
    fn stop_flag_ends_decode_early() {
        let wav = build_wav_s16(48_000, 2, &sine_samples(48_000, 2, 440.0, 1.0));
        let media = MemMedia(Cursor::new(wav));

        let stop = AtomicBool::new(true); // already set — first iteration bails
        let mut collected = Vec::<StereoFrame>::new();
        let stats = decode_stream(
            media,
            DecodeConfig {
                target_sample_rate: 48_000,
                extension_hint: Some("wav"),
                mime_hint: None,
            },
            &stop,
            |frames| collected.extend_from_slice(frames),
        )
        .expect("decode");

        assert_eq!(stats.packets_decoded, 0);
        assert!(collected.is_empty());
    }

    #[test]
    fn linear_resampler_passthrough_on_equal_rates() {
        let mut rs = LinearResampler::new(48_000, 48_000);
        let input = vec![
            StereoFrame { l: 0.1, r: 0.2 },
            StereoFrame { l: 0.3, r: 0.4 },
            StereoFrame { l: 0.5, r: 0.6 },
        ];
        let mut out = Vec::new();
        rs.push(&input, &mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].l, 0.1);
        assert_eq!(out[2].r, 0.6);
    }

    #[test]
    fn linear_resampler_upsamples_2x() {
        let mut rs = LinearResampler::new(1, 2);
        let input = vec![
            StereoFrame { l: 0.0, r: 0.0 },
            StereoFrame { l: 1.0, r: 1.0 },
            StereoFrame { l: 0.0, r: 0.0 },
        ];
        let mut out = Vec::new();
        rs.push(&input, &mut out);
        // 3 in, ratio 0.5 → roughly 6 out.
        assert!(out.len() >= 5 && out.len() <= 7, "got {}", out.len());
        // Values must stay bounded and interpolated, not clipped.
        for f in &out {
            assert!((-1.01..=1.01).contains(&f.l));
        }
    }
}
