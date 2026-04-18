# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [1.2.0] - 2026-04-18

### Added

- **Settings tab in the picker.** Fourth tab next to Radio / Search /
  Library, opened with `4` or Tab-cycled. Shows Spotify auth status
  (Logged in / Logged out / Needs re-auth / Error with reason),
  Connect device name, resolved `daemon.toml` path, credentials path,
  and a context-sensitive instruction. Enter re-requests a fresh
  snapshot. New `Verb::ReadConfig` / `Event::ConfigSnapshot` IPC pair
  carries the payload.
- **In-TUI Spotify sign-in.** On the Settings tab, press `a` to start
  the OAuth flow without leaving the TUI. Daemon opens the user's
  default browser, reports `AuthStarted` / `AuthCompleted` /
  `AuthFailed` events that the tab renders in real time. On success
  the Settings tab auto-flips to "Logged in". 5-minute daemon
  timeout; 5-minute-30-second client timeout so the UI recovers even
  if the daemon crashes mid-flow. Idempotent — a second `a` press
  while a flow is in progress is silently ignored. Reconnect-safe:
  the Settings tab re-syncs against the daemon's actual auth state
  after a control-socket drop.
- **FFT signal-path diagnostics.** `RUST_LOG=clitunes::audio=trace`
  now emits a per-60-frame line with `frames_read`, `peak_sample`,
  `peak_mag`, and `sample_rate` so audio-pipeline health is
  observable without a custom build. New regression test on
  `FftTap::snapshot_from` pins sinusoid-amplitude preservation and
  FFT bin-location correctness.

### Changed

- **Visualisers render edge-to-edge.** Removed the defensive 1-col /
  2-row inset on the cell grid. `AnsiWriter` now disables DECAWM for
  the render session (re-enabled on shutdown) so writing the
  bottom-right cell no longer risks a terminal scroll. The full pane
  is now usable.
- **Braille visualisers fill their grid.** Scope's intrinsically-
  square Lissajous letterboxes with a muted phosphor-green tint
  instead of black; Wave, Heartbeat, and BarsDot paint empty cells
  with palette-tinted backgrounds instead of raw black; Pulse's disc
  now grows against the longer half-axis so it fills wide panes.
- **Bar-family visualisers scale with the audio.** `bars_dot`,
  `bars_outline`, and `classic_peak` now pipe FFT magnitudes through
  a shared `SpectrumScaler` that applies log/dB compression
  (`-60 dB` → `0 dB` → `[0, 1.0]`) plus a decaying peak tracker so
  bars reach 80–90 % of pane height at typical listening volumes,
  stay visible through quiet passages, and peak cleanly on loud
  transients without clipping.
- **Snappier reactivity across the braille catalogue.** Shared AGC
  attack tightened 50 ms → 25 ms, release 2.5 s → 1.2 s;
  `EnergyTracker` release retain 0.88 → 0.75 (258 ms → 115 ms) across
  heartbeat, wave, scope, matrix, terrain, retro, rain, binary,
  butterfly, scatter, fire, sakura, ripples, and the bar family.
  Pulse, Firework, Tunnel, Vortex, Plasma, Metaballs, and Moire left
  alone on purpose — their onset-detection or texture-evolution math
  depends on slower envelopes.
- **Settings-tab sign-in copy.** Reads `Press `a` to sign in — opens
  Spotify in your browser.` for the active case, with tailored
  variants for re-auth and scope-insufficient states. Headless / SSH
  users see a pending-state hint that still points at the `clitunes
  auth` subcommand as a fallback since the browser path may not
  work remotely.

### Fixed

- **Client recovers from mid-flow daemon crash.** The Settings tab
  no longer gets stuck in "Opening browser…" forever if the daemon
  dies between `AuthStarted` and `AuthCompleted`.
- **Control-session reconnect clears stale auth state.** On
  daemon-reconnect the client drops the in-progress flag and
  re-issues `Verb::ReadConfig`, so the Settings tab reflects the
  daemon's actual view instead of a phantom pending flow.

## [1.1.0] - 2026-04-18

### Added

- **`:viz <name>` command bar in the full TUI.** Press `:` to open a
  bottom-row overlay, type a visualiser name (partial / fuzzy match
  OK — `:sak` jumps to Sakura, `:hrt` jumps to Heartbeat), hit Enter
  to go there. Tied top matches show a disambiguation hint and wait
  for the user to refine the query. Esc cancels, Backspace edits.
  Submit waits up to 250 ms for the daemon's `VizChanged` ack;
  surfaces "daemon not responding" on timeout.
- **Discoverability hint.** When no modal is active and the now-
  playing strip is empty (first-run), a dim bottom-row hint ghosts
  over the visualiser showing `:jump  n/p cycle  s picker  q quit`.
- **Pane-mode visualiser parity.** `clitunes --pane visualiser
  --viz <name>` now accepts all 23 modes. Previously only 8 were
  wired in pane mode — a pre-existing gap from PR #37 that silently
  fell back to Plasma on unknown names.
- **`rust-toolchain.toml`** pinning the project to rustc 1.95.0 so
  local clippy fires the same lints CI does. Dev-only.

### Changed

- **Default visualiser corrected to Plasma in all docs.** The code
  has defaulted to Plasma (`active_idx = 0`) since v1.0.0, but the
  README and tutorial materials said "Auralis (default)" — a pre-
  existing documentation bug. README, CHANGELOG (below),
  `guide/tutorials/getting-started.md`, `guide/how-to/embed-panes.md`,
  and `guide/explanation/visualisers.md` now all say Plasma.

### Fixed

- **`rustls-webpki` 0.103.11 → 0.103.12** closes two Low-severity
  CVEs on the 0.103.x path (name-constraint matching for wildcard
  names and URI names). The 0.102.8 path via librespot's
  hyper-proxy2 remains on the older version pending an upstream
  update — tracked separately.

### Removed

- **Four visualiser variants (`Auralis`, `Starfield`, `Tideline`,
  `Cascade`) that were declared in the `VisualiserId` enum but
  never registered in the TUI carousel.** These were early GPU-
  heavy designs superseded by the pure-CPU rendering approach. The
  v1.0.0 CHANGELOG and README advertised them as part of the
  "Spectrum / core" family, but the active carousel only contained
  23 reachable modes. `clitunes viz auralis` (or starfield /
  tideline / cascade) now returns an unknown-visualiser error
  instead of silently no-oping.

[1.1.0]: https://github.com/vxcozy/clitunes/releases/tag/v1.1.0

## [1.0.0] - 2026-04-17

First public release.

### Architecture

- Daemon/client split — `clitunesd` handles audio while `clitunes` renders
  the TUI. Music keeps playing when you close the terminal.
- Unix socket control bus with line-delimited JSON protocol.
- Shared-memory SPMC ring for zero-copy PCM delivery to multiple clients.
- Daemon auto-start on first `clitunes` invocation with 30-second idle
  shutdown after all clients disconnect.

### Sources

- **Internet radio** — curated station picker backed by radio-browser.info.
- **Local files** — MP3, FLAC, OGG, WAV, AAC/M4A with tag reading and
  recursive folder scan.
- **Spotify (URI playback)** — paste a track/album/playlist URI, clitunes
  authenticates via OAuth and streams via librespot. Premium required.
- **Spotify Connect receiver** — advertises clitunes as a Connect-capable
  device on the LAN; control from any Spotify client. Opt-in via
  `[connect].enabled = true` in daemon config (off by default).

### Visualisers

Twenty-three real-time visualisers reactive to the audio spectrum, rendered
at 30 fps using half-block ANSI, density-ramp glyphs, or Unicode braille
sub-pixels (terminal 24-bit colour required):

- **Spectrum / core:** Plasma (default), Ripples, Tunnel, Metaballs, Fire,
  Matrix, Moire, Vortex
- **Oscilloscope (braille):** Wave, Scope, Heartbeat
- **Spectrum variants:** ClassicPeak, BarsDot, BarsOutline, Binary
- **Particle / field:** Scatter, Terrain, Butterfly, Pulse
- **Animated scenes:** Rain, Sakura, Firework, Retro

### Interfaces

- Full TUI — picker + visualiser carousel with audio-reactive FFT energy.
- Standalone pane modes: `--pane visualiser`, `--pane now-playing`,
  `--pane mini-spectrum` for tmux, WezTerm, or Ghostty layouts.
- Headless CLI: `play`, `pause`, `next`, `prev`, `volume`, `viz`, `source`,
  `status --json`, `connect disconnect`.
- Diataxis-structured documentation in `guide/`.

### Security hardening

- Umask-atomic socket bind (SEC-001).
- Peercred UID gate — only the session user can connect.
- Terminal escape sanitisation (D20).
- Control bus DoS protection (SEC-007).
- State persistence — resumes your last station across sessions.

### Platforms

- **macOS 13+** on Apple Silicon and Intel.
- **Linux** with glibc 2.35+ (Ubuntu 22.04, Debian 12, RHEL 9 and newer).
  Requires `alsa-lib` at runtime (Homebrew formula declares this).
- Windows not supported — clitunes does not build on Windows.

### Install

- **Homebrew** (macOS + Linux): `brew install vxcozy/tap/clitunes`
- **Cargo** (any Rust toolchain): `cargo install --git https://github.com/vxcozy/clitunes --tag v1.0.0 --locked`
- **Direct download**: https://github.com/vxcozy/clitunes/releases/tag/v1.0.0

### Known limitations

- **Unsigned macOS binaries.** Release binaries are ad-hoc codesigned
  (`codesign --force --deep --sign -`) but not notarised by Apple.
  Gatekeeper may show a soft first-run prompt; use Right-click → Open,
  or System Settings → Privacy & Security → Open Anyway. Full Apple
  notarisation is planned for v1.0.x.
- **Unsigned tags.** The v1.0.0 git tag is not GPG-signed. Integrity relies
  on GitHub's transport + repo access controls, not cryptographic
  signatures. Signed tags with a published key fingerprint are planned
  for v1.1.
- **librespot patched from dev branch.** The `[patch.crates-io]` block
  pins librespot-* crates to commit `33bf3a77ed4b549df67e8347d7d6e55b007b3ec2`
  on the librespot-org/librespot `dev` branch (14 commits ahead of the
  v0.8.0 crates.io release). This works around the vergen-gitcl build
  conflict in 0.8.0. Audited: the 14 commits are bug fixes + additive
  API only. Will revert to plain crates.io when librespot cuts 0.8.1.
- **No crates.io publish.** Consequence of the patch block above —
  `cargo publish` would strip `[patch]` and ship a crate that fails to
  build. Use the `--git --tag` install form until librespot 0.8.1 ships.
- **Linux glibc floor of 2.35.** Binaries built on Ubuntu 22.04; older
  distros (Debian 11, RHEL 8, Ubuntu 20.04) must build from source.

[1.0.0]: https://github.com/vxcozy/clitunes/releases/tag/v1.0.0
