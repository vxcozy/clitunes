mod folder_scan;
mod queue;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;

use crate::audio::ring::PcmWriter;
use crate::sources::symphonia_decode::{decode_stream, DecodeConfig};
use crate::sources::Source;

use self::folder_scan::scan_paths;
use self::queue::Queue;

pub use self::queue::Queue as LocalQueue;

pub struct LocalSource {
    queue: Queue,
    sample_rate: u32,
}

impl LocalSource {
    pub fn new(paths: Vec<PathBuf>, sample_rate: u32) -> Result<Self> {
        let (tracks, warning) = scan_paths(&paths, 50_000);
        if let Some(w) = &warning {
            tracing::warn!(target: "clitunes", warning = %w, "folder scan capped");
        }
        if tracks.is_empty() {
            anyhow::bail!("no playable files found in provided paths");
        }
        tracing::info!(target: "clitunes", count = tracks.len(), "local source: queued tracks");
        let queue = Queue::new(tracks);
        Ok(Self { queue, sample_rate })
    }
}

impl Source for LocalSource {
    fn name(&self) -> &str {
        "local"
    }

    fn run(&mut self, writer: &mut dyn PcmWriter, stop: &AtomicBool) {
        while let Some(track) = self.queue.next() {
            if stop.load(Ordering::Relaxed) {
                return;
            }

            let path = track.path.clone();
            tracing::info!(
                target: "clitunes",
                path = %path.display(),
                title = ?track.display_title(),
                "playing local track"
            );

            let file = match std::fs::File::open(&path) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!(
                        target: "clitunes",
                        error = %e,
                        path = %path.display(),
                        "failed to open file, skipping"
                    );
                    continue;
                }
            };

            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase());
            let ext_hint: Option<&'static str> = match ext.as_deref() {
                Some("mp3") => Some("mp3"),
                Some("flac") => Some("flac"),
                Some("ogg") | Some("opus") => Some("ogg"),
                Some("wav") => Some("wav"),
                Some("m4a") | Some("aac") => Some("m4a"),
                _ => None,
            };

            let cfg = DecodeConfig {
                target_sample_rate: self.sample_rate,
                extension_hint: ext_hint,
                mime_hint: None,
            };

            let media = FileMediaSource(file);
            match decode_stream(media, cfg, stop, |frames| {
                writer.write(frames);
            }) {
                Ok(stats) => {
                    tracing::debug!(
                        target: "clitunes",
                        path = %path.display(),
                        frames = stats.frames_emitted,
                        "track finished"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        target: "clitunes",
                        error = %e,
                        path = %path.display(),
                        "decode error, skipping track"
                    );
                }
            }
        }
        tracing::info!(target: "clitunes", "local source: queue exhausted");
    }
}

/// File-backed MediaSource for symphonia. Seekable, reports file length.
struct FileMediaSource(std::fs::File);

impl std::io::Read for FileMediaSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl std::io::Seek for FileMediaSource {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.0.seek(pos)
    }
}

impl symphonia::core::io::MediaSource for FileMediaSource {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        self.0.metadata().ok().map(|m| m.len())
    }
}
