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
    },
    SourceError {
        source: String,
        error: String,
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
        };
        let line = event.to_line();
        let parsed = Event::from_line(&line).unwrap();
        assert_eq!(parsed, event);
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
            }
            .topic(),
            "now_playing"
        );
        assert_eq!(Event::command_ok("x").topic(), "command");
    }
}
