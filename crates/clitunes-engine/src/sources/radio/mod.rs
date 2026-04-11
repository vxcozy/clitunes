//! Radio source: discovery + station database lookup + streaming.
//!
//! Unit 5 ties three submodules into the public surface that Slice 2 needs:
//!
//! - [`discovery`] — DNS SRV lookup of `_api._tcp.radio-browser.info` with
//!   cache + baked-in fallback chain.
//! - [`station_db`] — HTTP client that resolves a curated station UUID
//!   (or name search) to a `Station` with sanitised free-text fields.
//! - [`streamer`] — opens the resolved stream URL with `Icy-MetaData: 1`
//!   and returns a byte stream + headers.
//!
//! Unit 6 added [`icy_stream`]: the pure state-machine that splits
//! interleaved audio + metadata blocks and publishes now-playing events.
//!
//! Unit 7b wires it all together. [`IcyMediaSource`] (the async→sync
//! bridge in [`icy_media_source`]) turns an `mpsc<Vec<u8>>` of audio-only
//! bytes into a blocking `MediaSource`, and [`RadioSource::run`] spawns a
//! tokio network thread that pumps chunks into that bridge while the
//! main thread decodes with symphonia and writes `StereoFrame`s into the
//! PCM ring.
//!
//! # The `Source` trait impl
//!
//! `RadioSource` implements [`Source`](super::Source) so the control
//! layer can drive it like any other producer. With Unit 7 shipped, it
//! now writes **real decoded PCM** from the radio stream (not silence).
//! All bytes flow through the ICY parser first so metadata still
//! reaches the now-playing broadcast bus.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clitunes_core::{NowPlaying, NowPlayingEvent, PcmFormat, Station};
use tokio::runtime::Builder;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use super::Source;

pub mod discovery;
pub mod icy_media_source;
pub mod icy_stream;
pub mod station_db;
pub mod streamer;

pub use discovery::{
    discover_mirrors, discover_with_paths, DiscoveredMirrors, Mirror, MirrorSource,
};
pub use icy_media_source::{extension_hint_from_content_type, IcyMediaSource};
pub use icy_stream::{
    extract_stream_title, extract_stream_url, parse_metadata_block, IcyField, IcyParser,
    ParsedChunk, METADATA_BLOCK_STRIDE,
};
pub use station_db::StationDb;
pub use streamer::{OpenedStream, RadioStreamer, StreamHeaders};

#[cfg(feature = "audio")]
use crate::audio::PcmRingWriter;

#[cfg(feature = "decode")]
use crate::sources::symphonia_decode::{decode_stream, DecodeConfig};

/// Capacity of the now-playing broadcast channel. Eight is enough headroom
/// for several concurrent panes to receive every event without being slow
/// subscribers; broadcast uses a ring buffer so exceeding the capacity only
/// causes individual slow consumers to `Lagged`, not loss for everyone.
pub const NOW_PLAYING_CHANNEL_CAPACITY: usize = 8;

/// Capacity of the async→sync audio channel between the network thread
/// and the decoder. 64 chunks × ~8 KB typical Icecast chunk ≈ 500 KB of
/// slack, which is several seconds at 128 kbps — enough to ride out a
/// transient decoder stall but tight enough that a runaway producer
/// can't grow memory unbounded.
const AUDIO_CHANNEL_CAPACITY: usize = 64;

/// How long the main thread waits for the network thread to deliver
/// stream headers before giving up. Aligned with `CONNECT_TIMEOUT` plus
/// generous slack for the ICY handshake.
const HEADER_WAIT_TIMEOUT: Duration = Duration::from_secs(20);

/// Configuration handed to a `RadioSource` at construction time. The
/// control layer (Slice 3) builds this from picker selection + state.toml.
#[derive(Clone, Debug)]
pub struct RadioConfig {
    pub station: Station,
    pub format: PcmFormat,
}

impl RadioConfig {
    pub fn new(station: Station, format: PcmFormat) -> Self {
        Self { station, format }
    }
}

/// One-shot resolver: discover mirrors and look up a station UUID. This is
/// what the picker calls when the user confirms a curated slot.
pub async fn resolve_station(uuid: &str) -> Result<Station> {
    let discovered = discover_mirrors().await?;
    info!(
        source = ?discovered.source,
        count = discovered.mirrors.len(),
        "mirror set discovered"
    );
    let db = StationDb::new(discovered.mirrors)?;
    db.get_station_by_uuid(uuid).await
}

/// Blocking variant of [`resolve_station`] for sync callers (the clitunes
/// binary's `main`). Builds a throwaway current-thread runtime internally
/// so the caller doesn't need to depend on tokio.
pub fn resolve_station_blocking(uuid: &str) -> Result<Station> {
    let rt = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| anyhow::anyhow!("tokio runtime build failed: {e}"))?;
    rt.block_on(resolve_station(uuid))
}

/// `Source` implementation that drives a radio stream. Feature-gated
/// behind `decode` because the whole point is to hand decoded PCM to the
/// ring — without symphonia this source has nothing useful to do.
pub struct RadioSource {
    config: RadioConfig,
    streamer: RadioStreamer,
    now_playing_tx: broadcast::Sender<NowPlayingEvent>,
}

impl RadioSource {
    pub fn new(config: RadioConfig) -> Result<Self> {
        let (now_playing_tx, _) = broadcast::channel(NOW_PLAYING_CHANNEL_CAPACITY);
        Ok(Self {
            config,
            streamer: RadioStreamer::new()?,
            now_playing_tx,
        })
    }

    /// Subscribe to the now-playing event bus. Returns a fresh receiver;
    /// safe to call multiple times for multiple UI panes. Late subscribers
    /// miss events that were emitted before they subscribed — they should
    /// still render empty until the next event arrives, which in practice
    /// lands within a few seconds once the next metadata block arrives.
    pub fn subscribe_now_playing(&self) -> broadcast::Receiver<NowPlayingEvent> {
        self.now_playing_tx.subscribe()
    }

    /// A cheap clone of the sender handle for tests or other publishers.
    pub fn now_playing_sender(&self) -> broadcast::Sender<NowPlayingEvent> {
        self.now_playing_tx.clone()
    }
}

/// Build a [`NowPlaying`] snapshot from the HTTP response headers. Headers
/// are already sanitized by [`StreamHeaders::from_response`]; this is just
/// a field-by-field copy.
fn station_info_from_headers(headers: &StreamHeaders) -> NowPlaying {
    NowPlaying {
        station_name: headers.icy_name.clone(),
        station_genre: headers.icy_genre.clone(),
        station_description: headers.icy_description.clone(),
        station_bitrate_kbps: headers
            .icy_br
            .as_deref()
            .and_then(|s| s.trim().parse::<u32>().ok()),
        ..Default::default()
    }
}

#[cfg(feature = "decode")]
impl Source for RadioSource {
    fn name(&self) -> &str {
        "radio"
    }

    fn run(&mut self, writer: &mut PcmRingWriter, stop: &AtomicBool) {
        let url = self.config.station.url_resolved.clone();
        let station_name = self.config.station.name.clone();
        let format = self.config.format;
        let streamer = self.streamer.clone();
        let now_playing_tx = self.now_playing_tx.clone();

        // Audio pipe: network → decoder. Bounded so a slow decoder
        // eventually back-pressures the network loop instead of OOMing.
        let (audio_tx, audio_rx) = sync_channel::<Vec<u8>>(AUDIO_CHANNEL_CAPACITY);

        // Header handoff: 1-slot channel so the main thread can wait on
        // `StreamHeaders` before building the decoder config.
        let (hdr_tx, hdr_rx) = sync_channel::<StreamHeaders>(1);

        // Mirror of the outer stop flag. IcyMediaSource needs 'static,
        // so we can't hand it `&AtomicBool` directly — we clone an Arc
        // that a tiny watcher thread flips when the outer stop trips.
        let inner_stop = Arc::new(AtomicBool::new(false));

        std::thread::scope(|scope| {
            // Watcher: mirror outer stop → inner stop. Bounded polling
            // latency of 50ms means a quit keystroke reaches the decoder
            // well within a frame.
            let mirror_inner = Arc::clone(&inner_stop);
            scope.spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                mirror_inner.store(true, Ordering::SeqCst);
            });

            // Network thread: owns the tokio runtime, pumps reqwest
            // chunks through the ICY parser, pushes audio to the mpsc,
            // publishes metadata on the broadcast.
            let net_inner_stop = Arc::clone(&inner_stop);
            let net = scope.spawn(move || {
                let rt = match Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt,
                    Err(e) => {
                        error!(error = %e, "radio source: failed to build tokio runtime");
                        return;
                    }
                };
                rt.block_on(network_loop(
                    streamer,
                    url,
                    station_name,
                    now_playing_tx,
                    hdr_tx,
                    audio_tx,
                    net_inner_stop,
                ));
            });

            // Wait for headers so we can build an extension hint. We
            // poll the inner stop so a quit during connect bails out.
            let headers = match wait_for_headers(&hdr_rx, &inner_stop) {
                Some(h) => h,
                None => {
                    let _ = net.join();
                    return;
                }
            };
            info!(content_type = ?headers.content_type, "radio: headers received, starting decoder");

            let cfg = DecodeConfig {
                target_sample_rate: format.sample_rate,
                extension_hint: extension_hint_from_content_type(headers.content_type.as_deref()),
                mime_hint: None,
            };

            let source = IcyMediaSource::new(audio_rx, Arc::clone(&inner_stop));
            match decode_stream(source, cfg, &inner_stop, |frames| {
                writer.write(frames);
            }) {
                Ok(stats) => info!(
                    frames = stats.frames_emitted,
                    packets = stats.packets_decoded,
                    skipped = stats.packets_skipped,
                    "radio decoder clean exit"
                ),
                Err(e) => error!(error = %e, "radio decoder error"),
            }

            // Ensure the outer stop reaches the network loop even if the
            // decoder exited first (e.g. fatal ResetRequired).
            inner_stop.store(true, Ordering::SeqCst);
            let _ = net.join();
        });
    }
}

/// Poll the header channel while watching the stop flag. Returns `None`
/// if stop trips, the channel closes before a header arrives, or the
/// total wait exceeds [`HEADER_WAIT_TIMEOUT`].
fn wait_for_headers(
    hdr_rx: &std::sync::mpsc::Receiver<StreamHeaders>,
    stop: &Arc<AtomicBool>,
) -> Option<StreamHeaders> {
    let deadline = std::time::Instant::now() + HEADER_WAIT_TIMEOUT;
    loop {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        if std::time::Instant::now() >= deadline {
            error!("radio: timed out waiting for stream headers");
            return None;
        }
        match hdr_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(h) => return Some(h),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                error!("radio: network thread closed header channel");
                return None;
            }
        }
    }
}

/// Network pump. Opens the stream, forwards headers once, then loops
/// pulling `Bytes` chunks, running them through the ICY parser,
/// publishing metadata events, and pushing audio bytes into the
/// async→sync `mpsc`. Reconnects inline (same logic as the pre-7b
/// version) so a mid-stream drop is invisible to the decoder.
#[cfg(feature = "decode")]
async fn network_loop(
    streamer: RadioStreamer,
    url: String,
    station_name: String,
    now_playing_tx: broadcast::Sender<NowPlayingEvent>,
    hdr_tx: SyncSender<StreamHeaders>,
    audio_tx: SyncSender<Vec<u8>>,
    stop: Arc<AtomicBool>,
) {
    info!(station = %station_name, %url, "opening radio stream");
    let mut stream = match streamer.open(&url).await {
        Ok(s) => s,
        Err(e) => {
            error!(error = %e, "radio source: open failed; bailing");
            return;
        }
    };
    info!(headers = ?stream.headers, "radio stream open");

    // Send headers to the main thread so it can build the decoder config.
    // This is a one-shot; if the receiver is gone, we bail early.
    if hdr_tx.send(stream.headers.clone()).is_err() {
        warn!("radio: header receiver dropped before we could send");
        return;
    }

    let mut now_playing = station_info_from_headers(&stream.headers);
    if !now_playing.is_empty() {
        let _ = now_playing_tx.send(NowPlayingEvent::StationInfo(now_playing.clone()));
    }

    let mut parser = IcyParser::new(stream.headers.icy_metaint);
    let mut last_title: Option<String> = None;
    let mut reconnect_attempt: u32 = 0;

    loop {
        if stop.load(Ordering::Relaxed) {
            info!("radio: network loop stop requested");
            return;
        }

        match futures_util::StreamExt::next(&mut stream.bytes).await {
            Some(Ok(c)) => {
                let parsed = parser.push(c);
                // Publish each metadata block as a NowPlayingEvent if
                // the title actually changed.
                for block in parsed.metadata_blocks {
                    let title = extract_stream_title(&block);
                    let url_meta = extract_stream_url(&block);
                    if let Some(t) = &title {
                        if t.is_empty() || last_title.as_deref() == Some(t.as_str()) {
                            continue;
                        }
                        last_title = Some(t.clone());
                        now_playing.track_title = Some(t.clone());
                        now_playing.track_url = url_meta;
                        info!(track = %t, "icy track change");
                        let _ = now_playing_tx
                            .send(NowPlayingEvent::TrackChanged(now_playing.clone()));
                    }
                }

                // Forward audio bytes to the decoder. If the decoder is
                // slow, sync_channel::send blocks, which is fine — tokio's
                // current_thread executor has nothing else to run on this
                // thread anyway.
                if !parsed.audio.is_empty()
                    && push_audio(&audio_tx, parsed.audio, &stop).is_err()
                {
                    info!("radio: decoder receiver closed; network loop exiting");
                    return;
                }

                if reconnect_attempt > 0 {
                    let _ = now_playing_tx.send(NowPlayingEvent::Reconnected);
                    reconnect_attempt = 0;
                }
            }
            Some(Err(e)) => {
                warn!(error = %e, "radio stream chunk error; reopening");
                reconnect_attempt = reconnect_attempt.saturating_add(1);
                let _ = now_playing_tx.send(NowPlayingEvent::Reconnecting {
                    attempt: reconnect_attempt,
                });
                match streamer.open(&url).await {
                    Ok(s) => {
                        stream = s;
                        parser = IcyParser::new(stream.headers.icy_metaint);
                    }
                    Err(e2) => {
                        error!(error = %e2, "radio source: reopen failed; bailing");
                        return;
                    }
                }
            }
            None => {
                warn!("radio stream ended; reopening");
                reconnect_attempt = reconnect_attempt.saturating_add(1);
                let _ = now_playing_tx.send(NowPlayingEvent::Reconnecting {
                    attempt: reconnect_attempt,
                });
                match streamer.open(&url).await {
                    Ok(s) => {
                        stream = s;
                        parser = IcyParser::new(stream.headers.icy_metaint);
                    }
                    Err(e2) => {
                        error!(error = %e2, "radio source: reopen failed; bailing");
                        return;
                    }
                }
            }
        }
    }
}

/// Push an audio chunk onto the decoder mpsc. When the queue is full,
/// retry with a short sleep while watching the stop flag so the network
/// loop doesn't deadlock on a stalled decoder. Returns `Err(())` if the
/// decoder dropped the receiver, signalling the network loop to exit.
#[cfg(feature = "decode")]
fn push_audio(tx: &SyncSender<Vec<u8>>, audio: Vec<u8>, stop: &Arc<AtomicBool>) -> Result<(), ()> {
    let mut payload = Some(audio);
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err(());
        }
        // try_send lets us keep polling stop while the queue is full.
        let chunk = payload.take().expect("payload populated");
        match tx.try_send(chunk) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Full(returned)) => {
                payload = Some(returned);
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(TrySendError::Disconnected(_)) => return Err(()),
        }
    }
}
