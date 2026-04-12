use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Metadata for a local audio file. All free-text fields are sanitised at
/// construction via `crate::sanitize` (tags are untrusted user data).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track {
    pub path: PathBuf,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_num: Option<u32>,
    pub year: Option<u32>,
    pub duration_secs: Option<f64>,
    #[serde(skip)]
    pub embedded_art: Option<Vec<u8>>,
}

impl Track {
    /// Display title: tag title if available, else filename stem, else "Unknown".
    pub fn display_title(&self) -> &str {
        self.title.as_deref().unwrap_or_else(|| {
            self.path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
        })
    }
}
