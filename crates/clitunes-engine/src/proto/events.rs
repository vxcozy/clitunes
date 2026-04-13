use clitunes_core::{BrowseItem, LibraryCategory};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum Event {
    StateChanged {
        state: PlayState,
        source: Option<String>,
        station_or_path: Option<String>,
        position_secs: Option<f64>,
        duration_secs: Option<f64>,
    },
    NowPlayingChanged {
        artist: Option<String>,
        title: Option<String>,
        album: Option<String>,
        station: Option<String>,
        raw_stream_title: Option<String>,
        /// Optional cover-art URL (e.g. Spotify CDN). Added in v1.2;
        /// `#[serde(default)]` preserves compatibility with older daemons
        /// that do not emit this field.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        art_url: Option<String>,
    },
    SourceError {
        source: String,
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_code: Option<String>,
    },
    DaemonShuttingDown {
        reason: String,
    },
    VolumeChanged {
        volume: u8,
    },
    VizChanged {
        name: String,
    },
    LayoutChanged {
        name: String,
    },
    PcmMeta {
        sample_rate: u32,
        channels: u8,
        frame_count_total: u64,
    },
    PcmTap {
        shm_name: String,
        sample_rate: u32,
        channels: u8,
        capacity: u32,
    },
    CommandResult {
        cmd_id: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Results of a `Verb::Search` call, in provider-ranked order.
    SearchResults {
        query: String,
        items: Vec<BrowseItem>,
        total: u32,
    },
    /// Results of a `Verb::BrowseLibrary` call for one of the user's
    /// saved-library categories.
    LibraryResults {
        category: LibraryCategory,
        items: Vec<BrowseItem>,
        total: u32,
    },
    /// Tracks of a specific playlist fetched via `Verb::BrowsePlaylist`.
    PlaylistResults {
        playlist_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        playlist_name: Option<String>,
        items: Vec<BrowseItem>,
        total: u32,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlayState {
    Playing,
    Paused,
    Stopped,
}

impl Event {
    /// Build a successful command-result event.
    ///
    /// # Examples
    ///
    /// ```
    /// use clitunes_engine::proto::events::Event;
    ///
    /// let ev = Event::command_ok("cmd-42");
    /// assert_eq!(ev.topic(), "command");
    ///
    /// // The JSON representation omits the `error` field when it is None.
    /// let json = ev.to_line();
    /// assert!(!json.contains("error"));
    /// ```
    pub fn command_ok(cmd_id: impl Into<String>) -> Self {
        Self::CommandResult {
            cmd_id: cmd_id.into(),
            ok: true,
            error: None,
        }
    }

    /// Build a failed command-result event.
    ///
    /// # Examples
    ///
    /// ```
    /// use clitunes_engine::proto::events::Event;
    ///
    /// let ev = Event::command_err("cmd-99", "unknown verb");
    /// assert_eq!(ev.topic(), "command");
    ///
    /// let json = ev.to_line();
    /// assert!(json.contains("unknown verb"));
    /// ```
    pub fn command_err(cmd_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self::CommandResult {
            cmd_id: cmd_id.into(),
            ok: false,
            error: Some(error.into()),
        }
    }

    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("Event is always serialisable")
    }

    pub fn from_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }

    /// Returns the subscription topic string for this event, used to route
    /// events to subscribers.
    ///
    /// # Examples
    ///
    /// ```
    /// use clitunes_engine::proto::events::Event;
    ///
    /// assert_eq!(Event::command_ok("x").topic(), "command");
    ///
    /// let np = Event::NowPlayingChanged {
    ///     artist: None, title: None, album: None,
    ///     station: None, raw_stream_title: None,
    ///     art_url: None,
    /// };
    /// assert_eq!(np.topic(), "now_playing");
    /// ```
    pub fn topic(&self) -> &'static str {
        match self {
            Self::StateChanged { .. } => "state",
            Self::NowPlayingChanged { .. } => "now_playing",
            Self::SourceError { .. } => "errors",
            Self::DaemonShuttingDown { .. } => "state",
            Self::VolumeChanged { .. } => "state",
            Self::VizChanged { .. } => "state",
            Self::LayoutChanged { .. } => "state",
            Self::PcmMeta { .. } => "pcm_meta",
            Self::PcmTap { .. } => "pcm_meta",
            Self::CommandResult { .. } => "command",
            Self::SearchResults { .. } => "browse",
            Self::LibraryResults { .. } => "browse",
            Self::PlaylistResults { .. } => "browse",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_changed_roundtrip() {
        let event = Event::StateChanged {
            state: PlayState::Playing,
            source: Some("radio".into()),
            station_or_path: Some("SomaFM Groove Salad".into()),
            position_secs: None,
            duration_secs: None,
        };
        let line = event.to_line();
        let parsed = Event::from_line(&line).unwrap();
        assert_eq!(parsed, event);
    }

    #[test]
    fn now_playing_roundtrip() {
        let event = Event::NowPlayingChanged {
            artist: Some("Boards of Canada".into()),
            title: Some("Roygbiv".into()),
            album: None,
            station: Some("SomaFM".into()),
            raw_stream_title: Some("Boards of Canada - Roygbiv".into()),
            art_url: None,
        };
        let line = event.to_line();
        // art_url must be omitted when None (backward compat with v1.1 clients).
        assert!(!line.contains("art_url"));
        let parsed = Event::from_line(&line).unwrap();
        assert_eq!(parsed, event);
    }

    #[test]
    fn now_playing_with_art_url_roundtrip() {
        let event = Event::NowPlayingChanged {
            artist: Some("Daft Punk".into()),
            title: Some("Get Lucky".into()),
            album: Some("Random Access Memories".into()),
            station: None,
            raw_stream_title: None,
            art_url: Some("https://i.scdn.co/image/abc".into()),
        };
        let line = event.to_line();
        assert!(line.contains("i.scdn.co"));
        let parsed = Event::from_line(&line).unwrap();
        assert_eq!(parsed, event);
    }

    #[test]
    fn now_playing_backward_compat_no_art_url_field() {
        // Old daemons (pre-v1.2) emit NowPlayingChanged without art_url.
        // serde(default) must deserialize missing field as None.
        let json = r#"{"event":"now_playing_changed","data":{"artist":"x","title":"y","album":null,"station":null,"raw_stream_title":null}}"#;
        let parsed = Event::from_line(json).unwrap();
        match parsed {
            Event::NowPlayingChanged { art_url, .. } => assert_eq!(art_url, None),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn command_ok_roundtrip() {
        let event = Event::command_ok("cmd-42");
        let line = event.to_line();
        let parsed = Event::from_line(&line).unwrap();
        assert_eq!(parsed, event);
        assert!(!line.contains("error"));
    }

    #[test]
    fn command_err_roundtrip() {
        let event = Event::command_err("cmd-99", "unknown verb: explode");
        let line = event.to_line();
        let parsed = Event::from_line(&line).unwrap();
        assert_eq!(parsed, event);
    }

    #[test]
    fn topic_classification() {
        assert_eq!(
            Event::NowPlayingChanged {
                artist: None,
                title: None,
                album: None,
                station: None,
                raw_stream_title: None,
                art_url: None,
            }
            .topic(),
            "now_playing"
        );
        assert_eq!(Event::command_ok("x").topic(), "command");
    }

    #[test]
    fn source_error_roundtrip_without_code() {
        let event = Event::SourceError {
            source: "radio".into(),
            error: "connection refused".into(),
            error_code: None,
        };
        let line = event.to_line();
        // error_code should be omitted when None.
        assert!(!line.contains("error_code"));
        let parsed = Event::from_line(&line).unwrap();
        assert_eq!(parsed, event);
    }

    #[test]
    fn source_error_roundtrip_with_code() {
        let event = Event::SourceError {
            source: "spotify".into(),
            error: "Premium required".into(),
            error_code: Some("premium_required".into()),
        };
        let line = event.to_line();
        assert!(line.contains("premium_required"));
        let parsed = Event::from_line(&line).unwrap();
        assert_eq!(parsed, event);
    }

    #[test]
    fn search_results_roundtrip() {
        let event = Event::SearchResults {
            query: "daft punk".into(),
            items: vec![BrowseItem {
                title: "Get Lucky".into(),
                artist: Some("Daft Punk".into()),
                album: Some("Random Access Memories".into()),
                uri: "spotify:track:2Foc5Q5nqNiosCNqttzHof".into(),
                art_url: Some("https://i.scdn.co/image/x".into()),
                duration_ms: Some(369_000),
            }],
            total: 1,
        };
        let line = event.to_line();
        assert!(line.contains("search_results"));
        assert!(line.contains("daft punk"));
        let parsed = Event::from_line(&line).unwrap();
        assert_eq!(parsed, event);
        assert_eq!(event.topic(), "browse");
    }

    #[test]
    fn library_results_roundtrip() {
        let event = Event::LibraryResults {
            category: LibraryCategory::SavedTracks,
            items: vec![],
            total: 0,
        };
        let line = event.to_line();
        assert!(line.contains("saved_tracks"));
        let parsed = Event::from_line(&line).unwrap();
        assert_eq!(parsed, event);
    }

    #[test]
    fn playlist_results_roundtrip() {
        let event = Event::PlaylistResults {
            playlist_id: "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M".into(),
            playlist_name: Some("Today's Top Hits".into()),
            items: vec![],
            total: 0,
        };
        let line = event.to_line();
        let parsed = Event::from_line(&line).unwrap();
        assert_eq!(parsed, event);
    }

    #[test]
    fn source_error_backward_compat_no_error_code_field() {
        // Old daemons (pre-v1.2) emit SourceError without error_code.
        // serde(default) should deserialize missing field as None.
        let json = r#"{"event":"source_error","data":{"source":"radio","error":"oops"}}"#;
        let parsed = Event::from_line(json).unwrap();
        assert_eq!(
            parsed,
            Event::SourceError {
                source: "radio".into(),
                error: "oops".into(),
                error_code: None,
            }
        );
    }
}
