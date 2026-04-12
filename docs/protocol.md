# clitunes Control Protocol v1

Line-delimited JSON over a Unix stream socket at
`$XDG_RUNTIME_DIR/clitunes/clitunesd.sock` (macOS fallback:
`$TMPDIR/$USER/clitunes/clitunesd.sock`).

Each line is a complete JSON object terminated by `\n`. Max line length:
**65,536 bytes** (connections sending oversized lines are disconnected).

## Banner Exchange

On connect the server sends its banner immediately:

```json
{"version":"clitunes-control-1","capabilities":["radio","local","viz_auralis","viz_tideline"]}
```

The client responds with its banner:

```json
{"client":"clitunes-tui","version":"1.0.0","subscribe":["now_playing","state"]}
```

Both sides validate the `version` field. On mismatch, both disconnect.
The `subscribe` array in the client banner is a convenience shortcut for
initial subscriptions; clients can also subscribe dynamically after
handshake.

## Verbs (client -> daemon)

Every verb is wrapped in an envelope with a client-generated `cmd_id`:

```json
{"cmd_id":"abc-1","verb":"play"}
```

The daemon echoes the `cmd_id` in its `command_result` response so the
client can correlate request/response.

### Verb List

| Verb | Args | Description |
|------|------|-------------|
| `play` | — | Start/resume playback |
| `pause` | — | Pause playback |
| `next` | — | Advance to next track (queue/local; no-op for radio) |
| `prev` | — | Go to previous track |
| `volume` | `{"level": 0..100}` | Set output volume |
| `source` | `{"type":"local","path":"..."}` or `{"type":"radio","uuid":"..."}` | Switch source |
| `viz` | `{"name": "auralis"}` | Switch visualiser |
| `layout` | `{"name": "default"}` | Switch layout |
| `picker` | — | Show the curated picker overlay |
| `status` | — | Request a one-shot status snapshot |
| `subscribe` | `{"topic": "now_playing"}` | Start receiving events for topic |
| `unsubscribe` | `{"topic": "now_playing"}` | Stop receiving events for topic |
| `quit` | — | Disconnect cleanly |
| `capabilities` | — | Query daemon capabilities |

### Example: Change Volume

```json
{"cmd_id":"v-42","verb":"volume","args":{"level":75}}
```

Response:

```json
{"event":"command_result","data":{"cmd_id":"v-42","ok":true}}
```

## Events (daemon -> client)

Events are only delivered to clients subscribed to the relevant topic.

### Event List

| Event | Topic | Fields |
|-------|-------|--------|
| `state_changed` | `state` | `state`, `source`, `station_or_path`, `position_secs`, `duration_secs` |
| `now_playing_changed` | `now_playing` | `artist`, `title`, `album`, `station`, `raw_stream_title` |
| `source_error` | `errors` | `source`, `error` |
| `daemon_shutting_down` | `state` | `reason` |
| `volume_changed` | `state` | `volume` |
| `viz_changed` | `state` | `name` |
| `layout_changed` | `state` | `name` |
| `pcm_meta` | `pcm_meta` | `sample_rate`, `channels`, `frame_count_total` |
| `command_result` | (always delivered) | `cmd_id`, `ok`, `error` (optional) |

### Topics

- `state` — playback state changes, volume, viz, layout, shutdown
- `now_playing` — track/stream title changes
- `errors` — source errors
- `pcm_meta` — PCM format metadata

### Example: Now Playing Event

```json
{"event":"now_playing_changed","data":{"artist":"Boards of Canada","title":"Roygbiv","album":null,"station":"SomaFM","raw_stream_title":"Boards of Canada - Roygbiv"}}
```

## Error Handling

- **Malformed JSON**: `command_result` with `ok: false` and error message
- **Unknown verb**: `command_result` with `ok: false`
- **Oversized line (>64KB)**: connection terminated
- **Handshake timeout (5s)**: connection terminated
- **Slow client (event queue full)**: connection terminated; other clients unaffected

## Versioning Policy

- **Non-breaking**: adding new verbs or events (clients ignore unknown events)
- **Breaking**: removing verbs, changing existing verb/event shapes
- Breaking changes require bumping the version in the banner (e.g., `clitunes-control-2`)

## Debug

```sh
socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/clitunes/clitunesd.sock
```

Type verbs as JSON lines to interact with the daemon manually.
