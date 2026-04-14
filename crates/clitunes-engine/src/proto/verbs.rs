use clitunes_core::LibraryCategory;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerbEnvelope {
    pub cmd_id: String,
    #[serde(flatten)]
    pub verb: Verb,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "verb", content = "args", rename_all = "snake_case")]
pub enum Verb {
    Play,
    Pause,
    Next,
    Prev,
    Volume {
        level: u8,
    },
    Source(SourceArg),
    Viz {
        name: String,
    },
    Layout {
        name: String,
    },
    Picker,
    Status,
    Subscribe {
        topic: String,
    },
    Unsubscribe {
        topic: String,
    },
    Quit,
    Capabilities,
    /// Search the active content provider (currently Spotify Web API).
    Search {
        query: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },
    /// Fetch a slice of the user's saved library (tracks, albums, playlists,
    /// recently-played) from the active content provider.
    BrowseLibrary {
        category: LibraryCategory,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },
    /// Fetch the tracks of a specific playlist by provider-specific id/uri.
    BrowsePlaylist {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceArg {
    Local { path: String },
    Radio { uuid: String },
    Spotify { uri: String },
}

impl VerbEnvelope {
    /// Deserialise a `VerbEnvelope` from a single line of JSON.
    ///
    /// # Examples
    ///
    /// ```
    /// use clitunes_engine::proto::verbs::{VerbEnvelope, Verb};
    ///
    /// let line = r#"{"cmd_id":"abc-1","verb":"play"}"#;
    /// let env = VerbEnvelope::from_line(line).unwrap();
    /// assert_eq!(env.cmd_id, "abc-1");
    /// assert_eq!(env.verb, Verb::Play);
    /// ```
    pub fn from_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }

    /// Serialise this envelope to a single line of JSON suitable for
    /// writing to the control socket.
    ///
    /// # Examples
    ///
    /// ```
    /// use clitunes_engine::proto::verbs::{VerbEnvelope, Verb};
    ///
    /// let env = VerbEnvelope {
    ///     cmd_id: "v-2".into(),
    ///     verb: Verb::Volume { level: 42 },
    /// };
    /// let line = env.to_line();
    /// let roundtrip = VerbEnvelope::from_line(&line).unwrap();
    /// assert_eq!(roundtrip, env);
    /// ```
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("VerbEnvelope is always serialisable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_roundtrip() {
        let env = VerbEnvelope {
            cmd_id: "abc-1".into(),
            verb: Verb::Play,
        };
        let line = env.to_line();
        let parsed = VerbEnvelope::from_line(&line).unwrap();
        assert_eq!(parsed, env);
    }

    #[test]
    fn volume_roundtrip() {
        let env = VerbEnvelope {
            cmd_id: "v-2".into(),
            verb: Verb::Volume { level: 42 },
        };
        let line = env.to_line();
        assert!(line.contains("42"));
        let parsed = VerbEnvelope::from_line(&line).unwrap();
        assert_eq!(parsed, env);
    }

    #[test]
    fn source_radio_roundtrip() {
        let env = VerbEnvelope {
            cmd_id: "s-3".into(),
            verb: Verb::Source(SourceArg::Radio {
                uuid: "550e8400-e29b-41d4-a716-446655440000".into(),
            }),
        };
        let line = env.to_line();
        let parsed = VerbEnvelope::from_line(&line).unwrap();
        assert_eq!(parsed, env);
    }

    #[test]
    fn subscribe_roundtrip() {
        let env = VerbEnvelope {
            cmd_id: "sub-1".into(),
            verb: Verb::Subscribe {
                topic: "now_playing".into(),
            },
        };
        let line = env.to_line();
        let parsed = VerbEnvelope::from_line(&line).unwrap();
        assert_eq!(parsed, env);
    }

    #[test]
    fn source_spotify_roundtrip() {
        let env = VerbEnvelope {
            cmd_id: "sp-1".into(),
            verb: Verb::Source(SourceArg::Spotify {
                uri: "spotify:track:4PTG3Z6ehGkBFwjybzWkR8".into(),
            }),
        };
        let line = env.to_line();
        assert!(line.contains("spotify"));
        let parsed = VerbEnvelope::from_line(&line).unwrap();
        assert_eq!(parsed, env);
    }

    #[test]
    fn unknown_verb_fails_parse() {
        let line = r#"{"cmd_id":"x","verb":"explode"}"#;
        assert!(VerbEnvelope::from_line(line).is_err());
    }

    #[test]
    fn search_roundtrip_with_limit() {
        let env = VerbEnvelope {
            cmd_id: "q-1".into(),
            verb: Verb::Search {
                query: "bohemian rhapsody".into(),
                limit: Some(25),
            },
        };
        let line = env.to_line();
        assert!(line.contains("bohemian"));
        assert!(line.contains("25"));
        let parsed = VerbEnvelope::from_line(&line).unwrap();
        assert_eq!(parsed, env);
    }

    #[test]
    fn search_roundtrip_no_limit() {
        let env = VerbEnvelope {
            cmd_id: "q-2".into(),
            verb: Verb::Search {
                query: "daft punk".into(),
                limit: None,
            },
        };
        let line = env.to_line();
        // `limit` should be omitted when None.
        assert!(!line.contains("limit"));
        let parsed = VerbEnvelope::from_line(&line).unwrap();
        assert_eq!(parsed, env);
    }

    #[test]
    fn browse_library_roundtrip() {
        let env = VerbEnvelope {
            cmd_id: "b-1".into(),
            verb: Verb::BrowseLibrary {
                category: LibraryCategory::SavedTracks,
                limit: Some(50),
            },
        };
        let line = env.to_line();
        assert!(line.contains("saved_tracks"));
        let parsed = VerbEnvelope::from_line(&line).unwrap();
        assert_eq!(parsed, env);
    }

    #[test]
    fn browse_playlist_roundtrip() {
        let env = VerbEnvelope {
            cmd_id: "p-1".into(),
            verb: Verb::BrowsePlaylist {
                id: "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M".into(),
                limit: None,
            },
        };
        let line = env.to_line();
        let parsed = VerbEnvelope::from_line(&line).unwrap();
        assert_eq!(parsed, env);
    }
}
