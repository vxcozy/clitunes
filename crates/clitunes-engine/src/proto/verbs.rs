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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceArg {
    Local { path: String },
    Radio { uuid: String },
}

impl VerbEnvelope {
    pub fn from_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }

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
    fn unknown_verb_fails_parse() {
        let line = r#"{"cmd_id":"x","verb":"explode"}"#;
        assert!(VerbEnvelope::from_line(line).is_err());
    }
}
