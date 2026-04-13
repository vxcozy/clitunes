# Play Spotify tracks

clitunes can play Spotify tracks via librespot. This requires a **Spotify
Premium** account — free-tier accounts are not supported by librespot.

> **Community/experimental feature.** librespot reverse-engineers Spotify's
> proprietary protocol. While widely used, this technically falls outside
> Spotify's Developer Terms. clitunes will ask for your consent on first
> authentication. Use at your own discretion.

## First-time setup

On your first Spotify command, clitunes opens a browser for OAuth
authentication:

```
clitunes source spotify:track:4PTG3Z6ehGkBFwjybzWkR8
```

1. A browser window opens to Spotify's login page
2. Log in with your **Premium** account
3. Authorize clitunes
4. The browser redirects to `http://127.0.0.1:8898/login` — clitunes captures
   the token automatically

Credentials are cached at `~/.config/clitunes/spotify/credentials.json`
(mode 0600) and refreshed automatically on subsequent launches.

### Headless / SSH sessions

If no browser is available, clitunes prints the authorization URL to stderr.
Copy it to a machine with a browser, complete the login, then paste the
redirect URL back into the terminal.

## Play a track

```
clitunes source spotify:track:4PTG3Z6ehGkBFwjybzWkR8
```

The URI format matches Spotify's standard scheme. You can find track URIs in
the Spotify desktop app: right-click a track → Share → Copy Spotify URI.

## From the TUI

Run the source command from a second terminal while the TUI is open. The
visualiser switches to displaying Spotify audio, and the now-playing bar
updates with track metadata (artist, title, album).

## Switching back to radio

```
clitunes source radio <station-uuid>
```

Or press **s** in the TUI to reopen the station picker.

## Spotify Connect (v1.2)

Spotify Connect support — letting clitunes appear as a playback target in
the Spotify app — is planned for v1.2. This will allow you to start playback
from your phone or the Spotify desktop app and route audio to clitunes.

## Troubleshooting

### "Spotify Premium required"

librespot only works with Premium accounts. If you see an authentication
error, verify your account tier at
[spotify.com/account](https://spotify.com/account).

### Authentication fails or hangs

Delete the cached credentials and re-authenticate:

```
rm ~/.config/clitunes/spotify/credentials.json
clitunes source spotify:track:...
```

### Track unavailable

Some tracks are region-locked or removed from Spotify. The daemon emits a
`source_error` event and falls back to the tone generator. Check the track
URI is valid and available in your region.

### Session disconnects

If another Spotify client takes over playback (only one device can stream at
a time), clitunes attempts automatic reconnection with exponential backoff
(1s, 2s, 4s). If reconnection fails, it falls back to the tone generator and
emits a `source_error` event.
