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
///
/// Uses owned `String` fields (rather than `&'static str`) so the same
/// type can represent both the baked-in seed and a user-provided
/// override loaded from `~/.config/clitunes/curated_stations.toml` at
/// runtime. The seed list is constructed once at startup, so the
/// allocation cost is negligible and the flexibility is worth it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CuratedStation {
    pub slot: u8,
    pub name: String,
    pub genre: String,
    pub country: String,
    /// Either a direct stream URL or a `radiobrowser:<uuid>` sentinel
    /// — the picker resolves the latter to a live URL via `StationDb`
    /// at pick-time so the seed file never goes stale when a station
    /// operator changes their upstream host.
    pub url: String,
    pub rationale: String,
}
