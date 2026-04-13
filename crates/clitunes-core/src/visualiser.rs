use serde::{Deserialize, Serialize};

/// Stable visualiser identifier. These names are part of the user-facing
/// config surface and the `:viz <name>` command; do not rename them
/// without a migration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VisualiserId {
    Auralis,
    Plasma,
    Ripples,
    Tunnel,
    Metaballs,
    Starfield,
    Tideline,
    Cascade,
    Fire,
    Matrix,
    Moire,
    Vortex,
}

impl VisualiserId {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auralis => "auralis",
            Self::Plasma => "plasma",
            Self::Ripples => "ripples",
            Self::Tunnel => "tunnel",
            Self::Metaballs => "metaballs",
            Self::Starfield => "starfield",
            Self::Tideline => "tideline",
            Self::Cascade => "cascade",
            Self::Fire => "fire",
            Self::Matrix => "matrix",
            Self::Moire => "moire",
            Self::Vortex => "vortex",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auralis" => Some(Self::Auralis),
            "plasma" => Some(Self::Plasma),
            "ripples" => Some(Self::Ripples),
            "tunnel" => Some(Self::Tunnel),
            "metaballs" => Some(Self::Metaballs),
            "starfield" => Some(Self::Starfield),
            "tideline" => Some(Self::Tideline),
            "cascade" => Some(Self::Cascade),
            "fire" => Some(Self::Fire),
            "matrix" => Some(Self::Matrix),
            "moire" => Some(Self::Moire),
            "vortex" => Some(Self::Vortex),
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
