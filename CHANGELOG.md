# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [1.0.0] - 2026-04-12

### Added

- Daemon/client architecture — `clitunesd` handles audio while `clitunes`
  renders visualisers. Music never stops when you close the TUI.
- Internet radio with curated station picker (radio-browser.info discovery).
- Local file playback (MP3, FLAC, OGG, WAV, AAC/M4A) with tag reading and
  recursive folder scan.
- Eight visualisers reactive to the audio spectrum: Auralis, Tideline,
  Cascade, Plasma, Ripples, Tunnel, Metaballs, Starfield.
- Standalone pane modes (`--pane visualiser`, `--pane now-playing`,
  `--pane mini-spectrum`) for embedding in tmux, WezTerm, or Ghostty layouts.
- Headless CLI verbs: `play`, `pause`, `next`, `prev`, `volume`, `viz`,
  `source`, `status --json`.
- Shared-memory SPMC ring for zero-copy PCM delivery to multiple clients.
- Unix socket control bus with line-delimited JSON protocol.
- Security hardening: umask-atomic socket bind (SEC-001), peercred UID gate,
  terminal escape sanitisation (D20), control bus DoS protection (SEC-007).
- State persistence — resumes your last station across sessions.
- Daemon auto-start on first `clitunes` invocation with 30-second idle
  shutdown after all clients disconnect.
- Startup measurement (`--measure-startup`) for benchmarking cold/warm boot.
- Diataxis-structured documentation in `guide/`.

[1.0.0]: https://github.com/vxcozy/clitunes/releases/tag/v1.0.0
