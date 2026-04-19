use serde::{Deserialize, Serialize};

/// Stable visualiser identifier. These names are part of the user-facing
/// config surface and the `:viz <name>` command; do not rename them
/// without a migration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VisualiserId {
    Plasma,
    Ripples,
    Tunnel,
    Metaballs,
    Fire,
    Matrix,
    Moire,
    Vortex,
    Wave,
    Scope,
    Heartbeat,
    ClassicPeak,
    BarsDot,
    BarsOutline,
    Binary,
    Scatter,
    Terrain,
    Butterfly,
    Pulse,
    Rain,
    Sakura,
    Retro,
}

impl VisualiserId {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Plasma => "plasma",
            Self::Ripples => "ripples",
            Self::Tunnel => "tunnel",
            Self::Metaballs => "metaballs",
            Self::Fire => "fire",
            Self::Matrix => "matrix",
            Self::Moire => "moire",
            Self::Vortex => "vortex",
            Self::Wave => "wave",
            Self::Scope => "scope",
            Self::Heartbeat => "heartbeat",
            Self::ClassicPeak => "classicpeak",
            Self::BarsDot => "barsdot",
            Self::BarsOutline => "barsoutline",
            Self::Binary => "binary",
            Self::Scatter => "scatter",
            Self::Terrain => "terrain",
            Self::Butterfly => "butterfly",
            Self::Pulse => "pulse",
            Self::Rain => "rain",
            Self::Sakura => "sakura",
            Self::Retro => "retro",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "plasma" => Some(Self::Plasma),
            "ripples" => Some(Self::Ripples),
            "tunnel" => Some(Self::Tunnel),
            "metaballs" => Some(Self::Metaballs),
            "fire" => Some(Self::Fire),
            "matrix" => Some(Self::Matrix),
            "moire" => Some(Self::Moire),
            "vortex" => Some(Self::Vortex),
            "wave" => Some(Self::Wave),
            "scope" => Some(Self::Scope),
            "heartbeat" => Some(Self::Heartbeat),
            "classicpeak" => Some(Self::ClassicPeak),
            "barsdot" => Some(Self::BarsDot),
            "barsoutline" => Some(Self::BarsOutline),
            "binary" => Some(Self::Binary),
            "scatter" => Some(Self::Scatter),
            "terrain" => Some(Self::Terrain),
            "butterfly" => Some(Self::Butterfly),
            "pulse" => Some(Self::Pulse),
            "rain" => Some(Self::Rain),
            "sakura" => Some(Self::Sakura),
            "retro" => Some(Self::Retro),
            _ => None,
        }
    }
}

/// Which rendering surface a visualiser runs on. Post-pivot, every built-in
/// visualiser is `Tui` — a CPU cell grid emitted as truecolor ANSI. The
/// enum is retained for forward compatibility if a future visualiser ever
/// needs a different surface (e.g. a raw pixel buffer for image display).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceKind {
    /// Pure-CPU cell grid rendered as truecolor ANSI.
    Tui,
    /// Reserved for a hypothetical GPU/Kitty path. Not currently used.
    Gpu,
}
