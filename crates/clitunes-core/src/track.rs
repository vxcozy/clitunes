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
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use clitunes_core::Track;
    ///
    /// // When a title tag is present, it is used.
    /// let tagged = Track {
    ///     path: PathBuf::from("/music/song.flac"),
    ///     title: Some("Roygbiv".into()),
    ///     artist: None, album: None, album_artist: None,
    ///     track_num: None, year: None, duration_secs: None,
    ///     embedded_art: None,
    /// };
    /// assert_eq!(tagged.display_title(), "Roygbiv");
    ///
    /// // When no title tag exists, the filename stem is used.
    /// let untagged = Track {
    ///     path: PathBuf::from("/music/awesome_track.mp3"),
    ///     title: None,
    ///     artist: None, album: None, album_artist: None,
    ///     track_num: None, year: None, duration_secs: None,
    ///     embedded_art: None,
    /// };
    /// assert_eq!(untagged.display_title(), "awesome_track");
    /// ```
    pub fn display_title(&self) -> &str {
        self.title.as_deref().unwrap_or_else(|| {
            self.path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
        })
    }
}
