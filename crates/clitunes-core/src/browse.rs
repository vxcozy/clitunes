use serde::{Deserialize, Serialize};

/// A single browse/search result item — a track, album, or playlist entry
/// returned by a content-provider (e.g. Spotify Web API).
///
/// This is the shared transport type used by the picker UI and the daemon
/// verb/event protocol. It lives in `clitunes-core` so both crates can
/// depend on it without pulling in HTTP or async machinery.
///
/// Fields map loosely onto Spotify's track/album/playlist objects but are
/// deliberately generic so other providers can populate them later.
///
/// # Examples
///
/// ```
/// use clitunes_core::BrowseItem;
///
/// let item = BrowseItem {
///     title: "Roygbiv".into(),
///     artist: Some("Boards of Canada".into()),
///     album: Some("Music Has the Right to Children".into()),
///     uri: "spotify:track:4PTG3Z6ehGkBFwjybzWkR8".into(),
///     art_url: Some("https://i.scdn.co/image/abc".into()),
///     duration_ms: Some(2_212_00),
/// };
/// assert_eq!(item.uri, "spotify:track:4PTG3Z6ehGkBFwjybzWkR8");
/// ```
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowseItem {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    /// Provider-specific URI (e.g. `spotify:track:…`, `spotify:playlist:…`).
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Category for `BrowseLibrary` verb — which slice of the user's saved
/// library to fetch.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum LibraryCategory {
    SavedTracks,
    SavedAlbums,
    Playlists,
    RecentlyPlayed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browse_item_roundtrip_full() {
        let item = BrowseItem {
            title: "Roygbiv".into(),
            artist: Some("Boards of Canada".into()),
            album: Some("Music Has the Right to Children".into()),
            uri: "spotify:track:4PTG3Z6ehGkBFwjybzWkR8".into(),
            art_url: Some("https://i.scdn.co/image/abc".into()),
            duration_ms: Some(222_000),
        };
        let json = serde_json::to_string(&item).unwrap();
        let parsed: BrowseItem = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, item);
    }

    #[test]
    fn browse_item_art_url_omitted_when_none() {
        let item = BrowseItem {
            title: "Unknown".into(),
            artist: None,
            album: None,
            uri: "spotify:track:x".into(),
            art_url: None,
            duration_ms: None,
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(!json.contains("art_url"));
        assert!(!json.contains("artist"));
        assert!(!json.contains("duration_ms"));
    }

    #[test]
    fn library_category_roundtrip() {
        for cat in [
            LibraryCategory::SavedTracks,
            LibraryCategory::SavedAlbums,
            LibraryCategory::Playlists,
            LibraryCategory::RecentlyPlayed,
        ] {
            let json = serde_json::to_string(&cat).unwrap();
            let parsed: LibraryCategory = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, cat);
        }
    }

    #[test]
    fn library_category_snake_case() {
        let json = serde_json::to_string(&LibraryCategory::SavedTracks).unwrap();
        assert_eq!(json, "\"saved_tracks\"");
        let json = serde_json::to_string(&LibraryCategory::RecentlyPlayed).unwrap();
        assert_eq!(json, "\"recently_played\"");
    }
}
