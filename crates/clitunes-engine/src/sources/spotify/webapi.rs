//! Spotify Web API provider (daemon-side rspotify integration).
//!
//! Wraps an [`AuthCodePkceSpotify`] client built from a
//! [`SharedTokenProvider`]. Exposes a small, BrowseItem-shaped surface
//! that the daemon verb dispatcher hands to clients via
//! [`Event::SearchResults`] / [`Event::LibraryResults`] /
//! [`Event::PlaylistResults`].
//!
//! Every method returns `(Vec<BrowseItem>, u32 /* total */)`. Conversion
//! from rspotify model types is isolated in the private `map` submodule
//! so the network-bound methods stay thin and mappers are unit-testable.
//!
//! Token lifecycle:
//! - The client is built with `Config { token_refreshing: true, .. }`
//!   (rspotify's default), so 401-on-expiry triggers an automatic
//!   refresh using the `refresh_token` carried in the rspotify `Token`.
//! - If refresh itself fails (e.g. revoked consent), calls return
//!   `ClientError::Http(...)` — the dispatcher surfaces these as a
//!   `CommandResult { ok: false, error }` and the TUI prompts re-auth.

use std::sync::Arc;

use anyhow::{Context, Result};
use clitunes_core::BrowseItem;
use rspotify::{
    clients::{BaseClient, OAuthClient},
    model::{
        FullPlaylist, FullTrack, Image, PlayHistory, PlaylistId, PlaylistItem, SavedAlbum,
        SavedTrack, SearchResult, SearchType, SimplifiedPlaylist,
    },
    AuthCodePkceSpotify, Config, Credentials, OAuth,
};

use super::auth::{SPOTIFY_CLIENT_ID, SPOTIFY_REDIRECT_URI, SPOTIFY_SCOPES};
use super::token::SharedTokenProvider;

/// Default page size when callers don't specify one. Kept modest so
/// picker lists stay snappy; callers can raise it per-verb.
const DEFAULT_LIMIT: u32 = 50;

/// Spotify Web API handle. Holds a single rspotify client that handles
/// its own token refresh. Cheap to clone (underlying `Arc` everywhere).
#[derive(Clone)]
pub struct SpotifyWebApi {
    client: Arc<AuthCodePkceSpotify>,
}

impl SpotifyWebApi {
    /// Build a client from a [`SharedTokenProvider`] snapshot. Reads the
    /// current rspotify-shaped token (synchronous, no `.await`) and
    /// builds a fresh rspotify client.
    pub fn from_provider(provider: &SharedTokenProvider) -> Self {
        let token = provider.rspotify_token();

        let creds = Credentials::new_pkce(SPOTIFY_CLIENT_ID);
        let oauth = OAuth {
            redirect_uri: SPOTIFY_REDIRECT_URI.to_string(),
            scopes: SPOTIFY_SCOPES.iter().map(|s| (*s).into()).collect(),
            ..OAuth::default()
        };
        let config = Config {
            // Don't let rspotify persist tokens to ~/.spotify_token_cache.json;
            // our on-disk cache in auth.rs is the single source of truth.
            token_cached: false,
            ..Config::default()
        };

        let client = AuthCodePkceSpotify::from_token_with_config(token, creds, oauth, config);
        Self {
            client: Arc::new(client),
        }
    }

    /// Search Spotify for tracks matching `query`. Returns matching
    /// tracks mapped to [`BrowseItem`]s plus the total match count
    /// reported by Spotify.
    ///
    /// The return shape is `(Vec<BrowseItem>, u32)` — the second
    /// element is Spotify's reported `total`, which may exceed the
    /// number of items returned (search is paginated and capped by
    /// `limit`). Callers render `items.len()` of `total` in the picker
    /// footer. When Spotify returns a non-Tracks page (a protocol
    /// surprise because we asked for `SearchType::Track`), the method
    /// returns `(Vec::new(), 0)` rather than erroring.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn demo(api: &clitunes_engine::sources::spotify::webapi::SpotifyWebApi) -> anyhow::Result<()> {
    /// let (items, total) = api.search("boards of canada", Some(10)).await?;
    /// assert!(items.len() <= 10);
    /// assert!(total >= items.len() as u32);
    /// # Ok(()) }
    /// ```
    pub async fn search(&self, query: &str, limit: Option<u32>) -> Result<(Vec<BrowseItem>, u32)> {
        let limit = limit.or(Some(DEFAULT_LIMIT));
        let result = self
            .client
            .search(query, SearchType::Track, None, None, limit, None)
            .await
            .context("spotify search")?;

        match result {
            SearchResult::Tracks(page) => {
                let items = page.items.iter().map(map::track_to_item).collect();
                Ok((items, page.total))
            }
            // We asked for tracks; anything else is a protocol surprise
            // — treat as empty results rather than fail the verb.
            _ => Ok((Vec::new(), 0)),
        }
    }

    /// Fetch the user's saved ("Liked Songs") tracks.
    pub async fn saved_tracks(&self, limit: Option<u32>) -> Result<(Vec<BrowseItem>, u32)> {
        let limit = limit.or(Some(DEFAULT_LIMIT));
        let page = self
            .client
            .current_user_saved_tracks_manual(None, limit, None)
            .await
            .context("spotify saved tracks")?;
        let items = page.items.iter().map(map::saved_track_to_item).collect();
        Ok((items, page.total))
    }

    /// Fetch the user's saved albums.
    pub async fn saved_albums(&self, limit: Option<u32>) -> Result<(Vec<BrowseItem>, u32)> {
        let limit = limit.or(Some(DEFAULT_LIMIT));
        let page = self
            .client
            .current_user_saved_albums_manual(None, limit, None)
            .await
            .context("spotify saved albums")?;
        let items = page.items.iter().map(map::saved_album_to_item).collect();
        Ok((items, page.total))
    }

    /// Fetch the user's own / followed playlists.
    pub async fn playlists(&self, limit: Option<u32>) -> Result<(Vec<BrowseItem>, u32)> {
        let limit = limit.or(Some(DEFAULT_LIMIT));
        let page = self
            .client
            .current_user_playlists_manual(limit, None)
            .await
            .context("spotify user playlists")?;
        let items = page
            .items
            .iter()
            .map(map::simplified_playlist_to_item)
            .collect();
        Ok((items, page.total))
    }

    /// Fetch recently played tracks. The recently-played endpoint is
    /// cursor-paginated and does not carry a total, so we return the
    /// items' length as the "total" to keep the event shape uniform.
    pub async fn recently_played(&self, limit: Option<u32>) -> Result<(Vec<BrowseItem>, u32)> {
        let limit = limit.or(Some(DEFAULT_LIMIT));
        let page = self
            .client
            .current_user_recently_played(limit, None)
            .await
            .context("spotify recently played")?;
        let items: Vec<BrowseItem> = page.items.iter().map(map::play_history_to_item).collect();
        let total = items.len() as u32;
        Ok((items, total))
    }

    /// Fetch tracks on a specific playlist by id or URI.
    /// Returns the playlist's display name alongside items + total.
    pub async fn playlist_tracks(
        &self,
        id_or_uri: &str,
        limit: Option<u32>,
    ) -> Result<(String, Vec<BrowseItem>, u32)> {
        let playlist_id =
            PlaylistId::from_id_or_uri(id_or_uri).context("parse spotify playlist id/uri")?;

        // Fetch the playlist itself first for the display name (Spotify
        // does not return it on the `/items` endpoint).
        let full: FullPlaylist = self
            .client
            .playlist(playlist_id.as_ref(), None, None)
            .await
            .context("spotify playlist")?;

        let limit = limit.or(Some(DEFAULT_LIMIT));
        let page = self
            .client
            .playlist_items_manual(playlist_id.as_ref(), None, None, limit, None)
            .await
            .context("spotify playlist items")?;

        let items = page
            .items
            .iter()
            .filter_map(map::playlist_item_to_item)
            .collect();
        Ok((full.name, items, page.total))
    }
}

/// Private mapping helpers: rspotify model → `BrowseItem`.
///
/// Isolated so tests can exercise them without any network machinery.
mod map {
    use super::*;
    use clitunes_core::sanitize;
    use rspotify::model::PlayableItem;
    use rspotify::prelude::Id;

    /// Pick the largest available cover (Spotify returns images sorted
    /// largest-first; guard anyway in case that ever changes).
    pub(super) fn largest_cover(images: &[Image]) -> Option<String> {
        images
            .iter()
            .max_by_key(|img| img.width.unwrap_or(0) * img.height.unwrap_or(0))
            .map(|img| sanitize(&img.url))
    }

    fn join_artists(artists: &[rspotify::model::SimplifiedArtist]) -> Option<String> {
        if artists.is_empty() {
            return None;
        }
        let names: Vec<String> = artists.iter().map(|a| sanitize(&a.name)).collect();
        Some(names.join(", "))
    }

    fn track_uri(track: &FullTrack) -> Option<String> {
        track.id.as_ref().map(|id| id.uri())
    }

    pub(super) fn track_to_item(track: &FullTrack) -> BrowseItem {
        BrowseItem {
            title: sanitize(&track.name),
            artist: join_artists(&track.artists),
            album: Some(sanitize(&track.album.name)),
            // Local tracks may lack an id; fall back to empty URI so the
            // picker can display them but not attempt Spotify playback.
            uri: track_uri(track).unwrap_or_default(),
            art_url: largest_cover(&track.album.images),
            duration_ms: u64::try_from(track.duration.num_milliseconds()).ok(),
        }
    }

    pub(super) fn saved_track_to_item(saved: &SavedTrack) -> BrowseItem {
        track_to_item(&saved.track)
    }

    pub(super) fn saved_album_to_item(saved: &SavedAlbum) -> BrowseItem {
        let album = &saved.album;
        BrowseItem {
            title: sanitize(&album.name),
            artist: join_artists(&album.artists),
            album: Some(sanitize(&album.name)),
            uri: album.id.uri(),
            art_url: largest_cover(&album.images),
            duration_ms: None,
        }
    }

    pub(super) fn simplified_playlist_to_item(playlist: &SimplifiedPlaylist) -> BrowseItem {
        BrowseItem {
            title: sanitize(&playlist.name),
            artist: Some(sanitize(
                &playlist
                    .owner
                    .display_name
                    .clone()
                    .unwrap_or_else(|| playlist.owner.id.id().to_string()),
            )),
            album: None,
            uri: playlist.id.uri(),
            art_url: largest_cover(&playlist.images),
            duration_ms: None,
        }
    }

    pub(super) fn play_history_to_item(entry: &PlayHistory) -> BrowseItem {
        track_to_item(&entry.track)
    }

    /// Playlists can contain local files or episodes; skip anything
    /// that isn't a playable Spotify track.
    pub(super) fn playlist_item_to_item(item: &PlaylistItem) -> Option<BrowseItem> {
        match item.item.as_ref()? {
            PlayableItem::Track(track) => Some(track_to_item(track)),
            PlayableItem::Episode(_) | PlayableItem::Unknown(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;
    use rspotify::model::{
        AlbumId, Image, PlaylistId, PublicUser, SimplifiedAlbum, SimplifiedArtist, TrackId, Type,
        UserId,
    };
    use std::collections::HashMap;

    fn images_scrambled_sizes() -> Vec<Image> {
        vec![
            Image {
                height: Some(64),
                width: Some(64),
                url: "https://cdn/small".into(),
            },
            Image {
                height: Some(640),
                width: Some(640),
                url: "https://cdn/big".into(),
            },
            Image {
                height: Some(300),
                width: Some(300),
                url: "https://cdn/medium".into(),
            },
        ]
    }

    fn fixture_album() -> SimplifiedAlbum {
        #[allow(deprecated)]
        SimplifiedAlbum {
            album_group: None,
            album_type: Some("album".into()),
            artists: vec![SimplifiedArtist {
                external_urls: HashMap::new(),
                href: None,
                id: None,
                name: "Boards of Canada".into(),
            }],
            available_markets: vec![],
            external_urls: HashMap::new(),
            href: None,
            id: AlbumId::from_id("7dqftJ3kas6D0VAdmt3k3V").ok(),
            images: images_scrambled_sizes(),
            name: "Music Has the Right to Children".into(),
            release_date: None,
            release_date_precision: None,
            restrictions: None,
        }
    }

    fn fixture_track() -> FullTrack {
        #[allow(deprecated)]
        FullTrack {
            album: fixture_album(),
            artists: vec![SimplifiedArtist {
                external_urls: HashMap::new(),
                href: None,
                id: None,
                name: "Boards of Canada".into(),
            }],
            available_markets: vec![],
            disc_number: 1,
            duration: ChronoDuration::milliseconds(222_000),
            explicit: false,
            external_ids: HashMap::new(),
            external_urls: HashMap::new(),
            href: None,
            id: TrackId::from_id("4PTG3Z6ehGkBFwjybzWkR8").ok(),
            is_local: false,
            is_playable: None,
            linked_from: None,
            restrictions: None,
            name: "Roygbiv".into(),
            popularity: 0,
            preview_url: None,
            track_number: 8,
            r#type: Type::Track,
        }
    }

    #[test]
    fn largest_cover_picks_highest_pixel_count() {
        let picked = map::largest_cover(&images_scrambled_sizes());
        assert_eq!(picked.as_deref(), Some("https://cdn/big"));
    }

    #[test]
    fn largest_cover_empty_is_none() {
        assert!(map::largest_cover(&[]).is_none());
    }

    #[test]
    fn track_to_item_populates_all_fields() {
        let item = map::track_to_item(&fixture_track());
        assert_eq!(item.title, "Roygbiv");
        assert_eq!(item.artist.as_deref(), Some("Boards of Canada"));
        assert_eq!(
            item.album.as_deref(),
            Some("Music Has the Right to Children")
        );
        assert_eq!(item.uri, "spotify:track:4PTG3Z6ehGkBFwjybzWkR8");
        assert_eq!(item.art_url.as_deref(), Some("https://cdn/big"));
        assert_eq!(item.duration_ms, Some(222_000));
    }

    #[test]
    fn track_with_no_album_art_has_none_art_url() {
        let mut track = fixture_track();
        track.album.images.clear();
        let item = map::track_to_item(&track);
        assert!(item.art_url.is_none());
    }

    #[test]
    fn local_track_without_id_has_empty_uri() {
        let mut track = fixture_track();
        track.id = None;
        track.is_local = true;
        let item = map::track_to_item(&track);
        assert_eq!(item.uri, "");
    }

    #[test]
    fn simplified_playlist_prefers_display_name_then_user_id() {
        #[allow(deprecated)]
        let owner_with_name = PublicUser {
            display_name: Some("Daisy".into()),
            external_urls: HashMap::new(),
            followers: None,
            href: String::new(),
            id: UserId::from_id("user123").unwrap(),
            images: Vec::new(),
        };
        #[allow(deprecated)]
        let playlist = SimplifiedPlaylist {
            collaborative: false,
            external_urls: HashMap::new(),
            href: String::new(),
            id: PlaylistId::from_id("37i9dQZF1DXcBWIGoYBM5M").unwrap(),
            images: images_scrambled_sizes(),
            name: "Today's Top Hits".into(),
            owner: owner_with_name,
            public: Some(true),
            snapshot_id: "abc".into(),
            tracks: Default::default(),
            items: Default::default(),
        };
        let item = map::simplified_playlist_to_item(&playlist);
        assert_eq!(item.title, "Today's Top Hits");
        assert_eq!(item.artist.as_deref(), Some("Daisy"));
        assert_eq!(item.uri, "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M");
    }
}
