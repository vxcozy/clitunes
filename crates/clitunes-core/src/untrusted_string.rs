//! Sanitiser for untrusted free-text strings (radio metadata, ICY headers,
//! lofty tag values).
//!
//! Per round-2 SEC-004 / D20: an operator can register hostile station
//! metadata that contains terminal escape sequences. If we display that
//! metadata in the picker or now-playing strip without sanitisation, it
//! pwns the user's terminal (cursor moves, OSC writes, color leaks, even
//! DECSDM scrolling regions). This module is the single chokepoint that
//! every untrusted string MUST pass through before it touches a `Station`
//! field, an event payload, or anything ratatui will render.
//!
//! Bytes stripped:
//! - C0 (0x00–0x1F) **except** TAB (0x09), LF (0x0A), CR (0x0D)
//! - C1 (0x80–0x9F) — the 8-bit equivalents to ESC sequences
//! - ESC (0x1B) — explicit even though it's covered by the C0 strip,
//!   because it's the most-abused byte
//! - DEL (0x7F) — also a control byte that some terminals interpret
//!
//! Bytes preserved: everything printable ASCII, all of UTF-8 above 0xA0,
//! plus the three whitelisted whitespace bytes above. This means a station
//! named "Радио-1 ✨" survives intact while one named
//! "\x1b[2J\x1b[HBLEEP" comes out as "[2J[HBLEEP".
//!
//! Note: this is **not** a HTML / Markdown / SQL escaper. It is purely a
//! terminal-safety filter. It does not protect against, e.g., a station
//! name that is 4MB of A's — length-bounding lives at the call site.

/// Strip terminal-control bytes from an untrusted string.
///
/// Allocates a new `String` only when bytes are actually removed; for the
/// common case of clean station names, this returns the input as-is.
///
/// # Examples
///
/// ```
/// use clitunes_core::sanitize;
///
/// // Clean strings pass through unchanged.
/// assert_eq!(sanitize("BBC Radio 6 Music"), "BBC Radio 6 Music");
///
/// // ESC sequences are stripped, but printable chars remain.
/// assert_eq!(sanitize("\x1b[2J\x1b[Hpwn"), "[2J[Hpwn");
///
/// // Unicode above the C1 range is preserved.
/// assert_eq!(sanitize("Радио-1 ✨"), "Радио-1 ✨");
/// ```
pub fn sanitize(input: &str) -> String {
    // Fast path: if no byte needs stripping, return the input unchanged.
    if input.bytes().all(is_safe) {
        return input.to_string();
    }

    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if char_is_safe(ch) {
            out.push(ch);
        }
    }
    out
}

/// Sanitise in place, mutating an existing `String` to drop unsafe bytes.
///
/// # Examples
///
/// ```
/// use clitunes_core::sanitize_in_place;
///
/// let mut s = String::from("hello\x1b[31mworld");
/// sanitize_in_place(&mut s);
/// assert_eq!(s, "hello[31mworld");
/// ```
pub fn sanitize_in_place(input: &mut String) {
    if input.bytes().all(is_safe) {
        return;
    }
    let cleaned = sanitize(input);
    *input = cleaned;
}

#[inline]
fn is_safe(byte: u8) -> bool {
    match byte {
        // Whitelisted whitespace.
        b'\t' | b'\n' | b'\r' => true,
        // C0 control range (everything else under 0x20 is unsafe).
        0x00..=0x1F => false,
        // DEL.
        0x7F => false,
        // C1 control range — note: inside multi-byte UTF-8 these bytes are
        // continuation bytes, not standalone C1 controls. The byte-level
        // fast-path is only used to *decide whether to allocate*; the
        // char-level path below is the actual filter and handles UTF-8
        // correctly. We bias the fast path toward allocation here on the
        // safe side: any byte 0x80–0x9F (continuation byte or C1) trips us
        // out of the fast path so the char-level filter can adjudicate.
        0x80..=0x9F => false,
        _ => true,
    }
}

#[inline]
fn char_is_safe(ch: char) -> bool {
    match ch {
        '\t' | '\n' | '\r' => true,
        c if (c as u32) < 0x20 => false,
        '\u{007F}' => false,
        // C1 controls as Unicode codepoints (this is what the byte path
        // 0x80–0x9F can never hit, because those are continuation bytes
        // inside UTF-8 — only when 0x80–0x9F appears as a *codepoint*
        // does it represent an actual C1 control).
        c if (0x80..=0x9F).contains(&(c as u32)) => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_string_passes_through() {
        assert_eq!(sanitize("BBC Radio 6 Music"), "BBC Radio 6 Music");
    }

    #[test]
    fn whitespace_is_preserved() {
        assert_eq!(sanitize("foo\tbar\nbaz\rqux"), "foo\tbar\nbaz\rqux");
    }

    #[test]
    fn esc_is_stripped() {
        assert_eq!(sanitize("\x1b[2J\x1b[Hpwn"), "[2J[Hpwn");
    }

    #[test]
    fn full_csi_clear_is_stripped() {
        // The classic "clear screen + move home" sequence operators love.
        let hostile = "\x1b[2J\x1b[1;1HBLEEP";
        let cleaned = sanitize(hostile);
        assert!(!cleaned.contains('\x1b'));
        assert!(cleaned.ends_with("BLEEP"));
    }

    #[test]
    fn c0_other_than_whitespace_is_stripped() {
        // 0x00 NUL, 0x07 BEL, 0x08 BS, 0x0C FF, 0x1F US.
        let hostile = "a\x00b\x07c\x08d\x0ce\x1ff";
        assert_eq!(sanitize(hostile), "abcdef");
    }

    #[test]
    fn del_is_stripped() {
        assert_eq!(sanitize("a\x7fb"), "ab");
    }

    #[test]
    fn c1_unicode_codepoints_are_stripped() {
        // U+0085 NEL is a C1 control.
        let hostile = "line1\u{0085}line2";
        assert_eq!(sanitize(hostile), "line1line2");
    }

    #[test]
    fn unicode_above_c1_is_preserved() {
        assert_eq!(sanitize("Радио-1 ✨"), "Радио-1 ✨");
    }

    #[test]
    fn empty_string_returns_empty() {
        assert_eq!(sanitize(""), "");
    }

    #[test]
    fn osc_link_is_stripped() {
        // OSC 8 hyperlink — another common operator-injectable sequence.
        let hostile = "\x1b]8;;https://evil.example/\x1b\\click me\x1b]8;;\x1b\\";
        let cleaned = sanitize(hostile);
        assert!(!cleaned.contains('\x1b'));
        assert!(cleaned.contains("click me"));
    }

    #[test]
    fn sanitize_in_place_no_alloc_on_clean() {
        let mut s = String::from("clean string");
        let ptr_before = s.as_ptr();
        sanitize_in_place(&mut s);
        assert_eq!(s, "clean string");
        // The fast path returns early without re-allocating.
        assert_eq!(s.as_ptr(), ptr_before);
    }
}
