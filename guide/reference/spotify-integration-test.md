# Spotify Web API integration test

An opt-in end-to-end test that exercises the real Spotify Web API
through `SpotifyWebApi` (`crates/clitunes-engine/src/sources/spotify/webapi.rs`).

## Opting in

```
1. Authenticate: clitunes auth
2. Set: CLITUNES_SPOTIFY_INTEGRATION_TEST=1
3. Run:  cargo test -p clitunes-engine --all-features --test spotify_webapi_integration
```

Without the env var, or when the on-disk credential cache is missing,
every test prints a skip reason to stderr and returns `ok` so CI stays
green.

## What it tests

| Test | Verb exercised |
|------|----------------|
| `search_returns_tracks_for_common_query` | `SpotifyWebApi::search("beatles", Some(5))` |
| `saved_tracks_returns_consistent_shape` | `SpotifyWebApi::saved_tracks(Some(5))` |
| `playlist_tracks_on_todays_top_hits_returns_a_name` | `SpotifyWebApi::playlist_tracks("37i9dQZF1DXcBWIGoYBM5M", Some(3))` |

## What it asserts

Shape only, never content. Specifically:

- `items.len() <= requested_limit`
- `total >= items.len() as u32` (where `total` is returned)
- `uri` starts with `"spotify:track:"` or is empty (local files)
- `title` is non-empty
- saved tracks have at least one of `artist` / `album` populated
- playlist `name` is non-empty

Account-specific fields (exact titles, counts, orderings) are never
asserted, so the tests are stable across library changes.

## Why it's opt-in

The test requires live credentials on disk and outbound HTTPS to
`api.spotify.com`. Running it unconditionally would either fail in CI
(no credentials) or couple the test suite to Spotify's availability.
The gate (`CLITUNES_SPOTIFY_INTEGRATION_TEST=1` **and** a readable
credential cache at `default_credentials_path()`) ensures the test
only runs when a developer explicitly asks for it.

## Gate

The test file is compiled only when the `webapi` Cargo feature is on
(`#![cfg(feature = "webapi")]`). Each `#[tokio::test]` also checks
the env var and credential path at runtime; any missing prerequisite
short-circuits with an `eprintln!` skip message and passes.
