use serde::{Deserialize, Serialize};

/// Stable visualiser identifier. These names are part of the user-facing
/// config surface and the `:viz <name>` command; do not rename them
/// without a migration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VisualiserId {
    Auralis,
    Tideline,
    Cascade,
}

impl VisualiserId {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auralis => "auralis",
            Self::Tideline => "tideline",
            Self::Cascade => "cascade",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auralis" => Some(Self::Auralis),
            "tideline" => Some(Self::Tideline),
            "cascade" => Some(Self::Cascade),
            _ => None,
        }
    }
}

/// Which rendering surface a visualiser runs on. This is the forcing
/// function for the rendering-path-agnostic visualiser trait (D8): Cascade
/// is pure-CPU ratatui, Auralis and Tideline are GPU + Kitty graphics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceKind {
    /// Pure-CPU, rendered by ratatui as unicode cells.
    Tui,
    /// GPU-rendered, streamed to the terminal via Kitty graphics protocol.
    Gpu,
}
