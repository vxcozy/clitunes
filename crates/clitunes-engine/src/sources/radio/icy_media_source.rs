//! Async→sync bridge: turn a `std::sync::mpsc::Receiver<Vec<u8>>` of
//! audio-only bytes into a blocking `MediaSource` that symphonia can
//! drive from the decode thread.
//!
//! Unit 7b in the Slice 2 plan. The radio network thread owns a tokio
//! runtime, pulls async `Bytes` chunks from reqwest, runs them through
//! the ICY parser, publishes metadata to the now-playing broadcast, and
//! pushes the audio-only remnants into the `mpsc`. This module is the
//! *other* side of that pipe: it wraps the receiver in a `Read + Seek`
//! implementation (radio is never seekable) so symphonia sees a normal
//! byte stream.
//!
//! # Stop semantics
//!
//! Symphonia's `MediaSource: 'static` bound means we can't borrow an
//! outer `&AtomicBool`. Instead the caller wires in an `Arc<AtomicBool>`
//! that mirrors the outer stop flag. When it trips, the blocking `read`
//! returns `Ok(0)` (EOF) on the next timeout tick so the decoder unwinds
//! cleanly instead of hanging on `recv()`.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use symphonia::core::io::MediaSource;

/// Poll interval for the stop flag while waiting on a chunk. Chosen so
/// a shutdown request is visible to the decoder in ≤200ms without
/// burning CPU on a hot wait loop.
const STOP_POLL: Duration = Duration::from_millis(200);

/// Sync `MediaSource` that reads audio bytes from an `mpsc` fed by the
/// radio network thread. Never seekable, unknown length.
///
/// The receiver lives behind a `Mutex` because symphonia's `MediaSource`
/// bound requires `Send + Sync`, and `std::sync::mpsc::Receiver` is
/// `!Sync`. In practice the mutex is uncontended — symphonia only ever
/// reads from one thread — so the lock cost is negligible.
pub struct IcyMediaSource {
    rx: Mutex<Receiver<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    /// Overflow from the last pulled chunk when the caller's `out` buffer
    /// was smaller than the chunk. Drained before touching `rx` again.
    overflow: Vec<u8>,
    /// Once `true`, all reads return `Ok(0)` (EOF) forever.
    eof: bool,
}

impl IcyMediaSource {
    pub fn new(rx: Receiver<Vec<u8>>, stop: Arc<AtomicBool>) -> Self {
        Self {
            rx: Mutex::new(rx),
            stop,
            overflow: Vec::new(),
            eof: false,
        }
    }

    /// Drain as much of `self.overflow` into `out` as fits. Returns the
    /// number of bytes copied.
    fn drain_overflow(&mut self, out: &mut [u8]) -> usize {
        if self.overflow.is_empty() || out.is_empty() {
            return 0;
        }
        let n = self.overflow.len().min(out.len());
        out[..n].copy_from_slice(&self.overflow[..n]);
        // Preserve any leftover tail.
        self.overflow.drain(..n);
        n
    }
}

impl Read for IcyMediaSource {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.eof {
            return Ok(0);
        }

        // Fast path: serve leftover from a previous big chunk.
        let from_overflow = self.drain_overflow(out);
        if from_overflow > 0 {
            return Ok(from_overflow);
        }

        // Block on the channel, but wake every STOP_POLL so a shutdown
        // flips us to EOF without hanging.
        loop {
            if self.stop.load(Ordering::Relaxed) {
                self.eof = true;
                return Ok(0);
            }
            let recv_result = {
                let guard = self
                    .rx
                    .lock()
                    .map_err(|_| io::Error::other("icy media source mutex poisoned"))?;
                guard.recv_timeout(STOP_POLL)
            };
            match recv_result {
                Ok(chunk) => {
                    if chunk.is_empty() {
                        continue;
                    }
                    let n = chunk.len().min(out.len());
                    out[..n].copy_from_slice(&chunk[..n]);
                    if n < chunk.len() {
                        self.overflow.extend_from_slice(&chunk[n..]);
                    }
                    return Ok(n);
                }
                Err(RecvTimeoutError::Timeout) => {
                    // Loop — re-check stop, try recv again.
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    // Network thread exited; no more bytes will arrive.
                    self.eof = true;
                    return Ok(0);
                }
            }
        }
    }
}

impl Seek for IcyMediaSource {
    fn seek(&mut self, _pos: SeekFrom) -> io::Result<u64> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "icy stream is not seekable",
        ))
    }
}

impl MediaSource for IcyMediaSource {
    fn is_seekable(&self) -> bool {
        false
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// Map an HTTP `Content-Type` header to the file-extension hint
/// symphonia uses to pick a demuxer. `None` lets symphonia fall back to
/// magic-byte sniffing, which is fine for mp3/flac/ogg but flaky for
/// AAC-in-ADTS where there's no unambiguous magic.
pub fn extension_hint_from_content_type(content_type: Option<&str>) -> Option<&'static str> {
    let raw = content_type?;
    // Trim params like `; charset=utf-8`.
    let main = raw.split(';').next()?.trim().to_ascii_lowercase();
    match main.as_str() {
        "audio/mpeg" | "audio/mp3" | "audio/mpeg3" | "audio/x-mpeg" => Some("mp3"),
        "audio/aac" | "audio/aacp" | "audio/x-aac" => Some("aac"),
        "audio/ogg" | "application/ogg" | "audio/vorbis" => Some("ogg"),
        "audio/flac" | "audio/x-flac" => Some("flac"),
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" => Some("m4a"),
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some("wav"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn reads_a_single_chunk_into_a_small_buf() {
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(4);
        tx.send(vec![1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        drop(tx); // close channel so final read returns EOF

        let stop = Arc::new(AtomicBool::new(false));
        let mut src = IcyMediaSource::new(rx, stop);

        let mut buf = [0u8; 3];
        assert_eq!(src.read(&mut buf).unwrap(), 3);
        assert_eq!(&buf, &[1, 2, 3]);
        assert_eq!(src.read(&mut buf).unwrap(), 3);
        assert_eq!(&buf, &[4, 5, 6]);
        // Last 2 bytes plus a stale slot.
        let mut big = [0u8; 4];
        assert_eq!(src.read(&mut big).unwrap(), 2);
        assert_eq!(&big[..2], &[7, 8]);
        // Channel closed → EOF.
        assert_eq!(src.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn stitches_multiple_chunks() {
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(8);
        tx.send(vec![1, 2]).unwrap();
        tx.send(vec![3, 4, 5]).unwrap();
        tx.send(vec![6]).unwrap();
        drop(tx);

        let stop = Arc::new(AtomicBool::new(false));
        let mut src = IcyMediaSource::new(rx, stop);

        let mut all = Vec::new();
        loop {
            let mut buf = [0u8; 16];
            match src.read(&mut buf).unwrap() {
                0 => break,
                n => all.extend_from_slice(&buf[..n]),
            }
        }
        assert_eq!(all, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn stop_flag_returns_eof_even_with_pending_sender() {
        // Sender held open and idle; without the stop flag this would
        // block forever.
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(1);
        let stop = Arc::new(AtomicBool::new(false));
        let mut src = IcyMediaSource::new(rx, Arc::clone(&stop));

        // Flip stop from another thread shortly after the read starts.
        thread::spawn({
            let stop = Arc::clone(&stop);
            move || {
                thread::sleep(Duration::from_millis(50));
                stop.store(true, Ordering::SeqCst);
            }
        });

        let mut buf = [0u8; 8];
        let n = src.read(&mut buf).unwrap();
        assert_eq!(n, 0, "expected EOF after stop flag flipped");
        // Subsequent reads also EOF.
        assert_eq!(src.read(&mut buf).unwrap(), 0);

        drop(tx);
    }

    #[test]
    fn content_type_maps_to_extension_hint() {
        assert_eq!(
            extension_hint_from_content_type(Some("audio/mpeg")),
            Some("mp3")
        );
        assert_eq!(
            extension_hint_from_content_type(Some("audio/mpeg; charset=utf-8")),
            Some("mp3")
        );
        assert_eq!(
            extension_hint_from_content_type(Some("audio/aacp")),
            Some("aac")
        );
        assert_eq!(
            extension_hint_from_content_type(Some("AUDIO/OGG")),
            Some("ogg")
        );
        assert_eq!(
            extension_hint_from_content_type(Some("application/ogg")),
            Some("ogg")
        );
        assert_eq!(
            extension_hint_from_content_type(Some("audio/flac")),
            Some("flac")
        );
        assert_eq!(extension_hint_from_content_type(None), None);
        assert_eq!(extension_hint_from_content_type(Some("video/mp4")), None);
    }

    #[test]
    fn seek_is_unsupported() {
        let (_tx, rx) = mpsc::sync_channel::<Vec<u8>>(1);
        let stop = Arc::new(AtomicBool::new(false));
        let mut src = IcyMediaSource::new(rx, stop);
        let err = src.seek(SeekFrom::Start(0)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        assert!(!src.is_seekable());
        assert!(src.byte_len().is_none());
    }
}
