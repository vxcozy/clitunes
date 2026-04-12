//! Now-playing types shared across the engine, daemon, and UI panes.
//!
//! A radio stream produces two kinds of display information:
//! - **Static station info** from HTTP `Icy-*` response headers, known the
//!   moment we connect (name, genre, bitrate, description).
//! - **Dynamic track info** from in-band `StreamTitle` chunks interleaved
//!   every `Icy-MetaInt` bytes, updated whenever the DJ cross-fades.
//!
//! We model both as a single [`NowPlaying`] struct — the UI renders whatever
//! fields happen to be populated — and publish *changes* to it as events on
//! a broadcast channel so multiple panes can subscribe.
//!
//! Every free-text field below arrives from untrusted network input and is
//! **always** passed through [`crate::untrusted_string::sanitize`] before
//! landing in a `NowPlaying` value. That invariant lives at every ingestion
//! site (ICY header parser, ICY in-band parser, lofty tag reader) so that
//! downstream UI code can trust every string it receives without having to
//! re-sanitize. See plan decision D20.

use serde::{Deserialize, Serialize};

/// The currently-playing track and station. All fields are optional because
/// different sources expose different amounts of information: an Icecast
/// stream may omit `station_description`, a Shoutcast stream may not emit
/// `StreamUrl`, a silent moment in the ICY stream will leave `track_title`
/// equal to the previous value.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NowPlaying {
    /// In-band `StreamTitle='Artist - Song'` value, already sanitized.
    /// `None` until the first metadata block arrives.
    pub track_title: Option<String>,
    /// In-band `StreamUrl='...'` value if present, already sanitized.
    pub track_url: Option<String>,

    /// HTTP `Icy-Name` header, already sanitized.
    pub station_name: Option<String>,
    /// HTTP `Icy-Genre` header, already sanitized.
    pub station_genre: Option<String>,
    /// HTTP `Icy-Description` header, already sanitized.
    pub station_description: Option<String>,
    /// HTTP `Icy-Br` header parsed as kbps. `None` if absent or unparseable.
    pub station_bitrate_kbps: Option<u32>,
}

impl NowPlaying {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when no track or station information is present at all.
    ///
    /// # Examples
    ///
    /// ```
    /// use clitunes_core::NowPlaying;
    ///
    /// // A default NowPlaying has no fields set.
    /// assert!(NowPlaying::default().is_empty());
    ///
    /// // Setting any field makes it non-empty.
    /// let np = NowPlaying {
    ///     track_title: Some("Artist - Song".into()),
    ///     ..Default::default()
    /// };
    /// assert!(!np.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.track_title.is_none()
            && self.track_url.is_none()
            && self.station_name.is_none()
            && self.station_genre.is_none()
            && self.station_description.is_none()
            && self.station_bitrate_kbps.is_none()
    }
}

/// A change event published whenever the now-playing state updates. Panes
/// subscribe to a broadcast of these events and re-render on receipt.
///
/// `Reconnecting` / `Reconnected` give the UI a way to show a brief "link
/// dropped, retrying" banner in the now-playing strip without having to
/// poll the streamer's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NowPlayingEvent {
    /// Static station info was just populated from the response headers.
    /// Carries the full snapshot (not a delta) so late subscribers don't
    /// have to replay history.
    StationInfo(NowPlaying),
    /// In-band track title (and optionally URL) changed. The `NowPlaying`
    /// value is the full current state, not just the delta, for the same
    /// reason.
    TrackChanged(NowPlaying),
    /// Source reconnect underway. `attempt` starts at 1 for the first
    /// retry; the streamer gives up and surfaces an error after the
    /// configured backoff schedule is exhausted.
    Reconnecting { attempt: u32 },
    /// Stream reopened successfully after a drop. The UI can flash a brief
    /// "reconnected" marker and resume showing the previous track info.
    Reconnected,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_default_is_empty() {
        assert!(NowPlaying::default().is_empty());
    }

    #[test]
    fn populated_is_not_empty() {
        let np = NowPlaying {
            track_title: Some("Artist - Song".into()),
            ..Default::default()
        };
        assert!(!np.is_empty());
    }
}
