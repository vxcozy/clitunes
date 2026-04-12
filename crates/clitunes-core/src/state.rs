use serde::{Deserialize, Serialize};

use crate::{StationUuid, VisualiserId};

/// Persistent state between clitunes invocations. Serialised to
/// `~/.config/clitunes/state.toml` with mode 0600 (per SEC-011).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub picker_seen: bool,
    #[serde(default)]
    pub last_station_uuid: Option<StationUuid>,
    #[serde(default)]
    pub last_visualiser: Option<VisualiserId>,
    #[serde(default)]
    pub last_layout: Option<String>,
}

impl State {
    /// Returns a blank state with no persisted preferences.
    ///
    /// # Examples
    ///
    /// ```
    /// use clitunes_core::State;
    ///
    /// let state = State::fresh();
    /// assert!(!state.picker_seen);
    /// assert!(state.last_station_uuid.is_none());
    /// assert!(state.last_visualiser.is_none());
    /// assert!(state.last_layout.is_none());
    /// ```
    pub const fn fresh() -> Self {
        Self {
            picker_seen: false,
            last_station_uuid: None,
            last_visualiser: None,
            last_layout: None,
        }
    }
}
