//! Unit 6 hardening: end-to-end fuzz harness for the ICY parse + sanitise
//! pipeline. Feeds attacker-shaped payloads through the full
//! `IcyParser` → `parse_metadata_block` → `extract_stream_title` chain
//! (mirroring what the radio source does at runtime) and asserts that no
//! terminal-control byte survives into the emitted `NowPlaying` surface.
//!
//! Why this lives in a dedicated file: the existing
//! `untrusted_string_tests` unit tests cover `sanitize` in isolation, and
//! `icy_stream` has unit tests for protocol parsing. This file is the
//! *integration* contract — it proves that when a real radio operator
//! puts hostile bytes into a `StreamTitle='...'` block, the bytes that
//! reach our UI code are safe. If any single layer regresses (e.g. the
//! parser stops calling sanitize, or sanitize stops stripping C1), this
//! file catches it.

use bytes::Bytes;
use clitunes_core::sanitize;
use clitunes_engine::sources::radio::{
    extract_stream_title, extract_stream_url, parse_metadata_block, IcyParser,
    METADATA_BLOCK_STRIDE,
};

const METAINT: usize = 64;

/// Build a wire-format ICY chunk: METAINT audio bytes, then the length
/// prefix, then a padded metadata payload. Returns a single `Bytes`
/// containing the whole frame so callers can hand it to `IcyParser::push`
/// in one shot.
fn build_frame(metadata_body: &[u8]) -> Bytes {
    let audio = vec![0xAAu8; METAINT];
    let padded_len = metadata_body.len().div_ceil(METADATA_BLOCK_STRIDE) * METADATA_BLOCK_STRIDE;
    let length_byte = (padded_len / METADATA_BLOCK_STRIDE) as u8;

    let mut frame = Vec::with_capacity(METAINT + 1 + padded_len);
    frame.extend_from_slice(&audio);
    frame.push(length_byte);
    frame.extend_from_slice(metadata_body);
    // Shoutcast pads the block with NUL up to the next 16-byte boundary.
    frame.resize(METAINT + 1 + padded_len, 0);
    Bytes::from(frame)
}

/// Drive a metadata body through `IcyParser` and pull the decoded title
/// out the far end, exactly like `RadioSource::network_loop` does.
fn round_trip_title(metadata_body: &[u8]) -> Option<String> {
    let mut parser = IcyParser::new(Some(METAINT));
    let parsed = parser.push(build_frame(metadata_body));
    let block = parsed.metadata_blocks.first()?;
    extract_stream_title(block)
}

/// Assert that a string contains no terminal-control bytes that could
/// move the cursor, clear the screen, or hijack the OSC channel.
fn assert_terminal_safe(label: &str, value: &str) {
    for (i, b) in value.bytes().enumerate() {
        let is_whitelisted_whitespace = matches!(b, b'\t' | b'\n' | b'\r');
        let is_c0_control = b < 0x20 && !is_whitelisted_whitespace;
        let is_del = b == 0x7f;
        // We don't assert on raw bytes in 0x80..=0x9F because those are
        // UTF-8 continuation bytes for legitimate multi-byte chars. C1
        // controls are *codepoints* U+0080..=U+009F, which we check below.
        assert!(
            !is_c0_control,
            "{label}: byte {i:#x} ({b:#x}) is a C0 control in {value:?}"
        );
        assert!(!is_del, "{label}: byte {i:#x} ({b:#x}) is DEL in {value:?}");
    }
    for c in value.chars() {
        let cu = c as u32;
        assert_ne!(
            c, '\x1b',
            "{label}: literal ESC survived sanitisation in {value:?}"
        );
        assert!(
            !(0x80..=0x9F).contains(&cu),
            "{label}: C1 codepoint U+{cu:04X} survived sanitisation in {value:?}"
        );
    }
}

/// Twelve attack payloads straight from the Unit 6 bead. Each one is
/// exercised through the full parse+sanitise pipeline.
fn attack_payloads() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("csi_clear_screen", b"\x1b[2J".to_vec()),
        ("csi_color_sgr", b"\x1b[31mEVIL\x1b[0m".to_vec()),
        ("osc_set_window_title", b"\x1b]0;EVIL\x07".to_vec()),
        ("osc52_clipboard", b"\x1b]52;c;QUJDREVG\x07".to_vec()),
        ("bell_spam", b"\x07\x07\x07\x07\x07".to_vec()),
        // C1 CSI as Unicode codepoint U+009B encoded in UTF-8. A raw 0x9b
        // byte isn't valid UTF-8, so operators who want to smuggle C1
        // use the codepoint form. sanitize() must strip it.
        ("c1_csi_codepoint", "\u{009B}EVIL".as_bytes().to_vec()),
        (
            "mixed_legit_and_hostile",
            b"Artist - Title\x1b[2J\x07".to_vec(),
        ),
        (
            "em_dash_confusable",
            "Artist \u{2013} Title".as_bytes().to_vec(),
        ),
        ("very_long_title", vec![b'A'; 64 * 1024]),
        ("empty_title", Vec::new()),
        ("only_whitespace", b"   \t\n   ".to_vec()),
        ("embedded_nul", b"Artist\x00 - Title".to_vec()),
    ]
}

#[test]
fn sanitize_strips_every_known_attack_payload() {
    for (label, bytes) in attack_payloads() {
        // The parser is byte-oriented; sanitize operates on &str. We
        // lossy-decode the same way ICY field values do, and assert
        // safety on the cleaned string.
        let as_string = String::from_utf8_lossy(&bytes).into_owned();
        let cleaned = sanitize(&as_string);
        assert_terminal_safe(label, &cleaned);
    }
}

#[test]
fn round_trip_clear_screen_attack_is_stripped() {
    let body = b"StreamTitle='\x1b[2J\x1b[H pwned ';StreamUrl='';";
    let title = round_trip_title(body).expect("title extracted");
    assert_terminal_safe("csi_clear_round_trip", &title);
    assert!(
        title.contains("pwned"),
        "legit text must survive: {title:?}"
    );
    assert!(!title.contains('\x1b'), "ESC must be stripped: {title:?}");
}

#[test]
fn round_trip_osc_window_title_is_stripped() {
    let body = b"StreamTitle='Artist\x1b]0;OWNED\x07 - Track';StreamUrl='';";
    let title = round_trip_title(body).expect("title extracted");
    assert_terminal_safe("osc_set_title_round_trip", &title);
    assert!(!title.contains('\x07'), "BEL must be stripped: {title:?}");
    assert!(!title.contains('\x1b'), "ESC must be stripped: {title:?}");
}

#[test]
fn round_trip_osc52_clipboard_is_stripped() {
    let body = b"StreamTitle='ok\x1b]52;c;QUJDREVG\x07';StreamUrl='';";
    let title = round_trip_title(body).expect("title extracted");
    assert_terminal_safe("osc52_round_trip", &title);
    assert!(title.contains("ok"));
    assert!(!title.contains('\x1b'));
}

#[test]
fn round_trip_bell_spam_is_stripped() {
    let body = b"StreamTitle='ring\x07\x07\x07 ring';StreamUrl='';";
    let title = round_trip_title(body).expect("title extracted");
    assert_terminal_safe("bell_round_trip", &title);
    assert!(!title.contains('\x07'));
}

#[test]
fn round_trip_embedded_nul_is_stripped_title_survives() {
    let body = b"StreamTitle='Artist\x00 - Title';StreamUrl='';";
    let title = round_trip_title(body).expect("title extracted");
    assert_terminal_safe("embedded_nul_round_trip", &title);
    // The NUL is stripped but the legit text around it survives.
    assert!(title.contains("Artist"));
    assert!(title.contains("Title"));
}

#[test]
fn round_trip_em_dash_confusable_passes_through() {
    let body = "StreamTitle='Artist \u{2013} Title';StreamUrl='';".as_bytes();
    let title = round_trip_title(body).expect("title extracted");
    assert_eq!(title, "Artist \u{2013} Title");
}

#[test]
fn round_trip_long_title_is_preserved() {
    // The ICY length byte is a single octet multiplied by 16, so the
    // largest metadata block on the wire is 255 * 16 = 4080 bytes total,
    // including `StreamTitle='...';` framing. A ~3900-byte title is the
    // realistic upper bound an operator can actually ship. The point of
    // this test is to prove the pipeline doesn't panic or truncate
    // dangerously on a pathologically long (but protocol-legal) title.
    let long = "A".repeat(3900);
    let body = format!("StreamTitle='{long}';");
    let mut parser = IcyParser::new(Some(METAINT));
    let parsed = parser.push(build_frame(body.as_bytes()));
    let block = parsed
        .metadata_blocks
        .first()
        .expect("one metadata block emitted");
    let title = extract_stream_title(block).expect("title extracted");
    assert_terminal_safe("very_long_round_trip", &title);
    assert!(title.starts_with("AAAA"));
    assert_eq!(title.len(), 3900);
}

#[test]
fn round_trip_only_whitespace_title_yields_safe_value() {
    let body = b"StreamTitle='   \t\n   ';StreamUrl='';";
    let title = round_trip_title(body);
    // sanitize keeps whitespace, so we should see either Some(" ...") or
    // None depending on how the parser treats entirely-empty-after-trim
    // values. Either way the result must be terminal-safe.
    if let Some(t) = title {
        assert_terminal_safe("whitespace_round_trip", &t);
    }
}

#[test]
fn round_trip_empty_metadata_block_emits_no_title() {
    // Length byte of 0 → parser emits an empty metadata block. Downstream
    // code must not explode when asked to extract a title from nothing.
    let mut frame = vec![0xAAu8; METAINT];
    frame.push(0);
    let mut parser = IcyParser::new(Some(METAINT));
    let parsed = parser.push(Bytes::from(frame));
    // Either zero blocks or one empty block — both are acceptable; both
    // must be safe to dereference.
    if let Some(block) = parsed.metadata_blocks.first() {
        assert_eq!(
            extract_stream_title(block),
            None,
            "empty block must not synthesise a title"
        );
    }
}

#[test]
fn parse_metadata_block_sanitises_all_field_values() {
    // Exercise the lower-level parser: every field value must come out
    // sanitised, not just StreamTitle.
    let body = b"StreamTitle='ok\x1b[31m';StreamUrl='http://\x07evil/';Mode='live\x00';";
    let fields = parse_metadata_block(body);
    assert!(
        !fields.is_empty(),
        "parse_metadata_block must emit at least one field"
    );
    for f in &fields {
        assert_terminal_safe(&format!("field[{}]", f.key), &f.value);
    }
}

#[test]
fn extract_stream_url_is_sanitised() {
    let body = b"StreamTitle='clean';StreamUrl='https://good/\x1b[2J';";
    let url = extract_stream_url(body).expect("url extracted");
    assert_terminal_safe("stream_url", &url);
    assert!(url.contains("https://good/"));
    assert!(!url.contains('\x1b'));
}
