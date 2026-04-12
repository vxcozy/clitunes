# Architecture

## Daemon/client split

clitunes separates audio handling from rendering:

```
clitunes (client)          clitunesd (daemon)
┌──────────────┐           ┌──────────────────┐
│ visualiser   │◄─ SPMC ──►│ radio / local src │
│ picker       │   ring    │ PCM decode        │
│ ANSI render  │           │ cpal audio out    │
└──────┬───────┘           └────────┬──────────┘
       │ Unix socket control bus    │
       └────────────────────────────┘
```

**Why split?** A single long-running daemon can maintain audio continuity while
multiple clients connect and disconnect. You can close the TUI, open a
`--pane visualiser` in tmux, switch to a `mini-spectrum` in your status bar —
the music never stops because the daemon owns the audio pipeline.

## D15: the dependency firewall

The daemon binary (`clitunesd`) must never depend on visualiser, TUI, or GPU
crates. This is enforced as D15 — a CI check that greps the daemon's
dependency tree for `ratatui`, `crossterm`, and `wgpu`.

**Why?** The daemon runs headless, potentially as a background service. Pulling
in GPU or terminal libraries would bloat its binary, introduce unnecessary
failure modes, and violate the single-responsibility boundary.

Feature gates in `clitunes-engine` make this possible: the daemon enables
`audio`, `sources`, `control`, and `decode`; the client adds `visualiser`,
`tui`, and `layout`.

## SPMC PCM ring

Audio data travels from daemon to clients via a shared-memory single-producer
multi-consumer ring buffer, not over the Unix socket.

**Why shared memory?** At 48 kHz stereo float32, PCM data is ~375 KB/s. Socket
I/O would add syscall overhead per frame and require each client to buffer
independently. Shared memory gives every client zero-copy access to the same
ring with no per-frame kernel transitions.

**How it works:**
1. The daemon creates a POSIX shared memory region (`shm_open`)
2. It writes PCM frames into the ring as audio decodes
3. Each client maps the same region and reads at its own pace
4. Sequence numbers let clients detect if they've fallen behind

The `PcmTap` event on the control bus tells clients the shm region name,
sample rate, channel count, and ring capacity.

## Visualiser rendering

All eight visualisers render to a `CellGrid` — an in-memory grid of half-block
Unicode cells with 24-bit foreground and background colors. The `AnsiWriter`
flushes this grid to stdout as ANSI escape sequences at ~30 fps.

**Why not wgpu/GPU?** An early prototype (the "spike" phase) used wgpu
rendering with Kitty graphics protocol for terminal display. Per-frame GPU
readback and base64 transmission dominated CPU and battery. The CPU half-block
approach is simpler, more portable (works in any 24-bit color terminal), and
fast enough — the bottleneck is terminal parsing, not rendering.

## State persistence

The client saves the last-played station UUID to `~/.config/clitunes/state.toml`
using atomic file writes (write to temp file, then rename). On next launch with
`--source auto` (the default), it resumes the last station automatically.

## Idle shutdown

The daemon tracks connected clients. When the last client disconnects, a
30-second idle timer starts. If no client reconnects within that window, the
daemon exits cleanly. This keeps the daemon from lingering indefinitely while
still giving you time to restart the TUI without losing audio state.
