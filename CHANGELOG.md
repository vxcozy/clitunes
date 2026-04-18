# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Removed

- **Four visualiser variants (`Auralis`, `Starfield`, `Tideline`, `Cascade`)
  that were declared in the `VisualiserId` enum but never registered in
  the TUI carousel.** These were early GPU-heavy designs superseded by
  the pure-CPU rendering approach. The v1.0.0 CHANGELOG and README
  advertised them as part of the "Spectrum / core" family, but the
  active carousel only contained 23 reachable modes — the other four
  were dead code behind a stable-looking name. Removing them closes
  the gap between what ships and what the docs claim.

### Changed

- **Default visualiser corrected to Plasma in all docs.** The code has
  defaulted to Plasma (`active_idx = 0`) since v1.0.0, but the README and
  tutorial materials said "Auralis (default)" — a pre-existing
  documentation bug surfaced during v1.3 planning. README, CHANGELOG
  (below), `guide/tutorials/getting-started.md`, `guide/how-to/embed-panes.md`,
  and `guide/explanation/visualisers.md` now all say Plasma.

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
