# Control bus protocol

clitunes uses a line-delimited JSON protocol over a Unix domain socket for
communication between client and daemon.

## Socket location

```
$XDG_RUNTIME_DIR/clitunes/clitunesd.sock    # if XDG_RUNTIME_DIR is set
$TMPDIR/$USER/clitunes/clitunesd.sock        # fallback
```

The socket is created with mode `0600` and the directory with `0700`. The
daemon verifies the connecting process UID via `SO_PEERCRED` (Linux) or
`LOCAL_PEERCRED` (macOS).

## Wire format

Each message is a single JSON object followed by a newline (`\n`). The
connection begins with a banner exchange.

### Banner (daemon → client)

```json
{"banner":"clitunesd","version":"1.0.0","capabilities":["play","pause","source","viz","volume","picker","status"]}
```

### Verbs (client → daemon)

| Verb | Payload | Description |
|------|---------|-------------|
| `play` | — | Resume playback |
| `pause` | — | Pause playback |
| `next` | — | Next station |
| `prev` | — | Previous station |
| `volume` | `{"level": 0-100}` | Set volume |
| `viz` | `{"name": "..."}` | Switch visualiser |
| `source` | `{"radio": {"uuid": "..."}}` or `{"local": {"path": "..."}}` | Switch source |
| `status` | — | Request current state |
| `quit` | — | Request daemon shutdown |

Example:

```json
{"cmd_id":"abc123","verb":"source","args":{"radio":{"uuid":"..."}}}
```

### Events (daemon → client)

| Event | Description |
|-------|-------------|
| `PcmTap` | Shared memory ring details (shm name, sample rate, channels, capacity) |
| `StateChanged` | Playback state update (playing/paused, source, station/path) |
| `VizChanged` | Active visualiser changed |
| `VolumeChanged` | Volume level changed |
| `SourceError` | Source failed (with error message) |
| `CommandResult` | Success/failure response to a verb |
| `DaemonShuttingDown` | Daemon is exiting (with reason) |

### Command results

Every verb receives a `CommandResult` event:

```json
{"topic":"command","cmd_id":"abc123","ok":true}
{"topic":"command","cmd_id":"abc123","ok":false,"error":"reason"}
```

## PCM tap

Audio data is delivered via a shared-memory SPMC (single-producer,
multi-consumer) ring, not over the socket. The `PcmTap` event provides the
shared memory region name for `shm_open(3)`. See
[Architecture](../explanation/architecture.md) for details.
