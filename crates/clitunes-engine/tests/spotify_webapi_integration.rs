//! End-to-end sanity test for the real Spotify Web API.
//!
//! Gated behind `CLITUNES_SPOTIFY_INTEGRATION_TEST=1` AND an on-disk
//! credential cache. Skipped (early-return, prints reason) otherwise so
//! CI stays green without live credentials.
//!
//! Run opt-in:
//!   CLITUNES_SPOTIFY_INTEGRATION_TEST=1 \
//!     cargo test -p clitunes-engine --all-features \
//!     --test spotify_webapi_integration
//!
//! Every assertion is shape-only (lengths, URI prefixes, non-empty
//! strings) so the tests are stable across account state.

#![cfg(feature = "webapi")]

use clitunes_engine::sources::spotify::{
    default_credentials_path, load_credentials, token::SharedTokenProvider, webapi::SpotifyWebApi,
};

/// Apply the opt-in gate + load the on-disk credential cache. Returns
/// `None` (after `eprintln!`-ing a reason) when the test should skip.
async fn build_api() -> Option<SpotifyWebApi> {
    if std::env::var("CLITUNES_SPOTIFY_INTEGRATION_TEST")
        .ok()
        .as_deref()
        != Some("1")
    {
        eprintln!("skipping: CLITUNES_SPOTIFY_INTEGRATION_TEST != 1");
        return None;
    }

    let cred_path = match default_credentials_path() {
        Some(p) => p,
        None => {
            eprintln!("skipping: default_credentials_path() did not resolve (set $HOME?)");
            return None;
        }
    };

    if !cred_path.exists() {
        eprintln!(
            "skipping: credential cache not found at {} (run `clitunes auth`)",
            cred_path.display()
        );
        return None;
    }

    let cred_path_clone = cred_path.clone();
    let auth_result =
        match tokio::task::spawn_blocking(move || load_credentials(&cred_path_clone)).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                eprintln!("skipping: load_credentials failed: {e:#}");
                return None;
            }
            Err(e) => {
                eprintln!("skipping: credential task panicked: {e}");
                return None;
            }
        };

    let provider = SharedTokenProvider::new(auth_result.token, cred_path);
    Some(SpotifyWebApi::from_provider(&provider))
}

#[tokio::test(flavor = "multi_thread")]
async fn search_returns_tracks_for_common_query() {
    let api = match build_api().await {
        Some(api) => api,
        None => return,
    };

    let (items, total) = api
        .search("beatles", Some(5))
        .await
        .expect("spotify search should succeed with valid credentials");

    assert!(
        items.len() <= 5,
        "search honoured limit=5, got {} items",
        items.len()
    );
    assert!(
        total >= items.len() as u32,
        "total ({}) must be >= items.len() ({})",
        total,
        items.len()
    );

    if let Some(first) = items.first() {
        assert!(
            first.uri.starts_with("spotify:track:") || first.uri.is_empty(),
            "track uri should be spotify:track:* or empty (local file), got {:?}",
            first.uri
        );
        assert!(
            !first.title.is_empty(),
            "track title should be non-empty, got {:?}",
            first.title
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn saved_tracks_returns_consistent_shape() {
    let api = match build_api().await {
        Some(api) => api,
        None => return,
    };

    let (items, _total) = api
        .saved_tracks(Some(5))
        .await
        .expect("saved_tracks should succeed with valid credentials");

    assert!(
        items.len() <= 5,
        "saved_tracks honoured limit=5, got {} items",
        items.len()
    );

    for (idx, item) in items.iter().enumerate() {
        assert!(
            !item.title.is_empty(),
            "saved track #{idx} title should be non-empty"
        );
        assert!(
            item.artist.is_some() || item.album.is_some(),
            "saved track #{idx} must have at least one of artist/album populated"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn playlist_tracks_on_todays_top_hits_returns_a_name() {
    let api = match build_api().await {
        Some(api) => api,
        None => return,
    };

    let (name, items, _total) = api
        .playlist_tracks("37i9dQZF1DXcBWIGoYBM5M", Some(3))
        .await
        .expect("playlist_tracks should succeed for Today's Top Hits");

    assert!(!name.is_empty(), "playlist name should be non-empty");
    assert!(
        items.len() <= 3,
        "playlist_tracks honoured limit=3, got {} items",
        items.len()
    );

    for (idx, item) in items.iter().enumerate() {
        assert!(
            item.uri.starts_with("spotify:track:") || item.uri.is_empty(),
            "playlist item #{idx} uri should be spotify:track:* or empty, got {:?}",
            item.uri
        );
    }
}
