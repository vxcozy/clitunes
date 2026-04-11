use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StationUuid(pub String);

impl StationUuid {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A radio station as returned by radio-browser.info or loaded from a
/// curated seed. Field set is intentionally minimal — we do not retain
/// everything radio-browser returns, only what's needed for tuning and
/// display.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Station {
    pub uuid: StationUuid,
    pub name: String,
    pub url_resolved: String,
    pub country: Option<String>,
    pub language: Option<String>,
    pub tags: Vec<String>,
    pub bitrate: Option<u32>,
    pub codec: Option<String>,
}

/// A curated station slot from the first-run picker seed (Unit 8).
/// Distinct from `Station` because it carries human-authored rationale
/// and a stable `slot` index used for state persistence and tests.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CuratedStation {
    pub slot: u8,
    pub name: &'static str,
    pub genre: &'static str,
    pub country: &'static str,
    pub url: &'static str,
    pub rationale: &'static str,
}
