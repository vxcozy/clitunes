//! In-band Shoutcast/Icecast metadata parser.
//!
//! Servers that speak `Icy-MetaData: 1` interleave metadata blocks into the
//! audio byte stream every `Icy-MetaInt` bytes. The wire format is:
//!
//! ```text
//!  ┌───────────────┬───┬─────────────────────────────────────┬─────────┐
//!  │  audio bytes  │ L │ metadata payload, padded to L × 16  │  audio  │
//!  │  (metaint)    │   │                                     │         │
//!  └───────────────┴───┴─────────────────────────────────────┴─────────┘
//! ```
//!
//! where `L` is a single unsigned byte. `L = 0` means "no metadata this
//! cycle". The payload is ISO-8859-1 (in practice: UTF-8-ish, sometimes
//! Latin-1, sometimes garbage) and looks like
//! `StreamTitle='Artist - Song';StreamUrl='http://...';`. Titles are
//! quoted, keys separated by `;`. There is no escape mechanism.
//!
//! This module is a pure state machine: [`IcyParser::push`] consumes a raw
//! byte chunk from the HTTP body and returns the audio bytes plus any
//! metadata blocks that completed inside that chunk. No I/O, no network,
//! no async — drive it from whatever byte source you have. The radio
//! source pipes `reqwest` `Bytes` chunks through it; tests feed it
//! hand-crafted fixtures.
//!
//! # Security
//!
//! Metadata payloads are attacker-controlled terminal input: every string
//! parsed out of a metadata block is passed through
//! [`clitunes_core::sanitize`] before being returned. See plan decision
//! D20 and the `untrusted_string` module for the attack surface.

use bytes::Bytes;
use clitunes_core::sanitize;

/// Every metadata payload length byte is multiplied by this to get the
/// actual payload length in bytes. Both Shoutcast and Icecast use the same
/// 16-byte stride, historically chosen because early NSV/Shoutcast servers
/// wanted a byte-count-alignable block size.
pub const METADATA_BLOCK_STRIDE: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Consuming audio bytes, counting down to the next metadata block.
    Audio { remaining: usize },
    /// Next byte is the 1-byte length prefix.
    ExpectLength,
    /// Reading `expected` bytes of metadata payload into `meta_buf`.
    ReadingMeta { expected: usize },
}

/// Streaming parser for ICY-interleaved byte bodies.
///
/// Construct with [`IcyParser::new`]. Feed each chunk from the HTTP body
/// into [`IcyParser::push`]. The returned [`ParsedChunk`] carries the
/// subset of the chunk that is pure audio plus any metadata blocks that
/// completed during this call.
pub struct IcyParser {
    metaint: usize,
    state: State,
    meta_buf: Vec<u8>,
}

/// Output of a single `push` call. Multiple metadata blocks can complete
/// inside one byte chunk (if the chunk is huge or if several tiny blocks
/// are back-to-back), so `metadata_blocks` is a `Vec` rather than an
/// `Option`.
#[derive(Debug, Default)]
pub struct ParsedChunk {
    /// Audio bytes with the metadata block removed. Pass these to the
    /// decoder. When the chunk contained no audio bytes at all (e.g. a
    /// chunk that was entirely inside a metadata block) this is empty.
    pub audio: Vec<u8>,
    /// Zero or more metadata payloads. Each one is a raw metadata block
    /// body (already stripped of the length prefix). Use
    /// [`parse_metadata_block`] to extract structured fields.
    pub metadata_blocks: Vec<Vec<u8>>,
}

impl IcyParser {
    /// Construct a parser from the server's `Icy-MetaInt` header. Pass
    /// `None` when the server did not set the header — the parser then
    /// behaves as a transparent passthrough (`push` returns the chunk
    /// verbatim as audio and never emits metadata).
    pub fn new(metaint: Option<usize>) -> Self {
        let metaint = metaint.unwrap_or(0);
        let state = if metaint == 0 {
            State::Audio { remaining: 0 }
        } else {
            State::Audio { remaining: metaint }
        };
        Self {
            metaint,
            state,
            meta_buf: Vec::new(),
        }
    }

    /// Returns true when the parser is in passthrough mode (no metaint).
    pub fn is_passthrough(&self) -> bool {
        self.metaint == 0
    }

    /// Feed one `Bytes` chunk. Returns the split-out audio bytes and any
    /// metadata blocks that completed during the call.
    pub fn push(&mut self, chunk: Bytes) -> ParsedChunk {
        if self.is_passthrough() {
            return ParsedChunk {
                audio: chunk.to_vec(),
                metadata_blocks: Vec::new(),
            };
        }

        let mut out = ParsedChunk::default();
        // Reserve the common case: most of a chunk is audio.
        out.audio.reserve(chunk.len());

        let mut cursor = 0usize;
        let bytes = chunk.as_ref();

        while cursor < bytes.len() {
            match self.state {
                State::Audio { remaining } => {
                    let take = remaining.min(bytes.len() - cursor);
                    out.audio.extend_from_slice(&bytes[cursor..cursor + take]);
                    cursor += take;
                    let new_remaining = remaining - take;
                    if new_remaining == 0 {
                        self.state = State::ExpectLength;
                    } else {
                        self.state = State::Audio {
                            remaining: new_remaining,
                        };
                    }
                }
                State::ExpectLength => {
                    let len_byte = bytes[cursor] as usize;
                    cursor += 1;
                    let expected = len_byte * METADATA_BLOCK_STRIDE;
                    if expected == 0 {
                        // Empty metadata block: nothing to do, count the
                        // next metaint window.
                        self.state = State::Audio {
                            remaining: self.metaint,
                        };
                    } else {
                        self.meta_buf.clear();
                        self.meta_buf.reserve(expected);
                        self.state = State::ReadingMeta { expected };
                    }
                }
                State::ReadingMeta { expected } => {
                    let already = self.meta_buf.len();
                    let needed = expected - already;
                    let take = needed.min(bytes.len() - cursor);
                    self.meta_buf
                        .extend_from_slice(&bytes[cursor..cursor + take]);
                    cursor += take;
                    if self.meta_buf.len() == expected {
                        let block = std::mem::take(&mut self.meta_buf);
                        out.metadata_blocks.push(block);
                        self.state = State::Audio {
                            remaining: self.metaint,
                        };
                    }
                }
            }
        }

        out
    }
}

/// One parsed field extracted from a metadata block. Values are **already
/// sanitized** via [`clitunes_core::sanitize`] before reaching the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IcyField {
    pub key: String,
    pub value: String,
}

/// Parse a raw metadata block (as returned in [`ParsedChunk::metadata_blocks`])
/// into key-value pairs and sanitize every value. The metadata block format
/// is `Key1='value1';Key2='value2';` and is not escaped. We tolerate:
/// - Trailing zero-padding bytes (metadata blocks are padded to 16-byte
///   stride, so there's almost always NUL bytes at the end).
/// - Either single or double quotes around values.
/// - Missing terminal semicolon.
/// - Invalid UTF-8: falls back to [`String::from_utf8_lossy`].
/// - Extra whitespace around keys.
pub fn parse_metadata_block(bytes: &[u8]) -> Vec<IcyField> {
    // Strip trailing NUL padding before doing any string work.
    let end = bytes
        .iter()
        .rposition(|&b| b != 0)
        .map(|i| i + 1)
        .unwrap_or(0);
    let text = String::from_utf8_lossy(&bytes[..end]);

    let mut out = Vec::new();
    let mut rest = text.as_ref();
    while !rest.is_empty() {
        // Find the key.
        let eq = match rest.find('=') {
            Some(i) => i,
            None => break,
        };
        let key = rest[..eq].trim().to_string();
        rest = &rest[eq + 1..];

        // Value can be quoted with either ' or ". Unquoted values are
        // tolerated by reading until the next ';' — a quirk seen in the
        // wild from some home-rolled Icecast providers.
        let (value, advance) = if let Some(first) = rest.chars().next() {
            if first == '\'' || first == '"' {
                let quote = first;
                let body = &rest[1..];
                if let Some(end_quote) = body.find(quote) {
                    let val = body[..end_quote].to_string();
                    (val, 1 + end_quote + 1) // open + body + close
                } else {
                    // Unterminated quote: take everything we have.
                    (body.to_string(), rest.len())
                }
            } else if let Some(semi) = rest.find(';') {
                (rest[..semi].to_string(), semi)
            } else {
                (rest.to_string(), rest.len())
            }
        } else {
            break;
        };
        rest = &rest[advance..];

        // Skip the separator `;` if present.
        if let Some(stripped) = rest.strip_prefix(';') {
            rest = stripped;
        }

        // Drop keys that are all whitespace — they don't carry info.
        if key.is_empty() {
            continue;
        }

        out.push(IcyField {
            key,
            value: sanitize(&value),
        });
    }
    out
}

/// Convenience: given a metadata block, extract the StreamTitle if present
/// and return it already sanitized.
pub fn extract_stream_title(bytes: &[u8]) -> Option<String> {
    parse_metadata_block(bytes)
        .into_iter()
        .find(|f| f.key.eq_ignore_ascii_case("StreamTitle"))
        .map(|f| f.value)
}

/// Convenience: same as [`extract_stream_title`] but for `StreamUrl`.
pub fn extract_stream_url(bytes: &[u8]) -> Option<String> {
    parse_metadata_block(bytes)
        .into_iter()
        .find(|f| f.key.eq_ignore_ascii_case("StreamUrl"))
        .map(|f| f.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic ICY-interleaved byte stream:
    /// `audio_pattern * metaint`, then a metadata block with `meta_body`
    /// padded to 16-byte stride, then another `audio_pattern * metaint`.
    fn build_frame(metaint: usize, meta_body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for i in 0..metaint {
            out.push((i % 256) as u8);
        }
        // length prefix + padded body
        let blocks = meta_body.len().div_ceil(METADATA_BLOCK_STRIDE);
        out.push(blocks as u8);
        out.extend_from_slice(meta_body);
        let padding = blocks * METADATA_BLOCK_STRIDE - meta_body.len();
        out.extend(std::iter::repeat_n(0u8, padding));
        for i in 0..metaint {
            out.push((i % 256) as u8);
        }
        out
    }

    #[test]
    fn passthrough_when_no_metaint() {
        let mut p = IcyParser::new(None);
        assert!(p.is_passthrough());
        let chunk = Bytes::from_static(b"\x00\x01\x02\x03");
        let parsed = p.push(chunk);
        assert_eq!(parsed.audio, b"\x00\x01\x02\x03");
        assert!(parsed.metadata_blocks.is_empty());
    }

    #[test]
    fn parses_single_frame_whole_chunk() {
        let metaint = 32;
        let body = b"StreamTitle='Artist - Song';StreamUrl='http://ex.com';";
        let frame = build_frame(metaint, body);
        let mut p = IcyParser::new(Some(metaint));
        let parsed = p.push(Bytes::from(frame));
        assert_eq!(parsed.audio.len(), metaint * 2);
        assert_eq!(parsed.metadata_blocks.len(), 1);
        let title = extract_stream_title(&parsed.metadata_blocks[0]).unwrap();
        assert_eq!(title, "Artist - Song");
        let url = extract_stream_url(&parsed.metadata_blocks[0]).unwrap();
        assert_eq!(url, "http://ex.com");
    }

    #[test]
    fn empty_metadata_block_emits_no_event() {
        // metaint bytes, then length=0, then another metaint bytes.
        let metaint = 16;
        let mut bytes = vec![0xAA; metaint];
        bytes.push(0);
        bytes.extend_from_slice(&vec![0xBB; metaint]);
        let mut p = IcyParser::new(Some(metaint));
        let parsed = p.push(Bytes::from(bytes));
        assert_eq!(parsed.audio.len(), metaint * 2);
        assert!(parsed.metadata_blocks.is_empty());
    }

    #[test]
    fn split_across_length_prefix() {
        // The chunk boundary falls exactly on the length byte so the parser
        // must remember it's in ExpectLength across two `push` calls.
        let metaint = 8;
        let body = b"StreamTitle='X';";
        let frame = build_frame(metaint, body);
        // Split right after the `metaint` audio bytes but before the length.
        let (a, b) = frame.split_at(metaint);
        let mut p = IcyParser::new(Some(metaint));
        let r1 = p.push(Bytes::copy_from_slice(a));
        assert_eq!(r1.audio.len(), metaint);
        assert!(r1.metadata_blocks.is_empty());
        let r2 = p.push(Bytes::copy_from_slice(b));
        assert_eq!(r2.audio.len(), metaint); // second metaint window
        assert_eq!(r2.metadata_blocks.len(), 1);
        assert_eq!(
            extract_stream_title(&r2.metadata_blocks[0]).as_deref(),
            Some("X")
        );
    }

    #[test]
    fn split_mid_metadata_body() {
        // The chunk boundary falls in the middle of the metadata payload.
        // The parser must buffer across pushes.
        let metaint = 16;
        let body = b"StreamTitle='Split title here';";
        let frame = build_frame(metaint, body);
        // Find the metadata block start: after `metaint` audio + 1 length byte.
        let pivot = metaint + 1 + 8;
        let (a, b) = frame.split_at(pivot);
        let mut p = IcyParser::new(Some(metaint));
        let r1 = p.push(Bytes::copy_from_slice(a));
        let r2 = p.push(Bytes::copy_from_slice(b));
        // All the audio bytes accounted for across the two calls.
        assert_eq!(r1.audio.len() + r2.audio.len(), metaint * 2);
        // Metadata block is emitted exactly once, in r2.
        assert!(r1.metadata_blocks.is_empty());
        assert_eq!(r2.metadata_blocks.len(), 1);
        let title = extract_stream_title(&r2.metadata_blocks[0]).unwrap();
        assert_eq!(title, "Split title here");
    }

    #[test]
    fn multiple_metadata_blocks_in_one_chunk() {
        let metaint = 4;
        // frame1 + frame2 back-to-back.
        let f1 = build_frame(metaint, b"StreamTitle='A';");
        // build_frame starts with metaint audio bytes before the meta
        // block, which lines up with the trailing metaint bytes of f1 —
        // so the second frame has no leading audio to double-count.
        let f2_body_onwards = build_frame(metaint, b"StreamTitle='B';");
        // Skip the leading metaint of f2 because the previous frame already
        // put us ready for a length byte.
        let mut combined = f1;
        combined.extend_from_slice(&f2_body_onwards[metaint..]);

        let mut p = IcyParser::new(Some(metaint));
        let parsed = p.push(Bytes::from(combined));
        assert_eq!(parsed.metadata_blocks.len(), 2);
        assert_eq!(
            extract_stream_title(&parsed.metadata_blocks[0]).as_deref(),
            Some("A")
        );
        assert_eq!(
            extract_stream_title(&parsed.metadata_blocks[1]).as_deref(),
            Some("B")
        );
    }

    #[test]
    fn stream_title_is_sanitized() {
        // Hostile payload: OSC window-title injection with BEL terminator.
        let mut body: Vec<u8> = Vec::new();
        body.extend_from_slice(b"StreamTitle='Track\x1b]0;OWNED\x07';");
        let metaint = 32;
        let frame = build_frame(metaint, &body);
        let mut p = IcyParser::new(Some(metaint));
        let parsed = p.push(Bytes::from(frame));
        let title = extract_stream_title(&parsed.metadata_blocks[0]).unwrap();
        // Sanitizer strips ESC and BEL (BEL = 0x07, a C0 control byte).
        // The literal `]0;OWNED` survives because those are printable.
        assert!(!title.contains('\x1b'));
        assert!(!title.contains('\x07'));
        assert!(title.contains("Track"));
    }

    #[test]
    fn parse_metadata_block_handles_nul_padding() {
        let mut body: Vec<u8> = b"StreamTitle='X';".to_vec();
        while !body.len().is_multiple_of(METADATA_BLOCK_STRIDE) {
            body.push(0);
        }
        body.extend(std::iter::repeat_n(0u8, 16));
        let fields = parse_metadata_block(&body);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].key, "StreamTitle");
        assert_eq!(fields[0].value, "X");
    }

    #[test]
    fn parse_metadata_block_handles_unquoted_value() {
        let fields = parse_metadata_block(b"StreamTitle=Plain;Foo='bar';");
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].value, "Plain");
        assert_eq!(fields[1].value, "bar");
    }

    #[test]
    fn many_small_chunks_byte_by_byte() {
        // Pathological: feed a full frame one byte at a time.
        let metaint = 16;
        let body = b"StreamTitle='Byte by byte';";
        let frame = build_frame(metaint, body);
        let mut p = IcyParser::new(Some(metaint));
        let mut audio_total = 0;
        let mut blocks_total = 0;
        let mut captured_title: Option<String> = None;
        for &b in &frame {
            let parsed = p.push(Bytes::copy_from_slice(&[b]));
            audio_total += parsed.audio.len();
            for block in parsed.metadata_blocks {
                if captured_title.is_none() {
                    captured_title = extract_stream_title(&block);
                }
                blocks_total += 1;
            }
        }
        assert_eq!(audio_total, metaint * 2);
        assert_eq!(blocks_total, 1);
        assert_eq!(captured_title.as_deref(), Some("Byte by byte"));
    }
}
