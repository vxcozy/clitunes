use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: &str = "clitunes-control-1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerBanner {
    pub version: String,
    pub capabilities: Vec<String>,
}

impl ServerBanner {
    pub fn new(capabilities: Vec<String>) -> Self {
        Self {
            version: PROTOCOL_VERSION.to_owned(),
            capabilities,
        }
    }

    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("ServerBanner is always serialisable")
    }

    pub fn from_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientBanner {
    pub client: String,
    pub version: String,
    #[serde(default)]
    pub subscribe: Vec<String>,
}

impl ClientBanner {
    pub fn new(client: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            client: client.into(),
            version: version.into(),
            subscribe: Vec::new(),
        }
    }

    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("ClientBanner is always serialisable")
    }

    pub fn from_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_banner_roundtrip() {
        let banner = ServerBanner::new(vec!["radio".into(), "local".into()]);
        let line = banner.to_line();
        let parsed = ServerBanner::from_line(&line).unwrap();
        assert_eq!(parsed, banner);
        assert_eq!(parsed.version, PROTOCOL_VERSION);
    }

    #[test]
    fn client_banner_roundtrip() {
        let banner = ClientBanner {
            client: "clitunes-tui".into(),
            version: "1.0.0".into(),
            subscribe: vec!["now_playing".into()],
        };
        let line = banner.to_line();
        let parsed = ClientBanner::from_line(&line).unwrap();
        assert_eq!(parsed, banner);
    }

    #[test]
    fn client_banner_missing_subscribe_defaults_empty() {
        let line = r#"{"client":"test","version":"0.1"}"#;
        let parsed = ClientBanner::from_line(line).unwrap();
        assert!(parsed.subscribe.is_empty());
    }
}
