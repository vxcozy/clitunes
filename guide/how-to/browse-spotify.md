# Browse and search Spotify

Once your Spotify account is [authenticated](play-spotify.md), clitunes
lets you search the full Spotify catalogue, browse your saved library, and
drill into playlists — either from the TUI picker or from headless CLI
commands that emit JSON on stdout.

Everything on this page requires the `webapi` feature (enabled in the
default build) and a cached credential file at
`~/.config/clitunes/spotify/credentials.json`. Run `clitunes auth` first
if you haven't authenticated yet.

## From the TUI

Open the picker with **s**. The picker has four tabs — switch between
them with **Tab** (next) and **Shift+Tab** (previous):

| Tab | Contents |
| --- | --- |
| **Radio** | Curated station list (works without Spotify) |
| **Search** | Spotify track search — type to query |
| **Library** | Your saved tracks, saved albums, playlists, and recently played |
| **Playlist** | Tracks inside a playlist you selected from the Library tab |

Inside each tab:

- **↑ / ↓** move the selection
- **Enter** plays the highlighted track, opens the highlighted playlist,
  or drills into the highlighted library category
- **Esc** closes the picker (or backs out one level when drilling through
  categories)

On the **Search** tab the letters *n* and *p* type into the search field
rather than cycling visualisers — clitunes routes them back to viz
navigation only when a non-search tab is focused.

Search results are debounced: keep typing and clitunes waits ~300 ms
after your last keystroke before sending the query, so a fast-typed query
only costs one round-trip.

## From the command line

Three headless subcommands mirror the picker tabs and emit one JSON line
per result event to stdout. Each exits non-zero if authentication fails
or the daemon can't reach Spotify.

### Search

```
clitunes search "<query>" [limit]
```

Example:

```
$ clitunes search "boards of canada" 5
{"type":"search_results","query":"boards of canada","items":[...],"total":842}
```

The JSON line's `items` field contains up to `limit` (default 50)
`BrowseItem`s — `{title, artist, album, uri, art_url, duration_ms}`.
The `total` field is Spotify's reported match count, which may exceed
`items.len()`.

### Browse your library

```
clitunes browse <category> [limit]
```

Valid categories:

- `saved_tracks` — your Liked Songs
- `saved_albums` — albums you've saved
- `playlists` — playlists you own or follow
- `recently_played` — your recent play history (cursor-paginated; the
  `total` in the response echoes `items.len()` because Spotify doesn't
  report a cumulative total for this endpoint)

Example:

```
$ clitunes browse playlists 10
{"type":"library_results","category":"playlists","items":[...],"total":37}
```

### Browse a playlist

```
clitunes browse-playlist <id-or-uri> [limit]
```

Accepts either a bare playlist id (`37i9dQZF1DXcBWIGoYBM5M`) or a full
URI (`spotify:playlist:37i9dQZF1DXcBWIGoYBM5M`).

```
$ clitunes browse-playlist spotify:playlist:37i9dQZF1DXcBWIGoYBM5M 20
{"type":"playlist_results","name":"Today's Top Hits","items":[...],"total":50}
```

## Play a result

Every `BrowseItem` carries a `uri`. Feed it to `clitunes source` to start
playback:

```
$ URI=$(clitunes search "roygbiv" 1 | jq -r '.items[0].uri')
$ clitunes source "$URI"
```

Tracks without a Spotify URI (local files inside a playlist) are returned
with an empty `uri` and are skipped by the picker — the JSON carries them
through so you can filter them yourself.

## Album art

When a track with artwork starts playing, the TUI client fetches the
cover from Spotify's CDN and renders it in the upper-right corner of the
visualiser pane using truecolor halfblocks. Covers are cached in memory
for the life of the track — switching tracks re-fetches, but returning
to the same track does not.

If your terminal doesn't support truecolor SGR (most modern terminals
do), the art still renders but may look posterised.

## Troubleshooting

### `no cached Spotify credentials`

The daemon can't find a credential file. Run `clitunes auth` (or play any
Spotify URI from the client) to seed one.

### `cached Spotify credentials lack required scopes`

The credential file was created before v1.2 and doesn't cover the new
browse/search scopes (`user-library-read`, `playlist-read-private`,
`user-read-recently-played`). Re-run `clitunes auth` — it detects the
missing scopes and re-authorises.

### Search returns zero results despite a valid query

Spotify occasionally returns a non-Tracks page even when you ask for
tracks. clitunes treats that as empty rather than erroring — retry the
query, or try a slightly different wording.

### HTTP 429 "Too Many Requests" on search/library

The default build shares librespot's embedded OAuth client ID with every
other librespot-based player, and Spotify rate-limits the combined
traffic. During peak hours a cold search can 429 immediately. Register
your own Spotify Developer App and set `$CLITUNES_SPOTIFY_CLIENT_ID` —
see [Play Spotify tracks → Use your own Spotify Developer App](play-spotify.md#use-your-own-spotify-developer-app-optional)
for the exact steps.
