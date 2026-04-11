---
title: clitunes v1 — visualiser-first TUI music player
type: feat
status: active
date: 2026-04-11
deepened: 2026-04-11
origin: docs/brainstorms/clitunes-requirements.md
---

# clitunes v1 — visualiser-first TUI music player

## Overview

Build `clitunes`, a Rust TUI music player whose hero feature is a `wgpu`-rendered visualiser engine streamed to Kitty-graphics-protocol terminals (Ghostty, Kitty, WezTerm, Rio). v1 ships **radio + local files + three polished flagship visualisers (Auralis, Tideline, Cascade) + a daemon-client architecture with standalone `--pane` clients + a curated taste-neutral station picker**. Spotify integration (both `librespot` and system-capture) and the remaining visualiser candidates (Pulse, Aether, Polaris) are explicitly deferred to v1.1. The three v1 visualisers are deliberately chosen to occupy three genuinely different points in design space — frequency vs time, GPU vs CPU, instantaneous vs historical, maximalist vs minimal — so the "plethora of visualisers" identity claim is grounded in actual variety, not in counting variations of the same idea.

The two highest-risk technical bets — wgpu off-screen rendering streamed through Kitty graphics at 30+ fps, and a multi-client SPMC PCM ring backing the daemon-client split — are validated in pre-commitment spikes before any feature unit is built. If the spikes fail, the plan branches into pre-defined fallback paths rather than discovering the problem mid-implementation.

This plan resolves the 9 review findings (PF1–PF9) carried forward from the brainstorm AND a second wave of round-2 findings (F1, F2, F7, SEC-001 through SEC-013, AR-04/10/11/12, PF10–PF14, D-01..04, C1, C2) surfaced by a 7-persona document-review pass on 2026-04-11. Each is addressed in the appropriate section below; see `## Review Findings Resolution` for the full trace, including the round-2 subsection that captures the strategic decisions.

## Problem Frame

Existing terminal music players (cliamp, ncmpcpp, cmus, ncspot, spotify-tui) treat visuals as decoration, not the product. Modern terminals (Ghostty, Kitty, WezTerm) now support GPU-class graphics protocols that no existing TUI player exploits. clitunes is the first TUI player whose architecture is built backwards from the visualiser: the audio pipeline exists to feed the visuals, not the other way around.

The visualiser-first identity is the v1 differentiator. The brainstorm originally proposed shipping all five sources and five visualisers in v1, which the document review correctly flagged as a scope-vs-identity contradiction (PF1). v1 picks a side: **cut sources to ship a polished single visualiser**, then expand to Spotify and additional visualisers in v1.1 once the rendering pipeline has earned trust. See origin: `docs/brainstorms/clitunes-requirements.md`, decisions D1 and D8 (which are reconciled here as **D8'**).

A second framing decision the brainstorm made and v1 honors: clitunes is **infrastructure**, not a single app. The daemon (`clitunesd`) owns audio sources, decoders, the PCM ring buffer, and the state bus. Renderer clients (default tiled UI plus `--pane` standalone components) subscribe to the daemon over a Unix socket. This is the decision most expensive to retrofit, so it lands in slice 3 — early enough that no v1 code accumulates an in-process assumption, late enough that slice 1 (the visualiser spike) doesn't pay daemon-architecture cost before the visualiser path has been validated. Resolves PF2.

## Requirements Trace

### v1 — committed

- **R1.** Curated taste-neutral station picker as zero-config first-run (8–15 broadly varied stations spanning genres). Last selection persisted; subsequent launches auto-resume within ~3 seconds.
- **R2.** Radio source uses radio-browser.info community directory via DNS SRV mirror discovery. No API key.
- **R3.** Icecast/Shoutcast HTTP streams (MP3, AAC), `.pls`/`.m3u` files, HTTP redirects, ICY metadata parsing **with mandatory ANSI/control-byte sanitization** (security). Auto-reconnect on dropout.
- **R4.** Local files via CLI args and recursive folder scan. Symphonia decodes MP3, FLAC, OGG Vorbis, Opus, WAV, AAC. No persistent library.
- **R5.** Tag reads via `lofty` for now-playing display (ID3v2, Vorbis Comments, MP4 ilst, embedded album art bytes). No tag editing.
- **R8.** Active source switchable at runtime via `:source <name>` and via standalone-pane CLI.
- **R9 (revised round-2).** v1 ships **three** hand-tuned flagship visualisers (Auralis, Tideline, Cascade) implemented against a `Visualiser` trait that is rendering-path-agnostic — Auralis and Tideline use the GPU+Kitty path; Cascade uses the pure-CPU+ratatui path. Trait shape is forward-compatible with v1.1 plugin extraction. Resolves PF8 and AR-04/10.
- **R10 (revised round-2).** Three visualisers occupy three deliberately different points in design space:
  - **Auralis** — GPU spectrum analyser. wgpu fragment shader, frequency-mapped color palette, beat-sync subtle camera response, additive bloom. Maximalist. Headline.
  - **Tideline** — GPU waveform. Single morphing line driven by an instantaneous time-domain PCM buffer and a cheap fluid sim. Minimal monochrome. Contemplative counterpoint to Auralis.
  - **Cascade** — pure-CPU spectrogram waterfall. Last 30 seconds of FFT magnitudes scrolling up the pane, rendered as unicode block characters with a viridis colormap. **Zero wgpu, zero Kitty.** The visualiser that runs on terminals where the GPU path cannot, and the forcing function that proves the trait is rendering-path-agnostic.
  - The remaining brainstorm candidates (Pulse, Aether, Polaris) are deferred to v1.1.
- **R11.** Visualiser renders off-screen via `wgpu` to a texture, reads back via double-buffered staging, and streams to the terminal via the **Kitty graphics protocol** using `t=t` (temp file) transport with Unicode-placeholder rect reservation. Sixel is a fallback path stub for v1.1.
- **R12.** Active visualiser switchable at runtime via `:viz auralis|tideline|cascade`; persists across daemon restarts; configurable per-layout. The `--pane visualiser` standalone client honours `--viz <name>` so different panes in a layout can render different visualisers of the same audio.
- **R13.** Visualiser exposes named tunable parameters (`bloom`, `palette`, `fft_smoothing`) settable via `:set vis.auralis.<param> <value>` and persisted to `~/.config/clitunes/config.toml`.
- **R14 (revised).** Frame budget target: **30 fps minimum, 60 fps where the wgpu→Kitty pipeline sustains it on the user's hardware/terminal pair.** Audio↔visual sync is **bounded-drift, dropped-frame-reported** (cava lesson) — *not* sample-accurate. The earlier "sample-accurate" claim is replaced; the visualiser reports dropped sample windows on a counter the user can inspect. See origin: `docs/brainstorms/clitunes-requirements.md`, R14.
- **R15.** clitunes split into `clitunesd` (daemon) and `clitunes` (client CLI).
- **R16.** Running `clitunes` with no daemon active auto-spawns the daemon as a forked child (with pipe-readiness handshake) and attaches the default tiled-UI client. Daemon stays resident as long as ≥1 client is attached, plus configurable idle timeout.
- **R17.** Daemon exposes a Unix socket at `$XDG_RUNTIME_DIR/clitunes/ctl.sock`, mode **0600**, with **`SO_PEERCRED`/`LOCAL_PEERCRED` UID gating** (security). Protocol is **line-delimited JSON** with a banner line + `capabilities` command + MPD-style `idle`/`noidle` pub-sub. PCM ring buffer is exposed via a separate **shared-memory SPMC ring** (memmap2 + cache-line-padded cursors + write-sequence overrun detection), also UID-gated.
- **R18.** Any pane launchable as a standalone client process: `clitunes --pane visualiser`, `clitunes --pane mini-spectrum`, `clitunes --pane now-playing`, `clitunes --pane oscilloscope-stub`. Each renders a single component and subscribes to the running daemon.
- **R19.** `clitunes status --json` performs a one-shot daemon query and prints current state.
- **R20.** `clitunes <verb>` headless control: `play`, `pause`, `next`, `prev`, `source`, `vis`, `volume`. Exits cleanly without attaching a renderer.
- **R21.** Tiled ricer layout as default, configurable via TOML. Multiple named layouts (`default`, `compact`, `minimal`, `pure`, `fullscreen`) switchable at runtime via `:layout <name>`.
- **R22.** Layout config is declarative recursive splits with ratio weights; leaves are component panes from the standard set (`visualiser`, `now-playing`, `source-browser`, `queue`, `mini-spectrum`, `command-bar`).
- **R23.** Layout responds to terminal resize. Each component declares minimum-size requirements; panes that no longer fit are hidden gracefully (with fallback ladder).
- **R24.** Default layout is *structurally* opinionated and good (large visualiser top-left, source browser top-right, now-playing strip bottom). Per **D11**, the default's *aesthetic* dimensions (palette, default visualiser, font glyph style) are **never hardcoded** — they go through the same curated-picker mechanism as R1 if they reflect taste.
- **R25.** Modal overlays (`:`, `/`, `s`, `?`) are layout-independent and float above the active layout.
- **R26.** First-run experience: **zero friction, but never paternalistic.** Calibration tone visualiser running within ≤3 seconds; curated picker overlay; user choice persisted; subsequent launches skip the picker and resume the last station.
- **R27.** Single static binary per platform (macOS arm64/x86_64, Linux x86_64/arm64). Optional channels: Homebrew, `cargo install`, AUR. v1 build matrix omits Windows; deferred to v1.1.
- **R28.** Configuration at `~/.config/clitunes/config.toml` (XDG-respecting). Defaults baked in; config is purely override.

### v1.1 — explicitly deferred (cut by PF1 / D8' / R9 revisions)

- **R6, R7.** Spotify librespot path and system-audio-capture path (BlackHole/PulseAudio loopback/WASAPI). Deferred entire. The librespot spike happens at v1.1 planning time, *after* v1's rendering pipeline has earned trust and after we've measured how much polish budget Auralis actually consumed.
- **R10 (Tideline, Pulse, Aether, Polaris).** Four visualiser candidates from the original brainstorm slate. Each must demo well in v1.1 to earn its v1.1 slot — the plan does not pre-commit them.
- **Last.fm scrobbling.** Already v1.1 in the brainstorm.
- **Lyrics, cross-fade/gapless/ReplayGain, Windows build.** All v1.1+.

### Success criteria (revised; resolves PF5, PF6, PF9)

- **SC1. Day-one wow.** A new user installs clitunes, runs it with no arguments on Ghostty/Kitty/WezTerm, and within ≤3 seconds is watching Auralis on a calibration tone with the curated picker overlay. The visual quality is unmistakably beyond cliamp/cmus/ncmpcpp/ncspot. Screenshot-worthy on first launch with zero config.
- **SC2 (revised, resolves PF5). Primary radio + local surface for 30 days.** The clitunes engineer (user #1) uses clitunes as the primary listening surface for radio and local files for 30 consecutive days post-v1. Spotify use stays in the official client for discovery (Discover Weekly, Daylist, Connect, podcasts, Family account features). The dogfooding test is "clitunes is the better daily *visualiser-and-radio* experience," not "clitunes replaces Spotify." Spotify replacement is a v1.1 question.
- **SC3. Composability test.** A clitunes pane (`--pane mini-spectrum` or `--pane now-playing`) is embedded in a real tmux/wezterm/ghostty workspace alongside an editor and survives a normal coding session: terminal resize, daemon restart, source switching, suspend/resume, kill -9 of the daemon followed by client auto-respawn.
- **SC4 (revised, resolves PF6). Adoption signal, not vanity metric.** Replace "100 r/unixporn upvotes" with: at least **3 unrelated users** post screenshots of clitunes to the public web (any channel — r/unixporn, r/commandline, Mastodon, X, personal blogs, dotfile repos) **without being asked**, within 30 days of v1 release. This measures organic adoption, not a single subreddit's vote distribution.
- **SC5 (revised round-2, resolves PF9). Visualiser variety test, baseline form.** v1 ships three visualisers (Auralis, Tideline, Cascade) chosen on different design axes (GPU spectrum vs GPU waveform vs CPU waterfall; instantaneous vs historical; maximalist vs minimal). The variety test for v1 is the **side-by-side screenshot** committed in Unit 18: a single 10-second audio clip rendered by all three visualisers, demonstrating that they look genuinely different. The full n=5 user identification test still belongs to v1.1 when additional visualisers (Pulse/Aether/Polaris) join the slate.

## Scope Boundaries

**Non-goals for v1** (carrying from origin doc, plus PF1-driven additions):

- **Spotify integration** (both librespot and system-capture). v1.1.
- **Visualisers other than Auralis.** v1.1, each must earn its slot.
- **Tag editing, library indexing, SQLite, tag-based search across local files.** Use `beets` and feed clitunes the resulting paths.
- **Lyrics, cross-fade, gapless, ReplayGain.** v1.1.
- **Last.fm scrobbling.** v1.1 daemon plugin (~200 LOC once state bus exists).
- **MPD client mode.** clitunes is a player and daemon, not an MPD client.
- **Visualiser plugin DSL / preset authoring.** v2.
- **Pure unicode-block / ANSI-only rendering.** Not a v1 goal.
- **iOS/Android remote control.** Not v1.
- **DRM-protected content** (Apple Music, Tidal MQA). Out of scope entirely.
- **Windows v1 build.** v1 build matrix is macOS arm64/x86_64 + Linux x86_64/arm64. Windows + WASAPI loopback is v1.1.
- **Sixel beyond a stub fallback.** v1's tested transports are Kitty graphics + temp-file. Sixel polish is v1.1.

## Context & Research

### Relevant code and patterns

Greenfield project — no local code to mirror. The plan inherits patterns from the broader Rust + TUI ecosystem rather than from this repo. The patterns to follow are:

- **librespot's `Sink` trait** (`librespot-playback/src/audio_backend/mod.rs`) as the abstraction shape for any audio backend that needs to intercept PCM. Even though librespot itself is v1.1, the trait shape informs how the v1 `Source` trait should look so v1.1 can plug in cleanly.
- **MPD's text protocol with banner + `idle`/`noidle`** as the model for the daemon control protocol. Greppable text protocols age better than untagged binary; MPD's protocol is the existence proof.
- **mpv's line-delimited JSON over Unix socket** as the concrete wire format.
- **cava's input enumeration** as the model for "user picks source, daemon never auto-guesses" (matches D11).
- **gpg-agent / ssh-agent socket activation** via `listenfd` as the model for daemon auto-spawn lifecycle.

### Institutional learnings

No `docs/solutions/` folder exists in this greenfield project. Two relevant cross-cutting learnings carried from external research:

- **MPD's blocking FIFO output is the canonical anti-pattern** for "one stalled visualiser stalls the player." clitunes uses an SPMC shm ring with explicit overrun reporting instead.
- **`tokio::sync::broadcast::Sender` silently drops messages on lag.** Use per-client bounded `mpsc` channels with disconnect-on-overflow for the state bus instead.

### External references

The two research agents (`framework-docs-researcher` and `best-practices-researcher`) ran in Phase 1.3. Consolidated findings the plan rests on:

- **librespot 0.8.0** ships a `Sink` trait + `Player::new` with a `SinkBuilder` closure parameter that lets you intercept PCM samples without forking the crate. **Pin to `=0.8.0`**. (Relevant only to v1.1.)
- **wgpu 29.x** off-screen rendering is first-class. The latency-binding step is `device.poll → buffer.map_async → memcpy`. Realistic frame budget on M-series with **double-buffered staging + dedicated poll thread**: 2–8 ms p99 readback for a ~12 MB RGBA frame. Without ping-pong staging, you serialize submission and miss 60 Hz on M1.
- **Kitty graphics protocol**: streaming hot path is `t=t` (temp file in `/dev/shm` or `$TMPDIR`) + `f=32` (RGBA) + `i=<id>` overwrite + `q=2` (suppress acks) + Unicode placeholder (`U+10EEEE`) for ratatui Rect reservation. Do **not** use `f=100` (PNG) per frame. Do **not** use `a=d` between frames. Ghostty/WezTerm/Kitty/Rio coverage varies; WezTerm explicitly under-tested in the wild.
- **`ratatui-image` is the wrong tool for streaming.** Designed for static album-art widgets. v1 reserves a Rect via Unicode placeholders and writes Kitty escape sequences directly to stdout from the visualiser thread.
- **No public Rust prior art exists for `wgpu` → terminal-graphics-protocol streaming.** Treat as novel territory; spike is mandatory.
- **symphonia** does not parse ICY metadata. ~80 LOC custom parser, or use `stream-download` / `icy-metadata` crates as references.
- **radio-browser.info** requires DNS SRV mirror discovery (`_api._tcp.radio-browser.info`); hardcoding hostnames is explicitly discouraged. Free, no key, requires `User-Agent: clitunes/<version>` header. Use `url_resolved`, not `url`. Cache mirror list to disk.
- **PCM transport for multi-client tap**: SPMC shm ring (`rtrb` or `ringbuf::SharedRb` + `memmap2`), 64-byte cache-line padding on cursors, monotonic write-sequence per slot for overrun detection. Wait-free on the audio thread (no `Mutex`).
- **Daemon control protocol**: line-delimited JSON over `tokio::net::UnixListener`. Banner line + `capabilities` command + per-client bounded `mpsc` for pub-sub. Avoid `tokio-tungstenite` (WebSocket framing wasted on Unix socket); avoid `broadcast::Sender` (silent drops).
- **Daemon lifecycle**: `flock` on a lock file (NEVER a PID file), `listenfd` for socket-activation support, fork+pipe-readiness handshake from client on auto-spawn, idle-timeout shutdown.
- **Socket security**: `$XDG_RUNTIME_DIR/clitunes/ctl.sock`, mode 0600, `SO_PEERCRED` (Linux) / `LOCAL_PEERCRED` (macOS/BSD) UID check, hand-rolled `peercred.rs` with `#[cfg]` branches (no mature cross-platform Rust crate exists for this).
- **`cpal` SPMC tap pattern**: in the cpal output callback, fill the output buffer from the decoder ring AND simultaneously push the same samples into the visualiser's lock-free ring. Don't try to make cpal multi-consumer.

Sources cited inline at the bottom of this plan.

## Key Technical Decisions

Carrying brainstorm decisions D1–D11 (see origin: `docs/brainstorms/clitunes-requirements.md`) and adding planning decisions D8' and D12–D20.

- **D1.** Visualiser engine first. (Carried, now actually honored by D8'.)
- **D2.** Terminal aesthete audience. (Carried.)
- **D3.** Rust + ratatui + `wgpu` off-screen + Kitty graphics out. (Carried.)
- **D4.** Own the audio pipeline. (Carried.)
- **D5.** v1 = N hand-tuned flagships, plugin layer in v2. (Carried; N = 3, see D8'.)
- **D6.** Three sources, universal visualiser, radio as zero-config default. (Carried; "three sources" revised below.)
- **D8' (revises D8, resolves PF1 + round-2 PF10/PF14).** **v1 = radio + local + 3 visualisers (Auralis, Tideline, Cascade) + daemon. Spotify and the remaining visualiser candidates (Pulse, Aether, Polaris) are v1.1.** Rationale: D1 ("concentrate on visuals") and the original D8 ("v1 = 3 sources + 5 visualisers") were in scope-vs-identity contradiction. Round-1 review tilted the resolution to "visualiser-first, ship 1." Round-2 review pushed back: 1 visualiser actively undermines the visualiser-first identity claim ("the Ghostty of TUI music apps with a plethora of visualisers" rings hollow at v1 launch with one) AND prevents the `Visualiser` trait from being validated as rendering-path-agnostic. Final resolution: ship 3 visualisers picked to span vastly different axes — **Auralis** (GPU spectrum, instantaneous, maximalist bloom + beat-sync camera), **Tideline** (GPU waveform, instantaneous, fluid/minimal monochrome — pulled forward from v1.1), and **Cascade** (CPU spectrogram waterfall, last-30s historical time domain, terminal-native unicode block characters — *zero wgpu / zero Kitty graphics dependencies*). The three together are the forcing function for: (a) the `Visualiser` trait being rendering-path-agnostic — Cascade has no GPU code in its tree at all, which means the trait's surface cannot bake `wgpu::CommandEncoder` into its signature; (b) the `--pane` story end-to-end — slice 4 demos two clients on the same daemon rendering different visualisers of the same audio; (c) clitunes degrading gracefully on terminals without Kitty graphics protocol support — Cascade is the v1 fallback when capability probing rejects the GPU path. Polish budget that would have funded a 4th and 5th visualiser is concentrated on shader quality and parameter tuning of these three. Spotify gets its own v1.1 plan with the librespot spike and system-capture story handled fresh, *after* the visualiser pipeline has earned trust on real audio.
- **D9.** Tiled ricer with configurable TOML layouts. (Carried.)
- **D10.** Daemon + clients from day one. (Carried; "day one" interpretation clarified by D12.)
- **D11.** No taste imposition: curated pickers, never hardcoded defaults that touch taste. (Carried; D11 also applies to default visualiser selection — first-run shows Auralis but the picker also surfaces `:viz tideline` and `:viz cascade` as discoverable alternatives, and the choice is persisted in `~/.config/clitunes/state.toml` so subsequent runs honour the user's pick.)
- **D12 (resolves PF2).** **Daemon split lands in slice 3, not slice 1.** D10 says daemon-on-day-one is the most expensive thing to retrofit; the brainstorm's slice 1 said "trivial degenerate split." That's the trap D10 names. Resolution: slice 1 is the *visualiser pipeline spike* — calibration tone → cpal → rtrb in-process ring → realfft → wgpu → Kitty graphics — with **zero retrofit risk** because the in-process `rtrb` ring is throwaway scaffolding (replaced by the SPMC shm ring in slice 3). The calibration tone source is **not** thrown away — it survives slice 3 as a `Source` trait implementation and becomes a "no source selected" placeholder when the user has not yet picked anything. What is thrown away is the slice-1 *coupling* between the calibration tone and the in-process ring, not the calibration source itself. Slice 3 introduces the real `clitunesd` binary, the Unix socket, the SPMC shm ring, and the client refactor; nothing from slice 1 or slice 2 is in-process-coupled with the daemon. The retrofit cost is bounded to "swap the in-process ring for the shm ring + add a control socket connection," which is exactly what slice 3 does.
- **D13 (informed by research).** **Wire format: line-delimited JSON over Unix socket** for control + `idle`-style pub-sub. NOT msgpack. NOT a custom binary protocol. Rationale: MPD's text protocol aged best of any music daemon; greppable, version-skew-tolerant, easy to debug with `socat`. Parsing cost is irrelevant on a control bus that handles maybe 10 messages/sec. JSON-only also makes the future v1.1 mopidy-style HTTP/JSON-RPC bridge a 50-line addition rather than a serialization rewrite.
- **D14 (informed by research).** **PCM transport: SPMC shm ring with cache-line-padded cursors + write-sequence overrun detection.** Backed by `rtrb` or `ringbuf::SharedRb` + `memmap2`. Wait-free on the audio thread. The visualiser tap is a separate channel from the control socket. Rationale: control plane and bulk data plane are separate concerns with different latency / throughput / loss-tolerance shapes; mixing them is one of the regretted patterns from MPD/mpv prior art.
- **D15 (informed by research).** **Visualiser runs in the client process, not the daemon.** Daemon emits PCM via the shm ring; the client owns its own `wgpu` instance (or, for Cascade, its own ratatui buffer). Rationale: the `--pane` requirement means different clients want different visualisers of the same audio. Putting the visualiser in the daemon would re-invent RFB and break `--pane` isolation. Daemon has zero graphics dependencies — enforced via Cargo features now that the workspace is collapsed to 3 crates (see Workspace layout). `clitunes-engine` exposes a `visualiser` feature that gates the wgpu / Kitty / realfft modules. The daemon binary (`crates/clitunes/src/bin/clitunesd.rs`) declares `clitunes-engine = { default-features = false, features = ["audio", "control", "sources"] }`; the client binary (`crates/clitunes/src/main.rs`) adds `"visualiser", "tui", "layout"`. CI runs `cargo tree -e features --bin clitunesd` and fails the build if `wgpu` appears in the daemon's effective dependency tree. The architectural invariant is enforced mechanically via features rather than via a crate boundary.
- **D16 (informed by research).** **Kitty graphics transport: `t=t` (temp file) by default, `t=d` (base64 inline) fallback over SSH or when temp dir is unavailable.** Frame replacement via fixed `i=<id>`, never `a=d` per frame. `q=2` to suppress per-frame acks. Unicode placeholder (`U+10EEEE`) reserves the ratatui Rect; pixels are bound out-of-band. Rationale: this is the documented hot path; the alternatives waste bandwidth or trigger flicker.
- **D17 (resolves part of PF4).** **wgpu pipeline must use double-buffered staging with `map_async` callbacks on a dedicated poll thread.** Single-buffered + `pollster::block_on` on the render thread will fail to hit 30 fps on M1. The spike (Unit 1) measures whether the ping-pong path hits 60 fps; if it doesn't, the architecture survives at 30 fps with the same code path.
- **D18.** **`flock` lock file, never PID file. `listenfd` socket activation supported. Fork+pipe-readiness on auto-spawn.** Rationale: PID files have bitten every daemon that's used them, including MPD. The `flock`+`listenfd`+`fork` pattern is what `gpg-agent` and `ssh-agent` converged on after a decade of regretted alternatives.
- **D19.** **Socket at `$XDG_RUNTIME_DIR/clitunes/ctl.sock`, mode 0600, `SO_PEERCRED`/`LOCAL_PEERCRED` UID check.** Hand-rolled `peercred.rs` with `#[cfg]` branches; no mature cross-platform Rust crate wraps both. The runtime dir is per-user 0700 by default — the 0600 mode + UID check is belt-and-braces. Resolves the security-lens P0 around control-socket access.
- **D20.** **All untrusted strings sanitized of ANSI escapes and C0/C1 control bytes before any terminal write.** Station operators, radio-browser submitters, ICEcast `Icy-*` HTTP headers, and the tagging hands of every random MP3 ripper on earth all control strings that flow into the now-playing display. Unsanitized rendering of any of them is a terminal control-code injection vector. The sanitizer lives in `clitunes-core` (not `clitunes-sources`) as a small utility module — `untrusted_string::sanitize(&str) -> String` — and is applied at every ingestion boundary: radio-browser station/tag/country/codec fields (Unit 5), Icecast `Icy-Name`/`Icy-Genre`/`Icy-Br`/`Icy-Description` HTTP headers (Unit 6), in-band ICY `StreamTitle` chunks (Unit 6), `lofty`-extracted file tags (Unit 14), and `radio-browser` directory output before persistence (Unit 8). Resolves the security-lens P0 around ICY injection and the broader "any untrusted string written to the terminal" class of bugs.

## Open Questions

### Resolved during planning

- **PF1: scope vs identity contradiction.** Resolved as D8'. v1 ships visualiser-first; Spotify and additional visualisers are v1.1.
- **PF2: daemon-day-1 vs slice-1 in-process contradiction.** Resolved as D12. Slice 1 is the visualiser pipeline spike with throwaway in-process scaffolding; slice 3 introduces the real daemon. Retrofit risk is bounded to swapping the ring buffer source.
- **PF4: visual pipeline throughput.** Architectural risk resolved by D17 (double-buffered staging + dedicated poll thread is the only viable shape) and by Unit 1 (the spike measures actual numbers and chooses 30 vs 60 fps).
- **PF5: dogfooding test.** Resolved as SC2 — primary radio + local surface for 30 days, Spotify discovery stays in the official client.
- **PF6: vanity metric.** Resolved as SC4 — 3 unrelated users post screenshots without being asked.
- **PF7: slice 1 too large.** Resolved by D12 — slice 1 shrinks to *just* the visualiser pipeline spike (calibration tone → cpal → rtrb → realfft → wgpu → Kitty), with no station picker, no radio source, no persistence, no reconnect, no daemon. Each of those lands in a later slice. The radio source, ICY parsing, picker, and persistence are Units 5–8 in slice 2; the daemon binary lifecycle (Unit 9) lands in slice 3.
- **PF8: 5-visualiser slate not validated.** Resolved by R9 revision (round-1: Auralis only) and then revised again in round-2 review — v1 ships **Auralis + Tideline + Cascade** (3 visualisers chosen to span vastly different rendering paths, time domains, and aesthetics; see D8'). Pulse, Aether, and Polaris remain v1.1, each gated on a real-audio prototype before earning a v1.1 slot.
- **PF9: variety test unmeasurable.** Resolved by SC5 — variety test deferred to v1.1, v1.1's plan must define a real test.

### Deferred to implementation

- **Exact wgpu shader code for Auralis.** Comes out of slice 1 polish iteration. The plan commits to "GPU spectrum analyser with bloom and beat-sync camera response," not to specific WGSL.
- **Final palette / parameter values for Auralis** (`bloom`, `palette`, `fft_smoothing`). Tuned during slice 1 polish, exposed via R13.
- **Exact TOML schema for layout DSL.** First draft in Unit 14; final form depends on what `taffy` vs ratatui's built-in layout actually exposes.
- **Whether the SPMC ring uses `rtrb`, `ringbuf::SharedRb`, or a hand-rolled implementation.** Slice 3 picks based on which one's API actually allows the cross-process shm pattern; both are viable.
- **Final list of 8–15 curated stations.** Curation exercise during slice 2 polish. Must explicitly avoid loading the picker with a single dominant genre or the engineer's preferences.
- **Whether `--pane` clients negotiate a smaller PCM ring window or share the full window.** Unclear until after slice 3 measures real subscriber counts; bounded to a config flag if needed.
- **Whether v1 ships an `oscilloscope-stub` `--pane` component.** A non-flagship oscilloscope is useful as a `--pane` demonstration even though Tideline-the-flagship is v1.1; decided in slice 4 based on slice-1 spike findings (whether the wgpu pipeline can support a second simultaneous visualiser instance per terminal).

### Deferred to v1.1 planning (must address before any v1.1 unit is created)

- **librespot 0.8.0 spike** (PF3): does the `Player::new` custom `SinkBuilder` path actually work without forking `librespot-playback`? Does Spotify auth still succeed? End-to-end smoke test: login + 30s playback capturing PCM into a WAV.
- **System audio capture per OS**: BlackHole on macOS, PulseAudio loopback on Linux, WASAPI loopback on Windows. Per-OS setup documentation and clitunes-side enumeration code.
- **Pulse / Aether / Polaris**: each must demo against real audio in a v1.1 prototype before it earns its v1.1 slot. (Tideline pulled forward to v1 in round-2 review; see D8'.)
- **Last.fm scrobbling**: ~200 LOC daemon plugin subscribing to `track-changed` events on the state bus.
- **Lyrics**: `lrclib.net` source.
- **Cross-fade, gapless, ReplayGain.**
- **Windows build + WASAPI loopback.**

## High-Level Technical Design

> *This illustrates the intended architecture and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

### Process and data topology

```
                  ┌──────────────────────────────────────────────────┐
                  │                  RENDERER CLIENTS                 │
                  │      (each is its own clitunes process)           │
                  │                                                   │
                  │  ┌─────────────┐  ┌──────────┐  ┌──────────────┐ │
                  │  │ default     │  │ --pane   │  │ --pane       │ │
                  │  │ tiled UI    │  │ visualiser│ │ now-playing  │ │
                  │  │ (ratatui +  │  │ (wgpu +  │  │ (text-only)  │ │
                  │  │  wgpu vis)  │  │  Kitty)  │  │              │ │
                  │  └──────┬──────┘  └────┬─────┘  └──────┬───────┘ │
                  │         │              │                │        │
                  └─────────┼──────────────┼────────────────┼────────┘
                            │              │                │
                            │  control     │  control       │  control
                            │  (JSON over  │  (JSON over    │  (JSON over
                            │   Unix sock) │   Unix sock)   │   Unix sock)
                            │              │                │
                            │  PCM tap     │  PCM tap       │  (no PCM)
                            │  (shm ring)  │  (shm ring)    │
                            ▼              ▼                ▼
                  ┌──────────────────────────────────────────────────┐
                  │                  clitunesd  (daemon)              │
                  │                                                   │
                  │   ┌─────────────────────────────────────────┐    │
                  │   │  control-bus:  Unix socket 0600 +       │    │
                  │   │  SO_PEERCRED gate, line-delimited JSON, │    │
                  │   │  banner + capabilities + idle/noidle    │    │
                  │   └─────────────────────────────────────────┘    │
                  │                                                   │
                  │   ┌─────────────────────────────────────────┐    │
                  │   │  pcm-bus:  SPMC shm ring (memmap2 +     │    │
                  │   │  cache-line-padded cursors + write-seq  │    │
                  │   │  overrun detection)                      │    │
                  │   └─────────────────────────────────────────┘    │
                  │             ▲                                     │
                  │             │ (single producer, audio thread)     │
                  │   ┌─────────┴────────────┐                        │
                  │   │  cpal output stream  │                        │
                  │   │  + decoder ring      │                        │
                  │   └─────────┬────────────┘                        │
                  │             │                                     │
                  │   ┌─────────┴────────────┐                        │
                  │   │  Source trait        │                        │
                  │   │   ├── radio          │ (radio-browser SRV +   │
                  │   │   ├── local files    │  ICY parser + symphonia│
                  │   │   └── (calibration   │  HTTP MediaSource)     │
                  │   │        tone)         │                        │
                  │   └──────────────────────┘                        │
                  └──────────────────────────────────────────────────┘
```

### Visualiser pipeline (in the client process)

```
┌────────────────────────┐
│ shm PCM ring (read-    │ ← daemon writes here from audio thread
│ side, lock-free)        │
└──────────┬─────────────┘
           │ pop window of 2048 stereo samples
           ▼
┌────────────────────────┐
│ realfft (real-valued    │ ← reusable FFT planner, ~30 µs on M1
│ forward FFT, 1025 bins) │
└──────────┬─────────────┘
           │ magnitudes + smoothing
           ▼
┌────────────────────────┐
│ Auralis WGSL fragment   │ ← wgpu render pass to off-screen
│ shader (bloom, beat-    │   texture, ~1 ms render
│ sync camera)            │
└──────────┬─────────────┘
           │ render to texture (RGBA, ~2400×1280)
           ▼
┌────────────────────────┐
│ ping-pong staging       │ ← double-buffered: encode frame N
│ buffers (MAP_READ +     │   while frame N-1 is being read
│ COPY_DST)               │
└──────────┬─────────────┘
           │ map_async + memcpy on dedicated poll thread
           ▼
┌────────────────────────┐
│ Kitty graphics writer   │ ← write RGBA to a temp file whose name
│ (t=t temp file, i=<id>, │   contains the literal substring
│ q=2, RGBA f=32)         │   "tty-graphics-protocol" (kitty refuses
│                         │   any other name); then emit
│                         │   \033_Ga=T,i=N,...\033\\ to stdout
└──────────┬─────────────┘
           │ Kitty image bound to ratatui Rect via U+10EEEE
           ▼
       terminal pixels
```

### Slice 1 degenerate scaffolding (slice 1 only — replaced in slice 3)

```
┌─────────────────────┐
│ calibration tone    │ ← in-process function generator
│ (sine sweep + noise)│   (440 Hz + pink noise burst)
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐    ┌────────────────────┐
│ in-process rtrb     │───►│ visualiser pipeline│
│ ring (no shm yet)   │    │ (as above)         │
└─────────────────────┘    └────────────────────┘
```

Slice 1 has **zero** Unix socket, **zero** daemon binary, **zero** real audio source. It exists to validate the wgpu→Kitty pipeline and the realfft tap shape on actual hardware. Everything from slice 1 is reusable in slice 3 *except* the calibration tone (replaced by real sources) and the in-process rtrb ring (replaced by the SPMC shm ring). The realfft module, the Auralis shader, the Kitty writer, the staging buffer logic, and the `Visualiser` trait all survive verbatim.

### Workspace layout

```
clitunes/
├── Cargo.toml                       # workspace
├── crates/
│   ├── clitunes-core/               # pure types (Track, PlayerState, Source enum,
│   │                                #   untrusted_string sanitizer); no I/O, no async,
│   │                                #   no heavy deps; fast to compile
│   ├── clitunes-engine/             # all functional code, organized into modules:
│   │   └── src/
│   │       ├── pcm/                 #   SPMC shm ring + cpal tap (gated: feature "audio")
│   │       ├── proto/               #   control-bus JSON messages + serde + capabilities
│   │       │                        #     (gated: feature "control")
│   │       ├── sources/             #   Source trait + calibration / radio / local impls
│   │       │                        #     (gated: feature "sources")
│   │       ├── visualiser/          #   Visualiser trait + Auralis + Tideline + Cascade
│   │       │                        #     + realfft + wgpu pipeline (gated: "visualiser")
│   │       ├── kitty/               #   Kitty graphics writer + Unicode placeholder
│   │       │                        #     (gated: "visualiser")
│   │       ├── layout/              #   TOML layout DSL + ratatui integration
│   │       │                        #     (gated: "layout")
│   │       ├── tui/                 #   default tiled UI composition
│   │       │                        #     (gated: "tui")
│   │       ├── daemon/              #   lifecycle, control_bus, idle_pubsub, peercred,
│   │       │                        #     idle_timer, lockfile, socket_activation
│   │       └── cli/                 #   auto_spawn, control_client, pane_mode,
│   │                                #     headless_verbs, status_command
│   └── clitunes/                    # binary crate with two `[[bin]]` targets:
│       ├── src/main.rs              #   `clitunes` (client; default UI + --pane router
│       │                            #     + status + headless verbs)
│       └── src/bin/clitunesd.rs     #   `clitunesd` (daemon; orchestrates sources,
│                                    #     owns the control bus and shm ring producer)
├── docs/
│   ├── brainstorms/                 # (already exists)
│   ├── plans/                       # (this file lives here)
│   └── backlog.md                   # follow-up work parked from review (e.g. crate split
│                                    #   when boundaries are real)
└── .github/workflows/               # CI: cargo test, clippy, cross-build matrix, release
```

**Why 3 crates and not 10**: round-2 review pointed out that a 10-crate workspace for a solo-dev greenfield codebase is premature modularization — every crate boundary forces re-exports, slows incremental builds, and forces type plumbing to be designed before the types are well-understood. ratatui itself is one crate; tokio's split serves *users* of tokio, not tokio's own dev velocity. We start with 3 crates (`clitunes-core` for shared types with no heavy deps, `clitunes-engine` for everything functional, `clitunes` for the binaries) and let real boundaries emerge from real friction. If the engine crate later grows boundaries that obviously deserve to be their own crates, splitting is mechanical because all the modules are already organized as `src/<module>/` subdirs. **The future split is tracked in `docs/backlog.md`** so it does not get lost.

**How D15 (daemon has no graphics deps) is still enforced under 3 crates**: the engine crate exposes Cargo features. The daemon binary in `crates/clitunes/Cargo.toml` declares `clitunes-engine = { default-features = false, features = ["audio", "control", "sources"] }`. The client binary declares the superset `["audio", "control", "sources", "visualiser", "tui", "layout"]`. The `visualiser` feature gates the modules that pull in `wgpu`, `realfft`, and the Kitty writer; the `tui` feature gates the modules that pull in `ratatui` rendering. CI runs `cargo tree -e features --bin clitunesd` and `grep`s for `wgpu`; the build fails if it appears in the daemon's effective dependency tree. The architectural invariant from D15 is mechanically enforced; only the *mechanism* changed from "separate crates" to "feature flags within one crate."

### Pane content sketches

The plan names the v1 components — `visualiser`, `now-playing`, `source-browser`, `queue`, `mini-spectrum`, `command-bar` — but a strict reading of the round-1 plan didn't say what content lives inside each one. The implementer would have had to invent these on the fly during Units 14–16, with high churn risk. The sketches below are *content specs*, not visual mocks: they describe what data the pane displays, how it sorts, what it looks like at a small terminal, and what it does on edge cases. Final visual polish is iteration during slice 4 polish; these sketches set the boundary.

**`visualiser`** — Whichever visualiser the user has currently selected (default: Auralis). Fills its rect entirely. Responds to `:vis <name>` and the `v` hotkey to cycle through {Auralis, Tideline, Cascade}. Renders via the GPU path (Auralis, Tideline) or the TUI buffer path (Cascade) per the `Visualiser` trait's surface kind. Edge case: rect smaller than 20×8 — render the layout's "too small" placeholder, do not attempt the visualiser.

**`now-playing`** — A 4-row text strip:
- Row 1: track title (sanitized via `untrusted_string`) — left-aligned, truncated with ellipsis if it overflows.
- Row 2: artist · album · year — middle-dot-separated, truncated similarly.
- Row 3: source-specific metadata. Radio: `📻 station-name (codec @ kbps)`. Local: `📁 folder-name (track N of M)`. Calibration: `🎚 calibration tone`. (Emojis are unicode-only, single-codepoint, terminal-safe.)
- Row 4: progress bar (16 cells wide) + position/duration timestamps. For radio (no duration), the bar is replaced with a small reconnect indicator if `StreamState::Reconnecting`.

When height is exactly 1 row, collapse to: `<title> — <artist> [▮▮▮▮▯▯ 1:23/3:45]`. When height is 2 rows, drop the source-specific row.

**`source-browser`** — A vertical list, source-typed:
- In radio mode: shows the curated 12-station list (Unit 8) with current station highlighted; arrow keys move; enter switches station; `/` opens a filter input.
- In local mode: shows the current queue (track titles, current track highlighted, ▶ marker).
- Header line shows current source and a `[s] switch` hint.
- Edge case: width < 30 cols — drop the artist/tag column, show titles only. Width < 20 — collapse to track-numbers only with the current row highlighted.

**`queue`** — Available only in local mode. Two columns: `#` (track number) and `title — artist`. Current track highlighted. Up/down move; enter jumps; `d` removes from queue; `c` clears queue. In radio mode, this pane component is replaced with a clear "queue is local-files only" placeholder so the layout never shows a blank pane.

**`mini-spectrum`** — A 1-row-tall, fullwidth horizontal bar of unicode block characters (▁▂▃▄▅▆▇█), one block per FFT bin (log-scaled). Renders via the CPU path identical to Cascade's per-frame bar (factored into a shared helper in `clitunes-engine/src/visualiser/cascade/render.rs`). No Kitty graphics; no wgpu. Specifically intended for status-line embedding via `clitunes --pane mini-spectrum`. Updates at the same rate as the visualiser (target 30 fps) but is rate-limited to 10 fps when stdout is a pipe to avoid wasting bandwidth on a `tail -f`-style consumer. Edge case: rect height > 1 — vertically center the bar; do not stretch it.

**`command-bar`** — A 1-row pane at the bottom that toggles between two states: idle (shows `:` or empty) and active (`:set vis.auralis.bloom_radius 12`-style command entry, à la vim). Up/down navigate command history; enter executes; ESC cancels. Always 1 row tall. When the layout doesn't include `command-bar`, the `:` keybind temporarily steals the bottom row of whichever pane is active, returning it on enter/ESC.

## Implementation Units

Plan groups 20 units into 5 phases (round-2 review added Units 17 Tideline and 18 Cascade; the previous Units 17 First-run polish and 18 Distribution renumbered to 19 and 20). Phase 0 is a hard gate — no Phase 1+ unit begins until Phase 0 succeeds or its fallback is committed to.

> **Note on file paths.** Round-2 review collapsed the workspace from 10 crates to 3 (`clitunes-core` / `clitunes-engine` / `clitunes`). File paths in the unit lists below reflect the new layout: `crates/clitunes-engine/src/<module>/...` for functional code, `crates/clitunes/src/main.rs` for the client binary, `crates/clitunes/src/bin/clitunesd.rs` for the daemon binary. See [`docs/backlog.md`](../backlog.md) for the deferred "split `clitunes-engine` into focused crates when real boundaries emerge" item, including the explicit triggers that should promote it to a real plan.

### Phase 0 — Pre-commitment spikes (HARD GATE)

- [ ] **Unit 1: wgpu → Kitty graphics throughput spike**

**Goal:** Empirically measure whether the `wgpu` off-screen → ping-pong staging → Kitty graphics protocol pipeline sustains 30 fps (target: 60 fps) on the user's actual hardware and target terminals. Decide 30-vs-60 fps target. Surface any architecture-killing finding (e.g., M1 Metal readback consistently >15 ms) before any feature work.

**Requirements:** R11, R14, D17, D16. Resolves PF4.

**Dependencies:** None. This is the gate.

**Files:**
- Create: `crates/clitunes-engine/examples/visualiser_spike_wgpu_kitty.rs`
- Create: `crates/clitunes-engine/src/visualiser/wgpu_pipeline.rs`
- Create: `crates/clitunes-engine/src/kitty/lib.rs`
- Create: `crates/clitunes-engine/src/kitty/temp_file_transport.rs`
- Create: `crates/clitunes-engine/src/kitty/unicode_placeholder.rs`
- Create: `docs/spikes/2026-04-11-wgpu-kitty-throughput-spike.md` (results write-up)
- Test: `crates/clitunes-engine/tests/visualiser_wgpu_readback_smoketest.rs`

**Approach:**
- Spike binary clears a `2048×1024` (or terminal-derived) RGBA texture each frame to a moving gradient + a ramping sine modulation (no real audio yet — just deterministic visual content).
- Uses `wgpu` 29.x off-screen `RENDER_ATTACHMENT | COPY_SRC` texture, copies to a `MAP_READ | COPY_DST` staging buffer.
- **Two staging buffers, ping-pong.** Frame N+1 renders + submits while frame N's `map_async` callback is in flight on a dedicated poll thread (`device.poll(PollType::Poll)` in a loop, not `Wait` on the render thread).
- Kitty writer: `t=t` (temp file in `/dev/shm` if it exists, else `$TMPDIR`), `f=32` RGBA, `i=1` (single fixed image id), `q=2` (suppress acks), Unicode placeholder for placement. **Critical:** the temp file path must contain the literal substring `tty-graphics-protocol` or kitty will reject the chunk; create via `mkstemp` with template `tty-graphics-protocol-clitunes-XXXXXX.rgba`, opened with `O_NOFOLLOW | O_EXCL`, mode `0600`, in the chosen tmpdir. Reuse the same file across frames (truncate + rewrite) to avoid `mkstemp` cost in the hot path.
- Measure: p50, p95, p99 frame time across 5 minutes of streaming, separately for `submit→map`, `map→memcpy`, `memcpy→Kitty escape write`, `Kitty escape→terminal-rendered`. Repeat on Kitty, Ghostty, WezTerm. Repeat on M1 (or M-series user-#1 hardware) **and** a Linux laptop with Mesa.
- Output: a markdown table in `docs/spikes/2026-04-11-wgpu-kitty-throughput-spike.md` showing per-terminal, per-platform, per-stage timings, plus a go/no-go decision and chosen target frame rate.
- **Pre-committed go/no-go threshold (mechanical, not vibe-checked).** A platform/terminal combination passes the **60 fps** bar if `p99 frame time ≤ 16 ms` AND `p95 ≤ 14 ms`. A combination passes the **30 fps** bar if `p99 ≤ 33 ms` AND `p95 ≤ 25 ms`. Decision rules, applied in order:
  1. If at least 2 of {M-series macOS + Ghostty, M-series macOS + Kitty, Linux + Kitty, Linux + WezTerm} hit the 60 fps bar, **v1 commits to 60 fps**.
  2. Else, if at least 3 of those 4 hit the 30 fps bar, **v1 commits to 30 fps**.
  3. Else, the spike **fails** and the plan branches to one of the Risks & Dependencies fallbacks (texture shrink → JPEG encoding → CPU rendering at lower visual ceiling). The branch decision is captured in the spike doc with the measurements that drove it.

  These thresholds are pre-committed *before* the spike runs so the decision is mechanical. Changing them after seeing the numbers requires writing a new decision (D17') with explicit rationale; "we're close enough" is not a valid post-hoc justification.

**Execution note:** This is an exploration spike. Write characterization measurements first; the spike's only deliverable is the measured numbers and the resulting decision. No production code lands from this unit *except* the modules listed in `Files` (`wgpu_pipeline.rs`, `temp_file_transport.rs`, `unicode_placeholder.rs`), which are reused verbatim in Unit 4.

**Patterns to follow:**
- `tpix` (Kitty discussion #5660) for the temp-file transport pattern that's known to hit 60 fps with video frames.
- `wgpu`'s own headless rendering examples for the off-screen pipeline shape.
- WezTerm's `termwiz` crate for Kitty escape encoding reference.

**Test scenarios:**
- Happy path: spike runs for 5 minutes on M1+Ghostty without dropped frames at 30 fps. Recorded in spike doc.
- Edge case: spike runs at 60 fps target — pipeline either holds 60 fps p99 ≤16 ms, or degrades gracefully and the run-time decision drops to 30 fps. Either outcome is acceptable; the *decision* is what the spike produces.
- Edge case: terminal is resized mid-spike. Pipeline either re-derives the texture size and continues, or exits with a clear error message — both acceptable outcomes for a spike, neither acceptable for the production unit.
- Failure path: spike runs on a terminal that doesn't support Kitty graphics (e.g. xterm). Spike refuses to start and prints a clear "this terminal does not support Kitty graphics protocol; clitunes is not for it" message. This becomes the production error in Unit 4.
- Integration: the `Visualiser` trait shape (input: `&[f32]` PCM window + frame index; output: render-to-texture call) is exercised by feeding the spike's gradient generator through the trait. Validates that the trait survives contact with real wgpu code before Unit 4 commits to it.

**Verification:**
- The spike write-up `docs/spikes/2026-04-11-wgpu-kitty-throughput-spike.md` exists, contains per-terminal/per-platform timing tables, and ends with an explicit "v1 frame budget target: 30 fps" or "v1 frame budget target: 60 fps" decision plus rationale.
- If the spike fails (no terminal+platform combination hits 30 fps p99 under the real workload), the plan branches to one of the pre-defined fallbacks in `## Risks & Dependencies` (smaller texture, lower fps, JPEG instead of RGBA, drop the wgpu path entirely in favor of pure-CPU rendering at lower visual ceiling). The branch decision is captured in the spike doc.

### Phase 1 — Slice 1: First Pixels (the visualiser pipeline, standalone)

- [ ] **Unit 2: Cargo workspace skeleton + crate scaffolding**

**Goal:** Establish the workspace layout described in `## High-Level Technical Design`, set up CI, license, README placeholder, MSRV pin, and CI cross-build matrix for macOS arm64/x86_64 + Linux x86_64/arm64 (Windows excluded per scope boundary).

**Requirements:** R27 (build matrix).

**Dependencies:** Unit 1 must have produced a go decision.

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/clitunes-core/Cargo.toml`, `crates/clitunes-core/src/lib.rs`
- Create: `crates/clitunes-engine/Cargo.toml`, `crates/clitunes-engine/src/lib.rs`
- Create: `crates/clitunes-engine/src/pcm/mod.rs` — feature `audio`
- Create: `crates/clitunes-engine/src/proto/mod.rs` — feature `control`
- Create: `crates/clitunes-engine/src/sources/mod.rs` — feature `sources`
- Create: `crates/clitunes-engine/src/visualiser/mod.rs` — feature `visualiser`
- Create: `crates/clitunes-engine/src/kitty/mod.rs` — feature `visualiser` (graduated from Unit 1)
- Create: `crates/clitunes-engine/src/layout/mod.rs` — feature `layout`
- Create: `crates/clitunes-engine/src/tui/mod.rs` — feature `tui`
- Create: `crates/clitunes/Cargo.toml` — binary crate with `[[bin]] name = "clitunes"` and `[[bin]] name = "clitunesd"`
- Create: `crates/clitunes/src/main.rs` — client binary, depends on `clitunes-engine` with all features
- Create: `crates/clitunes/src/bin/clitunesd.rs` — daemon binary, depends on `clitunes-engine` with `default-features = false, features = ["audio", "control", "sources"]`
- Create: `.github/workflows/ci.yml`
- Create: `deny.toml`
- Create: `LICENSE` (MIT or Apache-2.0; pick one in Unit 20)
- Create: `README.md` (placeholder, replaced in Unit 20)
- Create: `rust-toolchain.toml` (pin MSRV)
- Create: `.gitignore`, `.editorconfig`

**Approach:**
- Workspace root `Cargo.toml` with `[workspace.package]` for shared metadata and `[workspace.dependencies]` for pinned versions of `wgpu = "29"`, `ratatui`, `crossterm`, `tokio`, `serde`, `serde_json`, `symphonia` (with feature flags `mp3 flac vorbis isomp4 aac wav opus`), `lofty`, `realfft`, `cpal`, `rtrb` (or `ringbuf`), `memmap2`, `nix`, `interprocess`, `listenfd`, `fs4`, `dirs`, `directories-next`, `tracing`, `tracing-subscriber`, `anyhow`, `thiserror`.
- CI: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `cargo deny check advisories bans sources licenses` (with a checked-in `deny.toml`) — this catches RUSTSEC advisories (Symphonia, librespot, reqwest, tokio, wgpu, lofty are all sizable transitive surfaces and warrant continuous advisory checking), license incompatibilities, and yanked deps. Build matrix `(macos-14, ubuntu-22.04) × (stable)`, cross-builds for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`.
- **Architectural enforcement test:** a CI step that runs `cargo tree -e features --bin clitunesd` and greps for `wgpu`/`ratatui`/`crossterm` — fails the build if any of them appear in the daemon binary's transitive deps. This enforces D15 mechanically under the 3-crate layout: the daemon binary declares `clitunes-engine = { default-features = false, features = ["audio", "control", "sources"] }` and the visualiser/tui/layout modules are gated behind features the daemon never enables, so they never link in even though they share the engine crate.

**Patterns to follow:**
- `ratatui` itself, `tokio`, and `wgpu` are all multi-crate Rust workspaces with similar structure. Crib `[workspace.dependencies]` shape from any one of them.

**Test scenarios:**
- Happy path: `cargo build --workspace` succeeds on macOS arm64 and Linux x86_64.
- Happy path: `cargo test --workspace` succeeds (with no actual test bodies yet — the scaffolding compiles).
- Edge case (architectural): `cargo tree -e features --bin clitunesd | grep -qE 'wgpu|ratatui|crossterm'` returns non-zero. If it ever returns zero, CI fails with "daemon must not depend on visualiser/tui/layout features — those belong in the client binary (D15). Check that any new module added to clitunes-engine is gated behind the right Cargo feature."
- Edge case: cross-build for `aarch64-unknown-linux-gnu` succeeds (catches glibc version pinning issues).

**Verification:**
- All 3 crates compile cleanly in CI.
- The daemon-must-not-depend-on-visualiser-features check is wired and passing.
- The cross-build matrix produces artifacts for all 4 v1 platforms.
- `cargo deny check` passes with the checked-in `deny.toml`.

- [ ] **Unit 3: Calibration tone source + cpal output + in-process PCM ring + realfft tap**

**Goal:** Stand up the audio half of the slice 1 pipeline. A calibration tone (440 Hz sine + slowly modulated pink noise burst) generates samples in the daemon process; cpal outputs them to the system audio device; an in-process `rtrb` ring tees the same samples to a consumer thread that runs `realfft` and exposes the magnitude bins. **No real audio sources yet** — the calibration tone exists so the visualiser pipeline can be validated without depending on the radio source landing first.

**Requirements:** R14 (revised), partial R4 (in-process scaffolding for slice 1, replaced in slice 3).

**Dependencies:** Unit 2.

**Files:**
- Create: `crates/clitunes-engine/src/sources/calibration.rs`
- Create: `crates/clitunes-engine/src/sources/source_trait.rs`
- Create: `crates/clitunes-engine/src/pcm/in_process_ring.rs`
- Create: `crates/clitunes-engine/src/pcm/cpal_tap.rs`
- Create: `crates/clitunes-engine/src/visualiser/realfft_tap.rs`
- Test: `crates/clitunes-engine/tests/sources_calibration_smoketest.rs`
- Test: `crates/clitunes-engine/tests/pcm_in_process_ring_tests.rs`
- Test: `crates/clitunes-engine/tests/visualiser_realfft_tap_tests.rs`

**Approach:**
- `Source` trait shape (forward-compatible with v1.1 librespot path): `fn next_packet(&mut self, buf: &mut [f32]) -> SourceResult<usize>` returning interleaved stereo f32 frames at 48 kHz.
- Calibration source: 440 Hz sine on the left channel, pink noise on the right, with a 0.5 Hz amplitude modulation envelope so the visualiser sees real movement. Deterministic and seedable for tests.
- cpal output stream: pinned to `BufferSize::Fixed(1024)`, falling back to `Default` if the device rejects 1024. In the cpal callback, the pattern from the research brief: fill the cpal buffer from the source AND push the same samples into the in-process `rtrb` ring as the SPMC tap. **No `Mutex` on the audio thread.**
- realfft tap thread: pops a 2048-sample window from the ring (or skips and reports a drop if behind), runs forward real FFT once per window, smooths the magnitudes, exposes them via a `&[f32]` snapshot the visualiser pulls per frame.
- Drop counter is exposed via the daemon state (later — for slice 1, just `eprintln!`'d when tracing is enabled).

**Patterns to follow:**
- `cpal`'s `output.rs` example for the basic stream setup.
- The research brief's "fill output buffer + push to ring in the same callback" pattern.
- `realfft`'s docs.rs example for one-shot FFT planner reuse.

**Test scenarios:**
- Happy path: calibration source produces 5 seconds of PCM, cpal renders to system audio, realfft consumer pops 240 windows (5s × 48kHz / 1024 ≈ 240) and reports zero drops on a quiet system.
- Edge case: source runs faster than the consumer for 1 second (consumer thread sleeps). Ring overflows. Consumer reports a drop count > 0 on resume. Verifies the drop-reporting path actually fires.
- Edge case: cpal device unplugged mid-stream (simulated by manually closing the stream). Source loop catches the `StreamError::DeviceNotAvailable` and rebuilds the stream cleanly. Manual integration test on real hardware.
- Error path: requested cpal buffer size 1024 is rejected by the device. Code falls back to `Default` and logs the actual chosen size.
- Integration: realfft output for a pure 440 Hz sine input shows a clean peak in the bin nearest 440 Hz × (FFT_SIZE / sample_rate). Quantitative check.

**Verification:**
- `cargo test -p clitunes-engine --features "audio sources visualiser"` passes the `sources_calibration_*`, `pcm_in_process_ring_*`, and `visualiser_realfft_tap_*` test files.
- Manual run: `cargo run -p clitunes-engine --features "audio sources visualiser" --example sources_calibration_audible` plays the tone audibly through the default device for 10 seconds, ringing the speaker on the user's actual hardware.
- realfft tap reports zero drops on a quiet system over a 60-second run.

- [ ] **Unit 4: Auralis v0 — wgpu pipeline + Kitty rect integration + frame-drop counter**

**Goal:** Wire the validated wgpu→Kitty pipeline from Unit 1 to the realfft tap from Unit 3, producing the first version of Auralis: a GPU spectrum analyser running fullscreen in the terminal, fed by the calibration tone, hitting the frame budget target chosen in Unit 1. This is the slice 1 deliverable.

**Requirements:** R9 (revised), R10 (Auralis), R11, R12, R13, R14 (revised). Resolves PF7.

**Dependencies:** Unit 1 (spike), Unit 3 (audio half).

**Files:**
- Create: `crates/clitunes-engine/src/visualiser/visualiser_trait.rs`
- Create: `crates/clitunes-engine/src/visualiser/auralis/mod.rs`
- Create: `crates/clitunes-engine/src/visualiser/auralis/shader.wgsl`
- Create: `crates/clitunes-engine/src/visualiser/auralis/bloom.wgsl`
- Create: `crates/clitunes-engine/src/visualiser/staging_pingpong.rs`
- Create: `crates/clitunes-engine/src/visualiser/poll_thread.rs`
- Modify: `crates/clitunes-engine/src/visualiser/wgpu_pipeline.rs` (graduated from spike)
- Modify: `crates/clitunes-engine/src/kitty/lib.rs` (graduated from spike)
- Modify: `crates/clitunes/src/main.rs` — minimal `clitunes` binary that runs Auralis on the calibration tone fullscreen, accepts `q` to quit
- Test: `crates/clitunes-engine/tests/visualiser_auralis_smoketest.rs`
- Test: `crates/clitunes-engine/tests/visualiser_staging_pingpong_tests.rs`

**Approach:**
- `Visualiser` trait: takes a `&VisualiserContext` (PCM window + magnitude bins + frame index + tunable params snapshot) and a `VisualiserSurface` enum that abstracts the rendering target. **The trait must be rendering-path-agnostic** — its surface CANNOT bake `wgpu::CommandEncoder` into its signature because Cascade (Unit 18) is a pure-CPU visualiser with zero wgpu deps and Tideline (Unit 17) needs a different wgpu pipeline shape than Auralis. The trait declares `fn capabilities(&self) -> SurfaceKind` (returning `Gpu` or `Tui`) and dispatches via either `fn render_gpu(&mut self, ctx, encoder, view)` or `fn render_tui(&mut self, ctx, buffer, area)`; only one is called per frame based on the visualiser's declared kind. Default impls of both return `unimplemented!()` so a visualiser only implements the one it needs. This shape is also forward-compatible with v2 plugin extraction — no `&mut self` state that couldn't be serialized to a hot-reload boundary, and the GPU vs CPU dispatch is the kind of capability boundary plugins will need anyway.
- Auralis: vertical bars indexed by log-spaced frequency bins, color from a 1D gradient palette texture indexed by frequency, additive bloom pass, beat-sync camera response (a small zoom/shake driven by an onset detector on the bass band). Tunable params: `bloom_radius`, `bloom_strength`, `palette_name`, `fft_smoothing`, `bar_count`, `beat_response_strength`.
- Render loop on the client thread: pull magnitude snapshot from the realfft tap, push into the wgpu pipeline, submit, hand the staging buffer to the poll thread.
- Poll thread: `device.poll(PollType::Poll)` in a tight loop, processes `map_async` callbacks, memcpys ready frames into the Kitty writer, signals the render thread when a staging buffer is free.
- Kitty writer: writes RGBA bytes to a temp file whose path **must** contain the literal substring `tty-graphics-protocol` (kitty rejects any other name when `t=t`). Created via `mkstemp` with template `tty-graphics-protocol-clitunes-XXXXXX.rgba` in `/dev/shm` (Linux) or `$TMPDIR` (macOS), opened `O_NOFOLLOW | O_EXCL` with mode `0600`. Same file is reused frame-to-frame (truncate + rewrite) to keep the hot path mkstemp-free after first allocation. Then emits the `\033_Ga=T,i=1,...\033\\` escape with `t=t,f=32,q=2`; ratatui leaves the rect blank with Unicode placeholders.
- Frame drop counter: tracks `(frames_submitted, frames_displayed, audio_drops, render_drops)` and exposes via a single-line debug overlay (`F: 60/60  AD: 0  RD: 0`) toggleable with `d`.

**Execution note:** Test-first for the `Visualiser` trait shape and the staging ping-pong logic — both are easy to write characterization tests for. Auralis-the-shader is iterative visual polish, so its "tests" are screenshot inspection during slice 1 polish, not unit tests.

**Patterns to follow:**
- Spike code from Unit 1 for the wgpu off-screen + Kitty writer hot path.
- WGSL bloom shader patterns from the rust-graphics community (separable Gaussian, two-pass).
- ratatui's example apps for the `crossterm` event loop + `q` to quit shape.

**Test scenarios:**
- Happy path: `cargo run --bin clitunes` on Ghostty starts the calibration tone, displays Auralis fullscreen at the target fps, and exits cleanly on `q`.
- Happy path: bloom radius is changed via the (slice-1-stub) `:set vis.auralis.bloom_radius 12` command; the next frame reflects the new value.
- Edge case: terminal is resized while running. Visualiser re-derives the texture dimensions, allocates new staging buffers, drops zero audio frames during the resize, and the next frame fills the new rect cleanly.
- Edge case: the realfft tap reports 5 dropped windows in a 1-second period (because the consumer thread was preempted). Visualiser's debug overlay reflects `AD: 5`.
- Error path: `clitunes` (the client binary) is run on a terminal that doesn't support Kitty graphics (e.g. ssh into a basic xterm) **with Auralis or Tideline selected**. Binary detects this via terminal-capability probe and either (a) exits with `clitunes requires a Kitty-graphics-protocol terminal for Auralis/Tideline. Detected: xterm. Try '--viz cascade' for a CPU-rendered visualiser that works on any 256-color terminal.` or (b) auto-falls-back to Cascade if `--viz auto` (the default) was passed.
- Error path: wgpu adapter request fails (no GPU available in CI runner). Binary exits with a clear "no compatible GPU found" message. Wired so CI tests can detect-and-skip this case.
- Integration: pipe the calibration source through realfft and verify the brightest column on the Auralis output corresponds to the 440 Hz bin. Manual screenshot inspection.

**Verification:**
- `cargo run --bin clitunes` on Ghostty: Auralis fills the terminal with a moving spectrum bar display within ≤3 seconds of launch.
- The `d` debug overlay shows the chosen target fps from Unit 1 (30 or 60), with audio-drop and render-drop counters at 0 on a quiet idle system.
- The frame budget target from Unit 1 is met under the real workload (the spike was synthetic; Unit 4 closes the loop).
- A screenshot of Auralis on the calibration tone is committed to `docs/spikes/2026-04-11-wgpu-kitty-throughput-spike.md` as visual evidence.

### Phase 2 — Slice 2: Real audio source (radio)

- [ ] **Unit 5: radio-browser.info SRV mirror discovery + cached station list**

**Goal:** Implement the radio-browser.info client correctly: DNS SRV mirror discovery, polite User-Agent, cached station list, search by genre/country/popularity, station object with `url_resolved`. No playback yet — this unit just produces a working `StationDirectory` API the picker and the source can consume.

**Requirements:** R2.

**Dependencies:** Unit 2.

**Files:**
- Create: `crates/clitunes-engine/src/sources/radio/mod.rs`
- Create: `crates/clitunes-engine/src/sources/radio/directory.rs`
- Create: `crates/clitunes-engine/src/sources/radio/srv_discovery.rs`
- Create: `crates/clitunes-engine/src/sources/radio/cache.rs`
- Test: `crates/clitunes-engine/tests/sources_radio_directory_tests.rs`

**Approach:**
- DNS SRV resolution against `_api._tcp.radio-browser.info` via `trust-dns-resolver` or `hickory-resolver`. Pick first healthy server (HTTP HEAD `/json/stats` returns 200); fall back through the round-robin list. Cache the chosen mirror to disk (`~/.cache/clitunes/radio-browser-mirror.txt`) for ≤24 h.
- HTTP client: `reqwest` with `User-Agent: clitunes/<version>`. (`<version>` from `env!("CARGO_PKG_VERSION")`.)
- Station list cache: full directory dump cached at `~/.cache/clitunes/radio-browser-stations.json` with a 24 h TTL. Cache miss falls back to a live `/json/stations/topvote?limit=500` query.
- `StationDirectory` API: `search(filter: StationFilter) -> Vec<Station>`, `get(station_uuid: &str) -> Option<Station>`. Filter supports name, tag, country, language, codec, min_bitrate.
- Station struct includes `url_resolved` (use this for playback, not `url`), name, country, tags, codec, bitrate, votes, click_count. **All free-text fields (`name`, `country`, `tags`, `codec`, `language`) are passed through `clitunes_core::untrusted_string::sanitize` before the `Station` struct is constructed** — radio-browser submissions are anonymous and arbitrary, so any name or tag could contain ANSI escapes or terminal control bytes. The sanitizer lives in `clitunes-core` so all sources share it (D20).

**Patterns to follow:**
- The `radiobrowser` crate's mirror discovery code (cribbed, not depended on, since we want explicit cache control).

**Test scenarios:**
- Happy path: SRV lookup returns ≥1 mirror, HEAD probe returns 200, station search by tag `lo-fi` returns >0 stations.
- Happy path: cache hit on subsequent run within 24 h returns the same station list without an HTTP request (verifiable via mock HTTP recorder).
- Edge case: SRV lookup returns zero mirrors. Falls back to the cached mirror list shipped in the binary (last-known-good DNS result). If that also fails, returns a clear "radio directory unavailable" error.
- Edge case: HTTP request to the chosen mirror returns 503. Code rotates to the next mirror, re-probes, retries.
- Edge case: cache file is corrupt JSON. Code logs a warning, deletes the cache, fetches fresh.
- Error path: network is offline entirely. `StationDirectory::search` returns the cached station list (if any) plus a `network_offline: true` flag the picker UI surfaces ("offline — showing cached stations").
- Integration: search by tag `classical` and verify ≥1 result has `tags` containing `classical`.

**Verification:**
- Tests pass.
- Manual run: `cargo run --example list_top_stations` prints 20 top-vote stations to stdout with their tags, country, and bitrate.

- [ ] **Unit 6: HTTP stream client + ICY metadata sanitized parser + reconnect**

**Goal:** Connect to an Icecast/Shoutcast HTTP URL, parse the `Icy-MetaInt` header, strip in-band metadata chunks from the body byte stream, **sanitize the metadata strings of ANSI escapes and C0/C1 control bytes**, and expose a `Read`-able byte stream of the audio body that symphonia can consume. Auto-reconnect on dropout.

**Requirements:** R3 (with the security addition from D20).

**Dependencies:** Unit 5 (for the URL inputs).

**Files:**
- Create: `crates/clitunes-engine/src/sources/radio/icy_stream.rs`
- Create: `crates/clitunes-engine/src/sources/radio/icy_sanitizer.rs`
- Create: `crates/clitunes-engine/src/sources/radio/reconnect.rs`
- Test: `crates/clitunes-engine/tests/sources_icy_stream_tests.rs`
- Test: `crates/clitunes-engine/tests/sources_icy_sanitizer_tests.rs`

**Approach:**
- `IcyStream` is a `Read`-implementing wrapper around a `reqwest::blocking::Response`. Construct: send `Icy-MetaData: 1` header, read the response, extract `Icy-MetaInt: <N>` and `Icy-Name`/`Icy-Genre`/`Icy-Br`/`Icy-Description` headers from the response. **All HTTP `Icy-*` header values are passed through `clitunes_core::untrusted_string::sanitize` before being stored or surfaced to the UI** — Icecast operators control these and they reach the now-playing display.
- `Read::read` impl: tracks bytes-since-last-metadata, when it reaches `metaint`, reads the 1-byte length prefix, reads `length × 16` bytes of metadata, parses `StreamTitle='...'` etc., and emits the parsed-and-sanitized metadata to a `Sender<TrackChange>` channel. Returns audio bytes (excluding the metadata block) to the caller.
- **Sanitizer**: lives in `clitunes-core` as `untrusted_string::sanitize(&str) -> String` (NOT `clitunes-sources` — radio is one of several callers; lofty tags in Unit 14 and radio-browser fields in Unit 5 also use it). Strips bytes in `0x00..=0x1F` and `0x7F..=0x9F` (C0 and C1 control bytes) and any ANSI escape sequence (`\x1b[...m` and friends — match-and-strip via a small DFA, not a regex). Replaces with U+FFFD or omits entirely. Tested against fuzz-style inputs including known terminal-injection payloads. Unit 6 only owns the in-band ICY parsing path and the call-site that pipes parsed `StreamTitle` chunks through the sanitizer; the sanitizer module itself moves to `clitunes-core`.
- `ReconnectingStream`: wraps `IcyStream` with exponential-backoff reconnect (1s, 2s, 4s, 8s, 16s, cap at 30s) on `io::Error` or HTTP 5xx. Reports state changes via the same `Sender<TrackChange>` channel as a `StreamState::Reconnecting { attempt: u32 }` event.

**Execution note:** Sanitizer is test-first. Write the malicious-input test cases (ESC sequences, BEL, NUL, embedded `\x1b]` OSC injections) before writing the sanitizer.

**Patterns to follow:**
- `stream-download` and `icy-metadata` crates as references for the binary wire format. Do not depend on them; the parser is small enough to own.

**Test scenarios:**
- Happy path: connect to a known stable test station (e.g., a SomaFM Groove Salad URL), read 30 seconds of audio bytes, observe ≥1 `TrackChange` event with a sanitized `StreamTitle`.
- Happy path: sanitizer passes a normal track title `"Artist - Song Name"` through unchanged.
- Edge case: stream sends `metaint = 16384` and the metadata block is 32 bytes. Audio frame boundary alignment is preserved across the metadata block.
- Edge case: stream sends an empty metadata block (`length = 0`). Stream continues without emitting a change event.
- Failure path: stream drops mid-frame (server closes connection). `ReconnectingStream` retries, the next track-change event includes a `StreamState::Reconnected` marker so the UI can flash a brief reconnect indicator.
- Failure path: HTTP 404 on the URL. No infinite retry — fails after 3 attempts with a clear error.
- **Security/sanitizer happy path:** input `"Artist - Song"` → output `"Artist - Song"`.
- **Security/sanitizer attack 1:** input `"Track\x1b]0;OWNED\x07"` (OSC window-title injection) → output strips the OSC sequence entirely.
- **Security/sanitizer attack 2:** input `"\x1b[2J\x1b[H"` (clear screen + cursor home) → output is empty or replacement chars; no escape bytes survive.
- **Security/sanitizer attack 3:** input `"\x07\x07\x07Bell-spam\x07"` → output is `"Bell-spam"`.
- **Security/sanitizer attack 4:** input contains `\x9b` (CSI as a single C1 byte) → stripped.
- **Security/sanitizer fuzz:** 1000 randomly-generated inputs containing arbitrary bytes 0x00–0xFF; output is verified to contain no bytes in the C0 or C1 ranges and no ESC sequences.

**Verification:**
- All tests pass, including the full security suite.
- Manual run: `cargo run --example play_station <station-uuid>` (using Unit 5's directory) connects, plays into a temporary file, and prints sanitized track-change events to stderr.

- [ ] **Unit 7: Symphonia HTTP MediaSource → realfft → Auralis (replaces calibration tone)**

**Goal:** Plumb the radio source through the existing audio pipeline so Auralis is now reactive to real radio audio instead of the calibration tone. Calibration tone becomes a fallback when no source is selected (the slice-1 spike scaffolding lives on as a "no source" placeholder, not as the default).

**Requirements:** R3, R4 (formats), R14 (revised).

**Dependencies:** Units 3, 4, 6.

**Files:**
- Create: `crates/clitunes-engine/src/sources/symphonia_decode.rs`
- Modify: `crates/clitunes-engine/src/sources/source_trait.rs` (add `RadioSource` impl)
- Modify: `crates/clitunes/src/main.rs` (`--source radio --station <uuid>` arg)
- Test: `crates/clitunes-engine/tests/sources_symphonia_decode_tests.rs`

**Approach:**
- Wrap the `IcyStream` (which implements `Read`) in a `MediaSource` that returns `is_seekable() = false`.
- Hand the `MediaSource` to `symphonia`'s default format reader. Decode in a loop, push decoded `f32` samples into the in-process PCM ring (the same ring the calibration source feeds in slice 1; replaced by the SPMC shm ring in slice 3).
- Convert sample format / channel layout / sample rate to the canonical 48 kHz stereo f32 the visualiser expects. Use `symphonia`'s built-in resampler or `rubato` if needed.
- On `EndOfStream` or decode error, the source signals `SourceState::Ended` and the daemon (later) decides whether to reconnect or move to the next track.

**Patterns to follow:**
- Symphonia's `examples/basic-interleaved.rs` for the decode loop shape.

**Test scenarios:**
- Happy path: feed a known-good MP3 radio stream URL, decode 30 seconds, push to the PCM ring, verify ≥1.4M samples landed (30s × 48kHz × stereo).
- Edge case: stream sends a 96 kHz / 5.1 surround source. Decoder downmixes to stereo and resamples to 48 kHz cleanly. No crash, no clipping.
- Edge case: stream is FLAC over Icecast (rare but legal). Decoder switches probe and continues.
- Edge case: stream is AAC. AAC codec coverage in symphonia is historically less battle-tested — this scenario may need an explicit "AAC support is best-effort in v1" caveat in the README.
- Failure path: stream sends a corrupt frame. Decoder logs and skips, audio continues with a brief glitch — does not crash the source.
- Integration: full pipeline run — pick a station from Unit 5's directory, connect via Unit 6, decode via Unit 7, FFT via Unit 3, render via Unit 4. Auralis is reactive to real music for 60 seconds. Manual screenshot.

**Verification:**
- Tests pass.
- Manual run: `cargo run --bin clitunes -- --source radio --station <uuid>` plays the station audibly with Auralis reactive to it for ≥5 minutes without crashing.

- [ ] **Unit 8: Curated 8–15 station picker UI + first-run persistence**

**Goal:** Build the curated taste-neutral station picker overlay (R1, R26, D11) and the persistence layer for first-run last-station resume. On first launch, the picker overlay floats above Auralis (still running on the calibration tone) and the user picks a station with arrow keys + enter. On subsequent launches, the saved station auto-resumes within ~3 seconds and the picker is not shown.

**Requirements:** R1, R26, R28, D11. Picker is reachable any time via `:source radio` or hotkey `s`.

**Dependencies:** Units 4, 5, 6, 7.

**Files:**
- Create: `crates/clitunes-engine/src/tui/picker/mod.rs`
- Create: `crates/clitunes-engine/src/tui/picker/curated_seed.rs`
- Create: `crates/clitunes-engine/src/tui/persistence.rs`
- Create: `docs/curation/2026-04-11-curated-stations.md` (the actual list, with rationale per slot)
- Modify: `crates/clitunes/src/main.rs`
- Test: `crates/clitunes-engine/tests/tui_picker_tests.rs`
- Test: `crates/clitunes-engine/tests/tui_persistence_tests.rs`

**Approach:**
- Curated seed list: 12 stations spanning at minimum the following slots, picked from the radio-browser directory based on stability (high vote count + low click_trend variance) and license-friendliness:
  - 1× ambient/lo-fi (e.g., SomaFM Groove Salad, Drone Zone, or equivalent — pick during slice 2 polish; picker code is data-driven from the curated list, not hardcoded)
  - 1× classical (e.g., a public broadcaster classical stream)
  - 1× jazz
  - 1× electronic / dance
  - 1× indie/alt rock
  - 1× world music
  - 1× news/talk (English-language public broadcaster)
  - 1× classic rock or classic hits
  - 1× soul/funk/r&b
  - 1× hip-hop / instrumental hip-hop
  - 1× experimental / drone
  - 1× one explicit "discovery" wildcard rotated each release
- The curated list is shipped in the binary as a `static` JSON blob in `crates/clitunes-engine/src/tui/picker/curated_seed.rs`, but is **also** override-able via `~/.config/clitunes/curated_stations.toml` for users who want a different seed.
- Picker UI: ratatui modal overlay over the Auralis pane. Header line: "First time? Pick a starting point — you can change anytime."  Body: list of 12 stations, one per line, with name + tag + country. Arrow keys move; enter selects; `q` exits without picking.
- Persistence: TOML file at `~/.config/clitunes/state.toml` (NOT `config.toml` — state and config are separate). Stores `last_station_uuid`, `last_source`, `last_visualiser`, `last_layout`. Atomic write via `tempfile::NamedTempFile::new_in(parent_dir)` (same dir, so `persist` is a `rename` not a cross-device copy) followed by `persist`. Both the temp file and the persisted file are explicitly chmod'd to `0600` (state may include private station UUIDs and listening history); the parent dir is created with mode `0700` if absent. Same treatment for `config.toml` when it's introduced.
- First-run detection: presence of `state.toml` is the gate. If absent, show picker. If present and `last_station_uuid` is set, auto-resume.

**Execution note:** The curated list itself is a curation exercise that **must explicitly avoid loading the picker with stations that reflect the engineer's personal taste** (per D11 and feedback memory `feedback_no_taste_imposition.md`). Curation gets a documented rationale per slot in `docs/curation/2026-04-11-curated-stations.md`. The list is NOT "stations the engineer likes."

**Patterns to follow:**
- ratatui's modal overlay examples for the picker layout.
- `tempfile::persist_noclobber` for atomic state writes.

**Test scenarios:**
- Happy path: first run, no state.toml exists, picker is shown, user selects station #5, station begins playing within ~2 seconds.
- Happy path: second run, state.toml exists with `last_station_uuid` set, picker is NOT shown, station auto-resumes within ~3 seconds.
- Happy path: user opens picker mid-session via `s`, selects a different station, new station replaces the current one and the new uuid is persisted.
- Edge case: state.toml exists but `last_station_uuid` references a station that no longer exists in the radio-browser directory. Code falls back to showing the picker with a banner: "Your last station is no longer available — pick another."
- Edge case: state.toml is corrupt TOML. Code logs a warning, deletes it, shows the picker.
- Edge case: curated list override file `~/.config/clitunes/curated_stations.toml` exists with 0 stations. Code falls back to the embedded curated list and logs a warning.
- Edge case: terminal is too narrow for the picker (e.g., 40 columns). Picker degrades gracefully: shows station name only, hides tag/country columns.
- Integration: full first-run flow — fresh install, no state, launch clitunes, picker appears within 3 seconds, pick station 1, audio begins, exit, relaunch, station 1 auto-resumes.

**Verification:**
- Tests pass.
- Manual run on a fresh state dir: time-from-launch-to-first-pixel is ≤3 seconds (SC1).
- The 12-station curated list is documented in `docs/curation/2026-04-11-curated-stations.md` with rationale per slot, and an explicit "engineer taste audit" note confirming no slot reflects the engineer's personal preferences.

### Phase 3 — Slice 3: Daemon split

This phase introduces the real `clitunesd` binary and refactors slice-1 and slice-2 code from in-process scaffolding to client-server. Per D12, this is the slice where D10 (daemon-day-1) actually lands. Nothing from slices 1-2 is in-process-coupled; the retrofit is bounded to "swap the in-process ring for the SPMC shm ring + add a control socket connection."

- [ ] **Unit 9: clitunesd binary lifecycle (`flock` lock + `listenfd` activation + idle shutdown)**

**Goal:** Stand up the `clitunesd` binary with the lifecycle pattern from research: `flock` lock file (NEVER PID file), `listenfd` socket activation support, fork+pipe-readiness handshake on auto-spawn, idle-shutdown timer.

**Requirements:** R15, R16, D18.

**Dependencies:** Phase 2 complete.

**Files:**
- Modify: `crates/clitunes/src/bin/clitunesd.rs`
- Create: `crates/clitunes-engine/src/daemon/lifecycle.rs`
- Create: `crates/clitunes-engine/src/daemon/lockfile.rs`
- Create: `crates/clitunes-engine/src/daemon/socket_activation.rs`
- Create: `crates/clitunes-engine/src/daemon/idle_timer.rs`
- Create: `crates/clitunes-engine/src/cli/auto_spawn.rs`
- Test: `crates/clitunes-engine/tests/daemon_lifecycle_tests.rs`

**Approach:**
- Lock file at `$XDG_RUNTIME_DIR/clitunes/clitunesd.lock`. On startup, `flock` exclusive non-blocking; if it fails, exit immediately ("daemon already running").
- Socket activation: check `LISTEN_FDS` env (set by systemd), use `listenfd` to inherit fd 3 if present. Otherwise create the socket ourselves at `$XDG_RUNTIME_DIR/clitunes/ctl.sock`. **Mode 0600 is enforced by setting `umask(0o177)` in the daemon process *before* `bind`**, not by `chmod` after — the chmod-after-bind window is a real race that the umask approach eliminates entirely. Restore the previous umask after `bind` returns. The same umask wraps the shm file creation in Unit 11.
- Auto-spawn from the client side (`crates/clitunes/src/auto_spawn.rs`): on `connect()` to the control socket returning ENOENT or ECONNREFUSED, fork the daemon binary (resolving the `clitunesd` sibling binary path via `std::env::current_exe`'s parent dir) with a pipe inherited as fd 3, daemon writes "ready\n" on fd 3 once it's listening, client reads "ready\n", retries connect. If the daemon writes anything other than "ready" or the pipe closes, the client surfaces a clear error.
- Idle shutdown: the daemon tracks attached client count. When the last client disconnects, start a timer (default: 5 minutes, configurable in `config.toml` as `daemon.idle_timeout_secs`). When the timer fires AND no clients are attached, exit cleanly (release `flock`, unlink the socket).
- Sigterm/sigint handler: drain in-flight client connections, flush state, exit.

**Patterns to follow:**
- `gpg-agent` and `ssh-agent` for the auto-spawn + pipe-readiness pattern.
- `listenfd`'s docs.rs example for inheriting an activated socket.
- `fs4`'s `FileExt::lock_exclusive` for `flock`.

**Test scenarios:**
- Happy path: `clitunesd` starts cleanly, lock acquired, socket created, idle timer running.
- Happy path: client connects, daemon registers the connection, idle timer paused.
- Happy path: client disconnects, idle timer resumes, daemon exits 5 minutes later.
- Happy path: second `clitunesd` invocation while one is running fails with "daemon already running" and exits non-zero.
- Auto-spawn: client `clitunes` is run when no daemon exists. Client forks the daemon, waits for ready, connects, plays audio. Verifiable end-to-end.
- Edge case: lock file exists from a previous crashed daemon but no process holds it. `flock` succeeds (Linux releases stale locks on process death), daemon starts cleanly.
- Edge case: the `LISTEN_FDS` env is set (systemd activation). Daemon inherits fd 3 instead of creating a socket. Verify with a fake systemd-style harness in tests.
- Failure path: daemon binary not in PATH. Client auto-spawn fails with "could not find clitunesd binary; ensure clitunes is correctly installed."
- Failure path: forked daemon panics before writing "ready". Client times out after 5 seconds, kills the orphan, surfaces "daemon failed to start; check ~/.cache/clitunes/clitunesd.log."

**Verification:**
- Tests pass.
- Manual: `clitunes` (cold) auto-spawns daemon, plays a station, exits, daemon idles for 5 minutes, exits.
- Manual: second `clitunes` instance (warm) attaches to the existing daemon without spawning a second one.

- [ ] **Unit 10: Control protocol — line-delimited JSON over Unix socket + banner + capabilities + idle/noidle**

**Goal:** Implement the control-bus protocol per D13. Banner line on connect, `capabilities` command, command/response request types, and an MPD-style `idle`/`noidle` pub-sub mechanism for state events. Per-client bounded `mpsc` for fanout (NOT `broadcast::Sender`).

**Requirements:** R17, D13.

**Dependencies:** Unit 9.

**Files:**
- Create: `crates/clitunes-engine/src/proto/messages.rs`
- Create: `crates/clitunes-engine/src/proto/banner.rs`
- Create: `crates/clitunes-engine/src/proto/capabilities.rs`
- Create: `crates/clitunes-engine/src/daemon/control_bus.rs`
- Create: `crates/clitunes-engine/src/daemon/idle_pubsub.rs`
- Create: `crates/clitunes-engine/src/cli/control_client.rs`
- Test: `crates/clitunes-engine/tests/proto_message_roundtrip_tests.rs`
- Test: `crates/clitunes-engine/tests/daemon_control_bus_tests.rs`
- Test: `crates/clitunes-engine/tests/daemon_idle_pubsub_tests.rs`

**Approach:**
- Wire format: one JSON object per line, terminated by `\n`. `tokio_util::codec::LinesCodec::new_with_max_length(65_536)` on both sides — the default `LinesCodec::new()` has `max_length = usize::MAX`, which lets a malicious or runaway client OOM the daemon by sending a never-terminated line. 64 KiB is far above any legitimate command (`status` responses are the largest, and stay well under 8 KiB). UTF-8.
- Banner: daemon writes `{"clitunesd": <version>, "protocol": <integer>}\n` immediately on accept. Clients verify protocol version is in their supported range; mismatch → disconnect with a clear error.
- Commands: `{"id": <int>, "cmd": "<verb>", "args": {...}}`. Responses: `{"id": <int>, "ok": true, "data": {...}}` or `{"id": <int>, "ok": false, "error": "<msg>"}`. `id` is client-chosen and echoed.
- Capabilities: `cmd: "capabilities"` returns the list of supported verbs and event types. Clients use this for forward-compat feature detection — never parse the version banner with regex.
- Verbs: `play`, `pause`, `stop`, `next`, `prev`, `source`, `vis`, `set` (param tuning), `volume`, `status`, `idle`, `noidle`, `subscribe_pcm` (in Unit 11).
- Pub-sub via `idle`: client sends `{"id": N, "cmd": "idle"}`. Daemon does NOT respond immediately; instead, the next time any subsystem (player, source, visualiser) emits a state-change event, the daemon sends `{"id": N, "ok": true, "data": {"event": "track_changed", "track": {...}}}` and the client must re-`idle` for the next event. This is exactly MPD's pattern. `noidle` cancels the pending response.
- Per-client fanout: each accepted connection gets a bounded `tokio::sync::mpsc::Sender<Event>` (capacity 256). State changes are fanned to all subscribers via per-client `try_send`. **If `try_send` returns `Full`, the daemon disconnects that client** (it's lagging) and forces it to reconnect+resync. This is the disconnect-on-overflow pattern from research that prevents silent message loss.
- Initial-state replay: when a client subscribes to a topic, the daemon sends a snapshot event first containing current state, then deltas. Avoids the "client connects mid-track and shows nothing" race.

**Patterns to follow:**
- MPD's `idle`/`noidle` for the pub-sub semantics.
- mpv's line-delimited JSON for the wire format.
- The Tokio docs' "use mpsc not broadcast for slow consumers" guidance.

**Test scenarios:**
- Happy path: client connects, reads banner, sends `capabilities`, receives the list.
- Happy path: client sends `play`, daemon responds `{ok:true}`, player starts.
- Happy path: client sends `idle`, daemon does not respond. After 2 seconds, daemon-side track-changed event fires; daemon sends the event as the response to the pending `idle`. Client re-issues `idle`.
- Edge case: client sends 1000 commands without reading responses. Per-client mpsc fills, daemon disconnects the client. Reconnect succeeds with a fresh banner.
- Edge case: client connects mid-track. Daemon sends snapshot event with current track, position, source. Verifies replay-on-subscribe.
- Edge case: protocol version mismatch (client supports protocol 1, daemon advertises protocol 2). Client disconnects with "incompatible daemon version 2 (this clitunes supports v1); upgrade clitunes."
- Edge case: malformed JSON line from client. Daemon responds with `{ok:false, error: "malformed json", id: null}` and continues serving the connection.
- Failure path: client crashes mid-command (TCP RST). Daemon's read loop returns EOF, client is removed cleanly, idle timer resumes if no other clients.
- Integration: two clients attached simultaneously, both `idle`-ing. A track change fires once; both clients receive the event independently.

**Verification:**
- Tests pass.
- Manual: `socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/clitunes/ctl.sock` and type JSON commands by hand. Banner appears, `capabilities` works, `play`/`pause` works. The protocol is greppable.

- [ ] **Unit 11: SPMC PCM ring in shared memory + cross-process visualiser tap (SPIKE-then-implement, with fallback path)**

**Goal:** Move the PCM ring buffer from in-process to shared memory so client processes (default UI client + `--pane` clients) can tap the same audio stream. Per D14: cache-line-padded cursors, monotonic write-sequence per slot, overrun reporting, wait-free audio thread.

**Round-2 review correction (load-bearing):** Round-1 framing of "use `rtrb` or `ringbuf::SharedRb`" was wrong. `rtrb` is **SPSC-only** — `Producer<T>` and `Consumer<T>` cannot be cloned. `ringbuf::SharedRb` is in-process-only; sharing it across processes requires hand-rolling the cross-process layout. Either way, the SPMC-shm path is a *real concurrency design project* with cross-process memory ordering concerns on aarch64 and a non-trivial verification story. Loom ("soft target" in round-1) is not adequate as the only safety net. Unit 11 is therefore split into a **Phase A spike with a hard time budget** and a **Phase B implementation that branches on Phase A's result.**

**Phase A — SPMC shm ring spike (3-day hard time budget).** Build the seqlock-style SPMC reader against an `mmap`'d file. Use the loom crate as a *required* gate (not soft target) on the producer/consumer interleavings. Write the spike binary so a producer thread and N consumer threads run against the same ring on a single host first (validates the ordering before adding the cross-process complication). Then add a separate-process consumer test (parent forks; child opens the same shm path read-only and reads). Spike write-up lives at `docs/spikes/2026-04-XX-spmc-shm-ring-spike.md`.

**Phase A go/no-go (mechanical, pre-committed):**
1. Loom test passes for producer + 2 consumer interleavings on `--release` profile.
2. Cross-process test (separate process consumer) reads 10,000 slots from a deterministic producer with zero torn reads (verified by content checksum, not just write-sequence) and the consumer's reported overrun count matches `producer_writes - ring_capacity` exactly when the producer is intentionally fast.
3. Spike fits in 3 calendar days.

If all 3 hold, Phase B implements the spike's design as the production code.

**Phase B fallback (taken if Phase A fails any gate)**: switch to a **per-consumer bounded mpsc fan-out**. The daemon's audio thread writes into a daemon-side `tokio::sync::broadcast` (small capacity, drop-oldest) consumed by a dedicated daemon-side fan-out thread that maintains one bounded `tokio::sync::mpsc` *per attached client*; that fan-out thread runs the same disconnect-on-overflow rule from Unit 10's control bus. Cross-process delivery uses a Unix datagram socket per client (one per `subscribe_pcm` call), with each datagram carrying one ring slot's worth of samples plus a sequence number. CPU cost is higher (one memcpy per consumer per slot vs zero) but the correctness story collapses to "tokio's mpsc is correct" plus "one syscall per slot per consumer," both of which are well-understood. Throughput is fine for v1's expected ≤4 simultaneous clients.

The two paths are isomorphic at the `Visualiser`-trait level: both expose `fn next_window(&mut self) -> Option<&[f32]>` to the realfft tap. The branch decision is made before any of Units 12/13's PR work begins so downstream code is not destabilized by it.

**Requirements:** R17 (PCM ring half), D14, D15. Resolves round-2 review F1 / AR-04 / AR-10 (rtrb-is-SPSC contradiction + cross-process memory ordering risk).

**Dependencies:** Units 9, 10.

**Files:**
- Create: `crates/clitunes-engine/examples/pcm_spmc_shm_ring_spike.rs` (Phase A spike binary)
- Create: `docs/spikes/2026-04-XX-spmc-shm-ring-spike.md` (write-up: loom results, cross-process numbers, branch decision)
- Create: `crates/clitunes-engine/src/pcm/transport_trait.rs` (the shared abstraction both paths implement)
- Create: `crates/clitunes-engine/src/pcm/shm_ring.rs` (Phase B-primary: SPMC seqlock impl, only built if Phase A passes)
- Create: `crates/clitunes-engine/src/pcm/shm_layout.rs` (header layout, format version, cache-line padding)
- Create: `crates/clitunes-engine/src/pcm/overrun.rs`
- Create: `crates/clitunes-engine/src/pcm/mpsc_fanout.rs` (Phase B-fallback: per-consumer mpsc fan-out + Unix datagram delivery, only built if Phase A fails)
- Modify: `crates/clitunes/src/bin/clitunesd.rs` (producer side; constructs whichever transport Phase A's branch chose)
- Modify: `crates/clitunes-engine/src/cli/control_client.rs` (consumer side)
- Modify: `crates/clitunes-engine/src/visualiser/realfft_tap.rs` (read via the `PcmTransport` trait; consumes from whichever impl was chosen)
- Test: `crates/clitunes-engine/tests/pcm_shm_ring_loom_tests.rs` (loom-gated)
- Test: `crates/clitunes-engine/tests/pcm_shm_ring_cross_process_tests.rs` (parent forks; child reads from same shm path)
- Test: `crates/clitunes-engine/tests/pcm_mpsc_fanout_tests.rs` (only meaningful if fallback is taken)
- Test: `crates/clitunes-engine/tests/pcm_overrun_tests.rs`

**Approach:**
- Memory layout: a single `mmap`'d file at `$XDG_RUNTIME_DIR/clitunes/pcm-ring.shm`, created with `umask(0o177)` (mode 0600). Header (cache-line-padded): write cursor (u64 atomic), config (sample rate, channel count, sample format, slot size, slot count, format version). Body: ring of fixed-size slots, each slot prefixed by a u64 monotonic write-sequence written *after* the sample data (release semantics). **Consumers `mmap` the file with `PROT_READ` only**; the file descriptor handed to clients is opened `O_RDONLY`. Only the daemon producer maps `PROT_READ | PROT_WRITE`. This means a misbehaving or compromised client cannot scribble on the ring header or another consumer's view.
- Producer (daemon): single producer (the cpal callback's tee). The cpal callback delivers ~1024 frames per call, but the slot size is 2048 stereo frames; the producer maintains a small in-callback accumulator (a `[f32; 4096]` scratch on the audio thread) that holds partially-filled slot bytes. When the accumulator hits the slot boundary it copies into the slot, then writes the write-sequence with `Release`, then bumps the write cursor with `Release`. Wait-free; no allocation on the audio thread.
- Consumer (each client): reads the write cursor with `Acquire`, picks a slot N back from the cursor (e.g. cursor - 1 to get the most recently completed slot), reads the slot's write-sequence with `Acquire`, reads the samples, re-reads the write-sequence with `Acquire`, compares: if changed, the producer overran the consumer mid-read → consumer reports an overrun and skips this read. Otherwise the samples are valid.
- Overrun reporting: each consumer maintains its own `dropped_windows` counter, exposed to the visualiser's debug overlay (already wired in Unit 4).
- Slot size: 2048 stereo f32 samples = 16 KB per slot. Slot count: 64 (≈1.4 seconds of buffered audio). Total ring size: ~1 MB. Fits comfortably in tmpfs.
- Format-version field in the header lets future v1.1 changes (sample rate change, format change) be detected by clients without an `mpsc` round trip — the consumer reads the header on each tap, and if the format version doesn't match, disconnects and re-handshakes via the control bus.

**Patterns to follow:**
- `rtrb` source for the lock-free SPMC pattern (likely vendored, since `rtrb` is SPSC and we need SPMC).
- `ringbuf::SharedRb` if it supports cross-process via `memmap2` (verify in implementation).
- The classic "Petersen / Lamport-style" sequence-locked read pattern for the write-sequence verification.

**Test scenarios:**
- Happy path: producer writes 100 slots, consumer reads all 100 in order, zero overruns.
- Edge case: producer writes 200 slots while consumer is paused. Consumer resumes, detects 136 overruns (200 written - 64 ring slots), reports them via the counter, reads the most recent slot cleanly.
- Edge case: two consumers attached simultaneously. Both read the same 100 slots independently. Neither sees an overrun (single producer is fast enough).
- Edge case: format version in the header changes mid-run. Consumer detects mismatch on next read, disconnects, re-handshakes via control bus, re-attaches with the new format.
- Edge case: shm file doesn't exist when consumer tries to map. Consumer reports a clear error and waits for the daemon to create it (via control-bus event).
- Edge case: shm file is on a filesystem that doesn't support sparse mmap (rare). `mmap` fails with EINVAL; consumer surfaces a platform-incompatibility error.
- Failure path: producer writes a corrupt write-sequence (bug). Consumer detects torn read on the verification re-read, reports an overrun for that slot, continues.
- Integration: full pipeline with two clients — one running Auralis, one running `--pane mini-spectrum`. Both render the same audio. Verify with a deterministic source (calibration tone) that both clients produce the same FFT output.

**Verification:**
- Tests pass.
- Manual: launch `clitunesd`, then launch `clitunes` (default UI) and `clitunes --pane mini-spectrum` in separate terminals. Both react to the same audio. (`--pane mini-spectrum` lands in Unit 15 — until then, validate with a debug consumer binary in `crates/clitunes-engine/examples/pcm_`.)
- Verified zero data races under `--cfg loom` with the `loom` crate for the producer/consumer interleaving (if feasible — `loom` is heavyweight; soft target).

- [ ] **Unit 12: Socket security (mode 0600 + `SO_PEERCRED`/`LOCAL_PEERCRED` UID gating)**

**Goal:** Land the security hardening for the control socket and the shm file. Per D19, this is a hand-rolled `peercred.rs` with `#[cfg]` branches for Linux and macOS.

**Requirements:** R17 (security half), D19. Resolves security-lens P0 (control socket access).

**Dependencies:** Units 9, 11.

**Files:**
- Create: `crates/clitunes-engine/src/daemon/peercred.rs`
- Modify: `crates/clitunes-engine/src/daemon/control_bus.rs` (peercred check in accept loop)
- Modify: `crates/clitunes-engine/src/daemon/lifecycle.rs` (`umask(0o177)` wrapping `bind` on socket creation)
- Modify: `crates/clitunes-engine/src/pcm/shm_ring.rs` (`umask(0o177)` wrapping shm file creation; consumers `mmap` `PROT_READ` only)
- Test: `crates/clitunes-engine/tests/daemon_peercred_tests.rs`

**Approach:**
- `peercred.rs` exports `fn peer_uid(stream: &UnixStream) -> io::Result<u32>`.
- Linux: `getsockopt(fd, SOL_SOCKET, SO_PEERCRED)` returns `struct ucred { pid, uid, gid }`. Use `nix::sys::socket::getsockopt` or hand-rolled `libc` call.
- macOS / FreeBSD: `getpeereid(fd, &mut uid, &mut gid)` from `libc`.
- Other OSes: not v1 targets; return an error and let the daemon refuse to start.
- Socket creation: wrap `bind` in `umask(0o177)` (set before `bind`, restore after). This eliminates the race window of the previous chmod-after-bind approach — there is no point in time when the socket exists at any mode other than `0600`. Belt-and-braces: also `chmod 0600` immediately after `bind` returns, so a process that inherited an unexpected umask still ends up correct.
- Shm file creation: same `umask(0o177)` wrapper around `OpenOptions::new().mode(0o600).custom_flags(O_NOFOLLOW)` via `std::os::unix::fs::OpenOptionsExt`. Consumers `mmap` the file with `PROT_READ` only (never `PROT_WRITE`); only the daemon producer maps `PROT_READ | PROT_WRITE`.
- Accept loop: after `accept`, call `peer_uid`, compare to `unsafe { libc::getuid() }`. Mismatch → close the stream immediately, log a warning with the offending UID and PID.

**Execution note:** Test-first. Write a test that spawns the daemon, drops privileges to a different UID in a child process, attempts to connect, and verifies the connection is refused.

**Patterns to follow:**
- The `nix` crate's `UnixCredentials` API for the Linux side.
- macOS `getpeereid` man page (the call signature is identical on FreeBSD).

**Test scenarios:**
- Happy path: client connects from same UID. `peer_uid` returns daemon UID. Connection accepted.
- Failure path: client connects from a different UID (simulated by `setuid` in a child process within a test fixture, or via `sudo -u nobody` in a manual integration test). Connection refused immediately, daemon logs the rejection with the offending UID.
- Edge case: peercred call fails (e.g., on an unsupported OS). Connection refused, daemon logs "peercred unsupported on this platform" and exits — clitunes does not run on platforms where peercred isn't available.
- Edge case: socket file mode is verified to be 0600 immediately after `bind` returns, with no chmod call having happened yet — proves the umask path works even if the belt-and-braces chmod were removed.
- Edge case: shm file mode is verified to be 0600 after creation.

**Verification:**
- Tests pass, including the cross-UID rejection test.
- Manual: `stat $XDG_RUNTIME_DIR/clitunes/ctl.sock` shows `srw-------`.
- Manual: `sudo -u nobody clitunes status --json` (assuming clitunesd is running as the user) fails with a clear "permission denied" error.

- [ ] **Unit 13: Client refactor — `clitunes` connects to daemon, drops in-process scaffolding**

**Goal:** Refactor the slice-1 / slice-2 in-process pipeline to use the daemon. After this unit, `clitunes` is a thin client: it connects to the daemon's control socket, subscribes to the PCM shm ring, and runs the visualiser locally. The calibration tone source moves into the daemon. The in-process `rtrb` ring is deleted.

**Requirements:** Architectural; no new external requirements. This is the refactor that honors D10 + D12.

**Dependencies:** Units 9, 10, 11, 12.

**Files:**
- Modify: `crates/clitunes/src/main.rs`
- Modify: `crates/clitunes-engine/src/cli/control_client.rs`
- Modify: `crates/clitunes-engine/src/tui/lib.rs`
- Modify: `crates/clitunes-engine/src/visualiser/realfft_tap.rs` (consume from shm ring)
- Delete: `crates/clitunes-engine/src/pcm/in_process_ring.rs` (no longer used; kept under `#[cfg(test)]` if needed for unit tests)
- Modify: `crates/clitunes-engine/src/cli/auto_spawn.rs`
- Test: `crates/clitunes/tests/cli_end_to_end_tests.rs`

**Approach:**
- `clitunes` startup sequence:
  1. Try `connect()` to control socket.
  2. On ENOENT/ECONNREFUSED, fork+pipe-readiness auto-spawn (Unit 9 path).
  3. Read banner, verify protocol version, send `capabilities`.
  4. Send `subscribe_pcm` to get the shm file path and format.
  5. Map the shm file, start the visualiser thread tapping it.
  6. Send `idle` to subscribe to state events.
  7. Run the ratatui main loop until `q`.
  8. On exit, send `noidle`, close cleanly.
- The visualiser thread no longer touches the source layer. It's pure: shm ring → realfft → wgpu → Kitty.
- `clitunes status --json` becomes a thin one-shot client: connect, send `status`, print response, exit. Used for status-line integration.
- Headless verbs (`clitunes play`, `clitunes pause`, etc.) become thin one-shot clients.

**Patterns to follow:**
- The slice-1 and slice-2 client code, just gutted of the in-process source/ring/cpal calls.

**Test scenarios:**
- Happy path: cold launch — daemon doesn't exist. `clitunes` auto-spawns, attaches, plays the calibration tone or last-station, displays Auralis. Time-from-launch-to-first-pixel ≤3 seconds.
- Happy path: warm launch — daemon already running with another client attached. `clitunes` attaches as a second client, both display the same audio.
- Edge case: daemon crashes mid-session (kill -9). Client detects EOF on the control socket, prints "daemon disconnected; reattaching..." and auto-respawns the daemon (passing through the crash-loop guard from Unit 9).
- Edge case: daemon protocol version is newer than client. Client surfaces a clear "upgrade clitunes" message.
- Failure path: `clitunesd` binary not in PATH (not installed). Client surfaces "could not find clitunesd; check installation."
- Integration: end-to-end test — fresh state, launch `clitunes`, picker appears (from Unit 8), pick station, audio plays, Auralis runs, `q` quits, daemon idles, daemon exits after timeout.

**Verification:**
- All tests pass.
- Manual: the slice-3 success criteria — two clients attached to the same daemon, daemon survives killing one client, daemon respawns when killed and the other client reconnects.
- The `cargo tree -e features --bin clitunesd | grep -qE 'wgpu|ratatui|crossterm'` check from Unit 2 still passes (returns non-zero — no visualiser/tui deps in the daemon).

### Phase 4 — Slice 4: Local files + layouts + standalone panes + visualiser fleet

This phase grew in round-2 review from 3 units to 5: the original local-files / layout-DSL / `--pane`-clients work (Units 14–16) plus the two additional v1 visualisers (Units 17 Tideline + 18 Cascade) that the visualiser-first identity claim requires. Tideline and Cascade are placed here, after the daemon split is real, because (a) they're the forcing function that proves the `Visualiser` trait is rendering-path-agnostic (Cascade has zero wgpu deps) and proves the `--pane` "different visualisers of the same audio" story works end-to-end, and (b) the local-files source from Unit 14 gives them a reliable, repeatable signal source for visual polish work that radio cannot provide (a station dropping out mid-tune is the wrong test environment for shader iteration).

- [ ] **Unit 14: Local files source (CLI args + folder scan + lofty tags + queue)**

**Goal:** Implement the local files source. `clitunes ~/Music/album.flac` plays a single file. `clitunes ~/Music` recursively scans the folder and queues all supported files in directory order (no library, no SQLite, no tag database). Tag reads via `lofty` for the now-playing display.

**Requirements:** R4, R5, R8 (source switching), R20 (`next`/`prev`).

**Dependencies:** Phase 3 complete.

**Files:**
- Create: `crates/clitunes-engine/src/sources/local/mod.rs`
- Create: `crates/clitunes-engine/src/sources/local/folder_scan.rs`
- Create: `crates/clitunes-engine/src/sources/local/queue.rs`
- Create: `crates/clitunes-engine/src/sources/local/symphonia_file.rs`
- Create: `crates/clitunes-core/src/track.rs`
- Modify: `crates/clitunes/src/bin/clitunesd.rs` (register `LocalSource`)
- Modify: `crates/clitunes/src/main.rs` (CLI args parsing for paths)
- Test: `crates/clitunes-engine/tests/sources_local_source_tests.rs`

**Approach:**
- CLI parsing: `clitunes [PATH...]`. Each PATH is either a file or a directory. Files are added to the queue in the order given. Directories are recursively walked (`walkdir` crate), files are filtered by extension (`.mp3 .flac .ogg .opus .wav .m4a .aac`), then added in alphabetical path order.
- The queue is a `VecDeque<Track>` owned by the daemon. `next` pops the front, `prev` pushes back.
- Each `Track` carries: file path, lofty-extracted tags (title, artist, album, albumartist, track number, year, duration, embedded art bytes if present). **All free-text tag fields are passed through `clitunes_core::untrusted_string::sanitize` at extraction time** (D20). MP3/FLAC/Vorbis tags are arbitrary user data — a hostile filename or doctored tag could embed terminal control sequences that break out of the now-playing pane on render.
- `LocalSource::next_packet` opens the file via symphonia (with `is_seekable() = true` for local files), decodes packets, pushes to the PCM ring. Same downstream pipeline as radio.
- Source switching: `:source local <path>` and `:source radio <station>` are control verbs (Unit 10's verb list, completed here).
- Album art: lofty's `Picture::data() -> &[u8]` gives the embedded image bytes. Cached per-track in memory; passed to the `now-playing` pane component for the Kitty graphics-protocol image display (slice 4 polish).

**Patterns to follow:**
- `walkdir` for the recursive scan.
- `lofty`'s `Probe::open(path)?.read()?` for tag extraction.
- `symphonia`'s `examples/basic-interleaved.rs`, but with a `File` `MediaSource`.

**Test scenarios:**
- Happy path: `clitunes ~/Music/test.flac` plays the file, displays its tags in now-playing, exits cleanly when done.
- Happy path: `clitunes ~/Music` scans, queues, plays in order, advances on track end.
- Happy path: `next` and `prev` skip tracks within the queue.
- Happy path: switching from radio to local mid-session (`:source local ~/Music/foo.flac`) tears down the radio source and brings up the local source without dropping a frame on the visualiser pipeline (it just goes silent for a moment as the source swaps).
- Edge case: folder contains 0 supported files. Source emits a clear "no playable files in <path>" error.
- Edge case: file with no tags. now-playing falls back to the filename.
- Edge case: file with embedded art larger than 5 MB. Art is loaded, but the now-playing pane scales the display via Kitty's `c=`/`r=` cell sizing rather than transmitting full resolution. (Actual image transmission is in slice 4 polish, not Unit 14.)
- Edge case: file is a symlink. Followed normally.
- Edge case: file is corrupt. Decode error logged, source skips to next track in queue.
- Edge case: folder contains 100,000 files. Scan is bounded by a configurable `local.scan.max_files` (default 50,000); if exceeded, the source surfaces a warning and queues the first N. Avoids DoS via massive directory.
- Integration: CLI `clitunes test_track.flac` with the daemon already running attaches as a client, plays the file, the visualiser is reactive.

**Verification:**
- Tests pass.
- Manual: scan a real ~/Music directory, play through 10 tracks, verify tag display matches reality.

- [ ] **Unit 15: TOML layout DSL + ratatui integration**

**Goal:** Implement the declarative recursive layout DSL per R21–R23. Named layouts in TOML, runtime switching, terminal-resize handling with a fallback ladder.

**Requirements:** R21, R22, R23, R24, R25.

**Dependencies:** Phase 3 complete.

**Files:**
- Create: `crates/clitunes-engine/src/layout/lib.rs`
- Create: `crates/clitunes-engine/src/layout/parser.rs`
- Create: `crates/clitunes-engine/src/layout/tree.rs`
- Create: `crates/clitunes-engine/src/layout/component_registry.rs`
- Create: `crates/clitunes-engine/src/layout/resize_ladder.rs`
- Create: `crates/clitunes-engine/examples/layout_default_layout.toml`
- Create: `crates/clitunes-engine/examples/layout_compact_layout.toml`
- Create: `crates/clitunes-engine/examples/layout_minimal_layout.toml`
- Create: `crates/clitunes-engine/examples/layout_pure_layout.toml`
- Create: `crates/clitunes-engine/examples/layout_fullscreen_layout.toml`
- Modify: `crates/clitunes-engine/src/tui/lib.rs` (use layout tree to assign panes)
- Test: `crates/clitunes-engine/tests/layout_parser_tests.rs`
- Test: `crates/clitunes-engine/tests/layout_resize_ladder_tests.rs`

**Approach:**
- TOML schema sketch (directional, not final):

  ```toml
  [layouts.default]
  fallback = "compact"
  root = { split = "horizontal", ratios = [3, 1], children = [
      { split = "vertical", ratios = [4, 1], children = [
          { component = "visualiser" },
          { component = "now-playing" },
      ]},
      { component = "source-browser" },
  ]}
  min_size = { cols = 80, rows = 24 }

  [layouts.compact]
  fallback = "minimal"
  root = { split = "vertical", ratios = [4, 1], children = [
      { component = "visualiser" },
      { component = "now-playing" },
  ]}
  min_size = { cols = 60, rows = 18 }

  [layouts.minimal]
  fallback = "fullscreen"
  root = { component = "now-playing" }
  min_size = { cols = 40, rows = 6 }

  [layouts.fullscreen]
  root = { component = "visualiser" }
  min_size = { cols = 20, rows = 5 }
  ```

- Parser: `serde` derives + `toml` crate. Strict — unknown components are an error, malformed splits are an error, missing `min_size` defaults to (1, 1).
- Tree: walks the recursive `LayoutNode` enum, converts to ratatui `Constraint::Ratio` calls per split, returns a flat list of `(ComponentName, Rect)` to render.
- Component registry: a small HashMap from component name to render function. v1 components: `visualiser`, `now-playing`, `source-browser`, `queue`, `mini-spectrum`, `command-bar`. Each has a `min_size: (cols, rows)` declared.
- Resize ladder: on terminal resize, walk the current layout's tree. If any component's rect is smaller than its declared min, fall back to the layout's `fallback` layout. Recurse. If all layouts fail their min, render a single "terminal too small" message.
- Layout switching at runtime: control verb `:layout <name>` (handled by Unit 10 + this unit).
- The default layout per R24 is the `default` example above; "structurally good" but never aesthetically prescriptive.

**Patterns to follow:**
- ratatui's built-in `Layout` API for `Constraint::Ratio` splits.
- `taffy` is mentioned in the brainstorm as a candidate, but for v1 ratatui's built-in layout is sufficient and avoids a heavy dep. The DSL is decoupled from the renderer, so swapping to `taffy` later is bounded to the `tree.rs` module.

**Test scenarios:**
- Happy path: load `default_layout.toml`, render at 120×40, verify the visualiser pane gets the largest rect.
- Happy path: switch from `default` to `compact` at runtime, verify the source-browser pane disappears and the visualiser fills more space.
- Edge case: terminal is resized to 50×15 while `default` is active. min_size fails (60×18 minimum), code falls back to `compact`, then `compact`'s min fails (60×18), falls back to `minimal`, then to `fullscreen`. Renders fullscreen visualiser with no chrome.
- Edge case: terminal is resized to 10×3 — smaller than even `fullscreen`'s min (20×5). Code renders the "terminal too small" placeholder (`clitunes — resize to at least 20×5 to display`).
- Edge case: malformed TOML in user config. Parser surfaces a clear error pointing to the line, daemon falls back to the embedded default layouts.
- Edge case: user defines a layout that references an unknown component (`{ component = "fnord" }`). Parser surfaces a clear error listing the available components.
- Edge case: user defines a circular fallback (`compact -> default -> compact`). Parser detects the cycle and rejects with an error.
- Integration: full layout switching cycle — `:layout fullscreen`, `:layout default`, `:layout compact`. Each switch is reflected in the visible UI within one frame.

**Verification:**
- Tests pass.
- Manual: 5 example layouts each render correctly in their target terminal size.
- Resize ladder verified by manually shrinking the terminal across the boundary points.

- [ ] **Unit 16: Standalone `--pane` clients + headless verbs + `clitunes status --json`**

**Goal:** Wire `clitunes --pane <name>` as a standalone-process client that subscribes to the daemon and renders one component fullscreen. Implement the headless control verbs (`play`, `pause`, etc.) and `clitunes status --json` as one-shot clients.

**Requirements:** R18, R19, R20.

**Dependencies:** Units 13, 15.

**Files:**
- Create: `crates/clitunes-engine/src/cli/pane_mode.rs`
- Create: `crates/clitunes-engine/src/cli/headless_verbs.rs`
- Create: `crates/clitunes-engine/src/cli/status_command.rs`
- Modify: `crates/clitunes/src/main.rs` (arg dispatch)
- Test: `crates/clitunes/tests/cli_pane_mode_tests.rs`
- Test: `crates/clitunes/tests/cli_headless_verbs_tests.rs`

**Approach:**
- `clitunes --pane visualiser` — fullscreen visualiser only, no chrome. Honours `--viz auralis|tideline|cascade` (default Auralis). Auralis and Tideline use the wgpu+Kitty pipeline; Cascade uses the unicode-block path.
- `clitunes --pane now-playing` — text-only now-playing strip with optional embedded art (Kitty graphics) at the left.
- `clitunes --pane mini-spectrum` — small spectrum bars in pure unicode block characters (no wgpu, no Kitty graphics — intended for status-line embedding in a 1-row-tall slot where wgpu+Kitty is overkill). Effectively a tiny preset of Cascade's CPU renderer.
- `clitunes status --json` — connect, send `status`, print pretty-printed JSON to stdout, exit. Suitable for status-line shell integration.
- `clitunes play|pause|next|prev|volume|source|vis` — connect, send the corresponding command, exit on response. Suitable for system media key bindings.
- All `--pane` clients respect `q` to quit and clean up the daemon connection.

**Patterns to follow:**
- The default UI client from Unit 13, just gutted of layout tree code.
- `clap` derive macros for the verb dispatch.

**Test scenarios:**
- Happy path: `clitunes --pane mini-spectrum` runs in a 1-row slot inside tmux, displays bars, exits on `q`.
- Happy path: `clitunes --pane visualiser` runs fullscreen, displays Auralis identical to the default UI's visualiser pane.
- Happy path: `clitunes status --json` prints `{"track":..., "source":..., "position":..., "duration":..., "visualiser":..., "layout":...}` and exits 0.
- Happy path: `clitunes play` connects, plays, exits 0.
- Edge case: `clitunes --pane mini-spectrum` is run in a single-column-tall pane with no bottom border. Renders correctly.
- Edge case: `clitunes status --json` is run when daemon is not running. Auto-spawns the daemon (per Unit 9), prints status, daemon idles after exit.
- Edge case: two `--pane visualiser` clients running simultaneously. Both render the same audio. Validates the multi-client SPMC ring (Unit 11) end-to-end.
- Failure path: `clitunes --pane fnord` (unknown pane). Exits with "unknown pane: fnord. Available: visualiser, now-playing, mini-spectrum."
- Happy path: `clitunes --pane visualiser --viz cascade` runs fullscreen Cascade, demonstrating the CPU-only rendering path on terminals without Kitty graphics.
- Integration: a real tmux session with a `mini-spectrum` pane in the status bar, a `visualiser` pane in a side window, and a coding editor in the main pane, all running for 30 minutes without crashes (manual SC3 dry-run).

**Verification:**
- Tests pass.
- Manual: real-world tmux/wezterm/ghostty embedding scenarios match SC3.
- `clitunes status --json` integrated into a sample status-line shell snippet that updates every 5 seconds without leaking processes.

- [ ] **Unit 17: Tideline visualiser — GPU waveform / fluid / minimal monochrome**

**Goal:** Ship the second of v1's three flagship visualisers. Tideline is an instantaneous time-domain waveform, rendered through the same wgpu→Kitty pipeline as Auralis but with a deliberately opposite aesthetic: monochrome, fluid, minimal, contemplative. Where Auralis is maximalist (bloom, beat-sync camera, many bars), Tideline is a single morphing line that breathes with the audio. This unit is the forcing function that proves the Visualiser trait can host genuinely different visualisers, not just different shaders for the same idea.

**Requirements:** R10 (Tideline), R11, R12, R13. Resolves PF8.

**Dependencies:** Units 4 (Visualiser trait + wgpu pipeline + Kitty rect), 11 (cross-process PCM ring — Tideline consumes raw PCM, not FFT bins, because it visualises the time domain not the frequency domain), 13 (default UI integration — Tideline is selectable from the picker and `:viz tideline` command).

**Files:**
- Create: `crates/clitunes-engine/src/visualiser/tideline/mod.rs`
- Create: `crates/clitunes-engine/src/visualiser/tideline/wave.wgsl` (waveform tessellation + line shader)
- Create: `crates/clitunes-engine/src/visualiser/tideline/fluid.wgsl` (fluid simulation that drives the line's curvature based on RMS energy)
- Create: `crates/clitunes-engine/src/visualiser/tideline/pcm_buffer.rs` (rolling 2048-sample stereo buffer with 8-tap Lanczos resample to terminal column count)
- Test: `crates/clitunes-engine/tests/visualiser_tideline_smoketest.rs`
- Test: `crates/clitunes-engine/tests/visualiser_tideline_pcm_buffer_tests.rs`

**Approach:**
- Tideline implements the same `Visualiser` trait as Auralis (`SurfaceKind::Gpu`, `render_gpu(ctx, encoder, view)`). It is **not** a fork of Auralis; it has its own render module, its own shaders, its own PCM consumer.
- Time-domain consumer: Tideline subscribes to the raw PCM stream from the SPMC ring (Unit 11), maintains a rolling 2048-sample stereo buffer, and resamples to one f32 per terminal column per frame using an 8-tap Lanczos resampler. **No FFT.** This is the key axis of difference from Auralis.
- Waveform shader: tessellate the resampled samples into a triangle strip representing a 4-pixel-thick line. Anti-aliased edges. Single hue (configurable; default cool monochrome based on a per-track palette extracted from the Icy genre tag — falls back to a fixed cool grey if no genre).
- Fluid shader: compute per-frame RMS energy across a 256ms window, drive a simple 2D fluid simulation (advection + decay, no pressure solve — keeps it cheap) that perturbs the line's vertical position smoothly. The fluid simulation runs on the GPU as a separate compute pass before the line shader.
- "Minimal" guarantee: no bloom, no glow, no particles, no beat detection, no camera motion. Tideline is the visualiser for users who find Auralis overstimulating.
- Calibration tone behavior: a 440 Hz sine wave produces a clean morphing horizontal line — visually distinctive enough that users instantly understand the placeholder state.
- Frame budget: must hit the same target the Unit 1 spike committed to (60fps or 30fps). The fluid sim is the most expensive new component; if it pushes Tideline over budget on Linux+WezTerm, the fallback is to halve the fluid grid resolution.

**Patterns to follow:**
- Auralis's wgpu setup (Unit 4) — Tideline reuses the staging ping-pong, the poll thread, the Kitty writer, and the frame-drop counter wholesale. The only Tideline-specific code is the shader pipeline and the PCM buffer.
- The fluid sim algorithm is the standard "advect + decay" loop from Jos Stam's "Stable Fluids" paper, simplified to a single advection pass per frame (no pressure projection).

**Test scenarios:**
- Happy path: Tideline runs on the calibration tone for 5 minutes on M1 + Ghostty without exceeding the committed frame budget. Visual: a single morphing horizontal line that breathes with the 0.5 Hz amplitude modulation envelope.
- Happy path: Tideline running on real radio audio (BBC Radio 6 Music or similar live source from Unit 6) — visually distinct from Auralis on the same audio, side-by-side.
- Edge case: silent input (audio source dropped, ring all zeros). Line goes flat horizontal but keeps animating fluid decay so the visualiser doesn't appear frozen.
- Edge case: extreme dynamics (sudden loud transient after silence). RMS spike drives the fluid sim harder; line should curve dramatically without clipping or NaN propagation.
- Edge case: terminal resize mid-frame. PCM buffer resamples to the new column count without dropping a frame.
- Performance regression: `cargo bench --bench tideline_frame` (criterion) measures p99 frame time, fails CI if it regresses >10% from baseline.

**Verification:**
- Smoketest passes.
- Manual: `clitunes --viz tideline` on M1 + Ghostty, M1 + Kitty, Linux + Kitty, Linux + WezTerm — all hit the committed frame budget on the calibration tone.
- Side-by-side screenshot of Auralis vs Tideline on the same 10-second audio clip committed to `docs/screenshots/auralis-vs-tideline.png` — visually demonstrates the maximalist/minimal axis.

- [ ] **Unit 18: Cascade visualiser — pure-CPU spectrogram waterfall / historical / unicode blocks**

**Goal:** Ship the third of v1's three flagship visualisers. Cascade is a spectrogram waterfall — historical time domain (the last ~30 seconds of audio scroll up the pane), pure-CPU rendered with unicode block characters via ratatui's standard buffer, **zero wgpu, zero Kitty graphics**. This is deliberately the visualiser that runs on terminals where Auralis and Tideline cannot. It is also the forcing function that proves the `Visualiser` trait can host both `SurfaceKind::Gpu` and `SurfaceKind::Tui` implementations — without Cascade in v1, the trait would never be exercised in the TUI path, and a future TUI visualiser would discover the trait was secretly GPU-only.

**Requirements:** R10 (Cascade), R11 (with caveat: Cascade is the visualiser whose frame budget is allowed to be lower because it's the fallback for terminals that can't do anything else), R13.

**Dependencies:** Units 4 (Visualiser trait — Cascade is the first `SurfaceKind::Tui` implementation), 11 (cross-process PCM ring — Cascade runs FFT itself on the raw PCM, doesn't share the realfft tap because it wants different window sizes and a longer history).

**Files:**
- Create: `crates/clitunes-engine/src/visualiser/cascade/mod.rs`
- Create: `crates/clitunes-engine/src/visualiser/cascade/history.rs` (rolling 30-second history of FFT magnitude rows, evicting oldest)
- Create: `crates/clitunes-engine/src/visualiser/cascade/render.rs` (history → ratatui Buffer with unicode blocks + 256-color background gradient)
- Create: `crates/clitunes-engine/src/visualiser/cascade/colormap.rs` (viridis-style perceptually-uniform 256-color LUT)
- Test: `crates/clitunes-engine/tests/visualiser_cascade_smoketest.rs`
- Test: `crates/clitunes-engine/tests/visualiser_cascade_history_tests.rs`

**Approach:**
- Cascade implements the `Visualiser` trait with `SurfaceKind::Tui` and `render_tui(ctx, buffer, area)`. It writes to a ratatui `Buffer`, not a wgpu texture. It must compile with `default-features = false, features = ["visualiser", "tui"]` — no wgpu, no Kitty deps.
- Time domain: Cascade owns a `History` struct of `VecDeque<Vec<f32>>` — one Vec per FFT row, evicted when older than 30 seconds. Each row is `area.width / 2` magnitude bins (one bin per two terminal columns; gives ~80–120 bins on typical terminals).
- FFT cadence: Cascade runs its own `realfft` planner on a 1024-sample window, hop size 512, on the raw PCM stream from the SPMC ring. Hop rate: 48000 / 512 ≈ 94 Hz. Frame render rate: 30 fps. Multiple FFT rows accumulate per frame.
- Render: each row becomes one terminal row. Vertical scrolling: newest row at the bottom, oldest at the top, scroll up by drawing rows 1..N at positions 0..N-1 each frame. Each cell is a unicode block (`▀ ▄ █`) with the foreground/background colors from the viridis colormap indexed by magnitude.
- Colormap: viridis 256-color LUT (perceptually uniform; readable on both light and dark backgrounds; well-known from matplotlib so users recognize the encoding intuitively).
- Configurability: `--viz cascade --colormap viridis|magma|grayscale|terminal` for users who prefer a different palette.
- Frame budget: Cascade is allowed to target 30 fps as a hard ceiling (it's not competing with Auralis on smoothness — it's the historical-context visualiser, and a slowly-scrolling waterfall at 30fps is correct behavior). p99 frame time ≤ 33ms. Pure CPU; no GPU dependency means it runs on every terminal that can render unicode.
- Calibration tone behavior: a 440 Hz sine produces a single bright horizontal stripe at the 440Hz row, with the slow amplitude modulation showing as periodic brightness pulsation along that row. Visually distinct.

**Patterns to follow:**
- ratatui's `BarChart` widget for the basic "draw a 2D buffer of colored cells" pattern.
- `colorgrad` crate for the viridis LUT (or hardcode the 256-entry table — it's small).
- The `realfft` reuse pattern from Unit 3, just with a different window/hop size.

**Test scenarios:**
- Happy path: Cascade runs on the calibration tone for 5 minutes in a fresh ratatui terminal. Visual: a horizontal stripe at the 440Hz row, scrolling up over 30 seconds.
- Happy path: Cascade runs on a terminal that explicitly does **not** support Kitty graphics (e.g., basic xterm via SSH). Renders correctly. This is the test that validates Cascade's reason for existence.
- Happy path: Cascade compiles with `cargo build -p clitunes-engine --no-default-features --features "visualiser tui"` — no wgpu, no Kitty in the dep tree. CI test: `cargo tree -p clitunes-engine --no-default-features --features "visualiser tui" | grep -qE 'wgpu|kitty'` returns non-zero.
- Edge case: terminal narrower than 40 columns. Cascade falls back to fewer bins per row (degrades gracefully) rather than crashing.
- Edge case: terminal taller than 60 rows. History buffer expands to fill; older audio remains visible longer.
- Edge case: terminal resize from 80×24 to 200×60 mid-render. History rebins from old column count to new column count without losing the historical data.
- Edge case: silent input. Cascade renders a uniform low-magnitude field (black or near-black) — empty but still scrolling so the user sees the "alive but no signal" state.
- Performance: `cargo bench --bench cascade_frame` measures p99 frame time, must stay ≤ 33ms.

**Verification:**
- Smoketest passes.
- The "no wgpu in deps" CI check passes.
- Manual: `clitunes --viz cascade` runs over SSH from M1 to a Linux box with `TERM=xterm-256color`, no Kitty graphics support — renders correctly for 5 minutes. This is the SC4 dry-run for "works on terminals without GPU paths."
- Side-by-side screenshot: Auralis (instantaneous, GPU, frequency, maximalist) | Tideline (instantaneous, GPU, time, minimal) | Cascade (historical, CPU, frequency, unicode) committed to `docs/screenshots/three-visualisers.png` — visually demonstrates that the three v1 visualisers occupy three genuinely different points in design space.

### Phase 5 — Slice 5: First-run polish + distribution

- [ ] **Unit 19: First-run UX polish + time-to-first-pixel measurement**

**Goal:** Validate and polish the SC1 first-run experience. Measure the time from `clitunes` invocation to the first Auralis frame on a fresh state dir; ensure it's ≤3 seconds. Polish the picker overlay's visual integration with Auralis behind it. Record a screenshot for marketing. The default first-run visualiser is **Auralis**; the picker also surfaces `:viz tideline` and `:viz cascade` as discoverable alternatives.

**Requirements:** R1, R26, SC1.

**Dependencies:** Phase 4 complete (Units 14–18).

**Files:**
- Modify: `crates/clitunes/src/main.rs` (timing instrumentation behind `--measure-startup` flag)
- Modify: `crates/clitunes-engine/src/tui/picker/mod.rs` (visual polish: spacing, framing, fade-in)
- Modify: `crates/clitunes-engine/src/visualiser/auralis/mod.rs` (calibration tone visual polish)
- Create: `docs/screenshots/2026-04-XX-first-run.png`
- Create: `docs/SC1-validation.md` (timing measurements + screenshots from 3 platforms)

**Approach:**
- Add `--measure-startup` flag to the `clitunes` binary that logs timestamps for: process start, daemon connected, picker shown, station selected, first audio frame, first Auralis frame. Output the durations as a tab-separated row to stderr.
- Run the measurement on M1 + Ghostty, M1 + Kitty, Linux + Kitty, Linux + WezTerm. Record results in `docs/SC1-validation.md`.
- If any platform exceeds 3 seconds, identify the binding step and optimize. Common binding steps: DNS SRV lookup (cache more aggressively), `wgpu` adapter request (warm in parallel with picker render), terminal capability probe (reduce sequence count).
- Picker visual polish: ensure the picker doesn't fight Auralis visually. The calibration tone behind the picker should be visually distinct from real audio (so users understand "this is a placeholder, your music will replace it") — tune the calibration tone to be visually quieter than typical music.

**Test scenarios:**
- Happy path: fresh state, `clitunes --measure-startup` shows total time ≤3000 ms on the user's M1 + Ghostty.
- Edge case: DNS SRV lookup takes >2 seconds (slow network). Picker shows with a "loading stations..." indicator instead of blocking.
- Edge case: `wgpu` adapter request takes >1 second (cold GPU). Picker shows with a calibration tone running on a CPU fallback gradient (not real wgpu) until the GPU is ready, then the wgpu pipeline takes over without a visible swap.

**Verification:**
- `docs/SC1-validation.md` has timing rows for ≥3 platform/terminal combinations, all ≤3000 ms.
- Screenshot of the first-run picker over Auralis is committed.

- [ ] **Unit 20: Distribution — single static binary CI matrix + Homebrew + cargo install + AUR**

**Goal:** Ship clitunes as a single static binary per platform with installation via Homebrew, `cargo install`, and AUR. Tag the v1.0.0 release.

**Requirements:** R27.

**Dependencies:** Phase 5 complete (Unit 19 must pass).

**Files:**
- Modify: `.github/workflows/ci.yml` (add release matrix on tag)
- Create: `.github/workflows/release.yml`
- Create: `dist/homebrew/clitunes.rb` (formula template)
- Create: `dist/aur/PKGBUILD`
- Create: `README.md` (replaces the placeholder from Unit 2; content per the brainstorm's distribution section, including the 3-visualiser identity)
- Create: `CHANGELOG.md`
- Create: `LICENSE` (final pick: MIT or Apache-2.0; default to MIT unless user overrides — supersedes Unit 2's placeholder pick)

**Approach:**
- Release CI on tags `v*`: build for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`. Strip binaries. Tar+gzip with checksums. Upload as GitHub release artifacts.
- Static linking: use `cargo zigbuild` for Linux (musl static) and the standard `cargo build --release` for macOS.
- Homebrew formula: a simple bottle pointing at the GitHub release artifacts.
- AUR PKGBUILD: builds from source via `cargo install --root pkgdir/usr/`.
- README structure: project pitch, screenshots (from Units 17, 18, 19 — three-visualisers side-by-side and first-run), 3-line install instructions, link to docs.
- CHANGELOG starting with v1.0.0.

**Test scenarios:**
- Happy path: tag `v1.0.0`, CI builds 4 artifacts, uploads them, `brew install clitunes` installs the macOS arm64 binary on a fresh macOS system, `clitunes` runs and shows the first-run picker.
- Edge case: glibc version mismatch on Linux release. `cargo zigbuild` with `--target x86_64-unknown-linux-gnu.2.17` to pin to a portable baseline.

**Verification:**
- A v1.0.0 release exists on GitHub with 4 artifacts and checksums.
- Manual install via Homebrew on a clean macOS box succeeds and runs.
- Manual install via `cargo install clitunes` on a clean Linux box succeeds and runs.

## System-Wide Impact

- **Interaction graph:** the daemon is the only process that holds OS audio resources (cpal stream, network sockets for radio, file handles for local files). All clients are pure renderers + a JSON control connection + an mmap. Daemon crash → all clients detect via the control socket EOF and either exit or auto-respawn the daemon. Client crash → daemon notices via the control socket EOF, decrements the client count, starts the idle timer if zero. The PCM ring is decoupled from connection state (clients can crash without affecting other clients' tap).
- **Error propagation:** layered. Source errors (network, file, decode) propagate to the daemon's player, which broadcasts a `source_error` event over the state bus. Clients display the error in the now-playing strip and may show a reconnect indicator. wgpu errors propagate within the client only — the daemon never sees them. Client errors (e.g. terminal capability probe failure) cause the client to exit with a clear message; the daemon is unaffected.
- **State lifecycle risks:** the SPMC PCM ring's overrun-detection is the primary correctness gate for the visualiser tap. If the write-sequence verification is wrong, clients render torn samples without knowing. Mitigated by Unit 11's `loom`-style test (best-effort) and by the Unit 11 happy-path test that compares two independent consumers' FFT outputs against a deterministic source. The state.toml file is the only persistent state that needs atomic write — all other state is daemon-in-memory and lost on restart.
- **API surface parity:** the control bus protocol is the public API for v1. Once shipped, it cannot be broken without bumping the protocol version in the banner. The capabilities command lets clients negotiate forward-compat. A future v1.1 mopidy-compat HTTP/JSON-RPC bridge would translate to/from this protocol; the protocol is shaped to make that translation trivial (verb names and event names are deliberately MPD-adjacent).
- **Integration coverage:** the cross-cutting integration tests are: (1) two clients on one daemon producing identical FFT output from a deterministic source (Unit 11), (2) daemon kill-and-respawn with attached clients (Unit 13), (3) end-to-end fresh-install first-run within 3 seconds (Unit 19), (4) a real tmux session with multiple `--pane` clients running for 30 minutes (Unit 16 manual), (5) Cascade rendering correctly on a Kitty-graphics-incapable terminal over SSH (Unit 18), (6) Auralis vs Tideline side-by-side on the same audio with both hitting frame budget (Units 4, 17). Unit tests with mocks alone will not prove these scenarios.
- **Unchanged invariants:** v1 explicitly does not change anything in the user's `~/Music` directory, anything in their Spotify account (no Spotify integration in v1), or anything in any other terminal. clitunes is read-only against the filesystem, write-only against `~/.config/clitunes/` and `$XDG_RUNTIME_DIR/clitunes/`, and write-only against `~/.cache/clitunes/`. No `~/.local/share/` writes. No system-wide config touched.

## Risks & Dependencies

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| **wgpu Metal readback >10 ms p99 on M1, killing 60 fps** | Medium | High (hero feature) | Unit 1 spike measures before any feature work. Fallback 1: drop to 30 fps target (still good visuals). Fallback 2: shrink the rendered texture to 1024×512 instead of full terminal pixel resolution. Fallback 3: switch to JPEG encoding (loses some visual quality but smaller frames). Fallback 4: drop wgpu and use a CPU spectrum renderer at lower visual ceiling. Fallback decision is captured in the spike doc. |
| **No public Rust prior art for wgpu→Kitty graphics streaming** | Confirmed | Medium | Unit 1 produces the first public reference implementation. Treat the spike as "we will be the prior art." Document the working pattern in `docs/spikes/` so it's reusable by other projects. |
| **Kitty graphics protocol coverage varies across terminals** | High | Medium | Unit 1 tests on Kitty, Ghostty, WezTerm. WezTerm's coverage is known to lag — if it fails, document WezTerm as v1.1 and ship v1 supporting Kitty, Ghostty, Rio only. |
| **radio-browser.info DNS SRV lookup returns zero mirrors** | Low | Medium | Cache last-known-good mirror list to disk (Unit 5). If both live SRV and cache fail, bundle a small fallback list of known-good mirrors in the binary. |
| **ICY metadata terminal-injection vector** | High (operators can inject) | High (terminal compromise) | Unit 6 sanitizer + fuzz test suite. Unit 6 tests include known attack payloads. Sanitizer is C0+C1+ESC stripping, not regex. |
| **Control socket exposed to other users on multi-user host** | Medium | High | Unit 12: 0600 mode + `SO_PEERCRED`/`LOCAL_PEERCRED` UID gate. Belt-and-braces with `$XDG_RUNTIME_DIR`'s 0700 default. |
| **`tokio::sync::broadcast::Sender` silently drops events** | Confirmed (research finding) | Medium | Unit 10: per-client bounded `mpsc` with disconnect-on-overflow. Never use `broadcast::Sender` on the state bus. |
| **MPD's blocking-FIFO anti-pattern** | N/A (we're using SPMC ring) | High if used | Unit 11: SPMC shm ring with overrun reporting, not a FIFO. Documented in D14. |
| **SPMC shm ring is harder than "pick a crate" implies** | High (`rtrb` is SPSC; `ringbuf::SharedRb` is in-process) | High (correctness of all visualiser data) | Unit 11 split into Phase A spike (3-day hard budget) with mandatory loom + cross-process tests, and Phase B implementation that branches: shm-ring on spike pass, **per-consumer mpsc fan-out + Unix datagram delivery on spike fail**. The fallback is slightly higher CPU but collapses correctness to "tokio's mpsc is correct." |
| **Cross-process memory ordering on aarch64 Apple Silicon** | Medium | High if not validated | Unit 11 Phase A's loom run is required to cover the producer/2-consumer interleaving on `--release`. The fallback path side-steps the problem entirely by moving cross-process data over a Unix datagram socket instead of shared memory. |
| **Daemon stale lock file from a crashed previous run** | Low | Low | `flock` releases on process death (Linux/macOS). Lock file approach is immune to stale-PID-file bugs. |
| **AAC codec in symphonia is less battle-tested** | Medium | Low | Unit 7 documents AAC as best-effort in v1. Most radio streams are MP3 or OGG anyway; AAC fallback is OK. |
| **Slice 1 spike (Unit 1) takes >1 week** | Medium | High (delays everything) | Spike has a hard 5-day budget. If 5 days pass without a clean go/no-go, the team makes a forced architecture decision (likely: drop wgpu, ship CPU-rendered Auralis at lower quality). Documented in spike doc. |
| **librespot upstream breaks before v1.1** | High (recurring) | Low for v1 (deferred) | v1 doesn't depend on librespot. v1.1 plan handles freshly. |
| **`cargo tree` daemon-no-wgpu CI check is bypassed accidentally** | Low | High | Unit 2 wires the check; Unit 13's PR review explicitly verifies the check is still passing. Architectural drift is mechanically enforced. |
| **User has multiple sound cards / output device confusion** | Medium | Low | cpal exposes device enumeration; v1 documents how to pick a specific device via `audio.output_device` config key. |
| **`/dev/shm` not available on macOS (it isn't)** | Confirmed | Low | Use `$TMPDIR` on macOS (which is per-session and 0700) and `/dev/shm` on Linux. Documented in Unit 1's transport selection. |

## Documentation / Operational Notes

- `README.md` (Unit 20): pitch + screenshots + install + quick-start + link to fuller docs.
- `docs/spikes/2026-04-11-wgpu-kitty-throughput-spike.md` (Unit 1): the load-bearing measurements. Future v1.1 visualisers must read this before implementing.
- `docs/SC1-validation.md` (Unit 19): timing measurements per platform.
- `docs/curation/2026-04-11-curated-stations.md` (Unit 8): the 12 curated stations with rationale per slot, plus the explicit "engineer taste audit."
- `docs/architecture.md` (write during Unit 13): the daemon-client architecture with the ASCII diagram from this plan, expanded.
- `docs/protocol.md` (write during Unit 10): the control bus wire format, banner, capabilities, verb list, event list. This is the public API contract.
- `docs/security.md` (write during Unit 12): threat model, peercred mechanism, ICY sanitizer, what clitunes does and doesn't protect against.
- Operational: clitunesd logs go to `~/.cache/clitunes/clitunesd.log` (rotated) and to stderr if run in foreground. Tracing level configurable via `RUST_LOG`.
- Rollout: v1 is a fresh release. No migration story needed because there's nothing to migrate from.
- Monitoring: the daemon exposes `clitunes status --json` for status-line scripts. No Prometheus, no telemetry, no analytics. Crashes are user-facing only; users file issues on GitHub.

## Review Findings Resolution

All 9 findings (PF1–PF9) carried into planning from `docs/brainstorms/clitunes-requirements.md`'s Review Findings section are addressed:

- **PF1** (scope vs identity) → resolved by **D8'** (visualiser-first v1; Spotify and additional visualisers deferred to v1.1).
- **PF2** (daemon-day-1 vs slice-1 in-process) → resolved by **D12** (slice 1 uses throwaway in-process scaffolding for the visualiser pipeline only; the real daemon lands in slice 3 / Phase 3, with bounded retrofit cost because no audio source code from slices 1-2 has in-process assumptions outside the swappable ring buffer).
- **PF3** (librespot fragility) → deferred to v1.1 by D8'. v1.1 plan must include the librespot 0.8.0 spike before any v1.1 unit is created.
- **PF4** (visual pipeline throughput unverified) → resolved by **Unit 1** (mandatory pre-commitment spike) + **D17** (double-buffered staging + dedicated poll thread architectural commitment) + risk-table fallback paths if the spike fails.
- **PF5** (dogfooding test unwinnable) → resolved by **SC2** (primary radio + local surface for 30 days; Spotify discovery stays in the official client).
- **PF6** (vanity metric) → resolved by **SC4** (3 unrelated users post screenshots without being asked).
- **PF7** (slice 1 too large) → resolved by **D12 + Unit 1 + Unit 4** (slice 1 is just the visualiser pipeline + calibration tone; radio source, picker, persistence, and reconnect all move to Phase 2).
- **PF8** (5 visualisers brainstormed not validated) → resolved by **R9 revised round-2** (v1 ships 3 visualisers — Auralis, Tideline, Cascade — selected on different design axes so the "plethora" claim is grounded by actual variety, not by counting; v1.1 candidates Pulse/Aether/Polaris must demo against real audio to earn slots).
- **PF9** (variety test unmeasurable) → resolved by **SC5** (variety test deferred to v1.1; v1.1 plan must define a real test before adding visualisers).

### Round-2 review findings (document-review pass on 2026-04-11)

A 7-persona document-review pass surfaced additional findings beyond the 9 PFs carried from brainstorm. Auto-fix findings (one clear correct answer each) were applied silently; strategic findings were resolved by user decision.

**Auto-applied fixes:**

- **F1** (rtrb is SPSC, not SPMC — original plan called it SPMC) → resolved by **Unit 11 rewrite** introducing Phase A spike + Phase B fallback path (per-consumer mpsc fan-out + Unix datagram delivery) since `rtrb` cannot satisfy the multi-consumer requirement.
- **F2** (Kitty graphics protocol rejects temp files whose path doesn't contain the literal substring `tty-graphics-protocol`) → resolved by **Unit 1 + Unit 4** approach updates specifying `mkstemp` template `tty-graphics-protocol-clitunes-XXXXXX.rgba`.
- **F7** (cpal callback buffer 1024 vs realfft window 2048 mismatch) → resolved by **Unit 3** approach update introducing a producer-side accumulator that fills 2048-sample slots from 1024-sample callbacks.
- **SEC-001** (control socket has a TOCTOU race between `bind` and `chmod`) → resolved by **Unit 9 + Unit 12** updates: set `umask(0o177)` immediately before `bind` so the socket inode is created mode 0600 atomically; the prior `chmod` is kept as belt-and-braces.
- **SEC-003** (`tempfile::NamedTempFile` doesn't guarantee `O_NOFOLLOW`+`O_EXCL`+0600 on all platforms) → resolved by **Unit 1 + Unit 4** approach updates specifying explicit `mkstemp` with `O_NOFOLLOW | O_EXCL` and mode 0600.
- **SEC-004** (untrusted strings from radio-browser, ICY headers, in-band ICY chunks, and lofty tags can inject ANSI escapes into the terminal) → resolved by **D20** + **Units 5, 6, 14** + **clitunes-core::untrusted_string::sanitize** module being applied at every ingestion boundary.
- **SEC-007** (LinesCodec without max length is a DoS vector — single attacker line can OOM the daemon) → resolved by **Unit 10** wire format update to `LinesCodec::new_with_max_length(65_536)`.
- **SEC-008** (consumer mmap is mapped writable, allowing one client to corrupt the ring for all other clients) → resolved by **Unit 11** memory-layout update specifying consumer mmap as `PROT_READ` only; the producer holds the only writable mapping.
- **SEC-011** (`state.toml` written with default umask, exposing user data) → resolved by **Unit 8** persistence update with explicit `chmod 0600` on the file and `0700` on the parent directory.
- **SEC-013** (no continuous advisory check for the sizable transitive surface from Symphonia, librespot, reqwest, tokio, wgpu, lofty) → resolved by **Unit 2** CI step adding `cargo deny check advisories bans sources licenses` against a checked-in `deny.toml`.
- **C1** (PF7 resolution text said Unit 8/9 lands in slice 2, but Unit 9 is the daemon binary lifecycle which lands in slice 3) → resolved by editing the PF7 resolution to read "Units 5–8 in slice 2; the daemon binary lifecycle (Unit 9) lands in slice 3."
- **C2** (D12 said the calibration tone is thrown away in slice 3, but D11 also requires a fallback when no source is selected) → resolved by clarifying in D12 that only the slice-1 *coupling* between calibration tone and the in-process ring is thrown away; the calibration tone source survives slice 3 as a "no source selected" placeholder per D11.

**Strategic findings resolved by user decision:**

- **PF10/PF14, F1** (SPMC ring complexity — 5 reviewers flagged the SPMC requirement as the single highest implementation risk in the plan) → user decision: **add SPMC shm ring spike + fallback path to Unit 11**. Phase A is a 3-day mandatory spike with `loom` model checking and a cross-process delivery test; Phase B branches based on the spike result (real shm ring if the spike validates, mpsc fan-out + Unix datagram delivery if it doesn't). Mechanical go/no-go gates documented in Unit 11.
- **PF11** (10-crate workspace overhead) → user decision: **collapse 10 crates to 3** (`clitunes-core`, `clitunes-engine`, `clitunes` binary crate with two `[[bin]]` targets). D15 enforcement is preserved via Cargo features rather than crate boundaries — the daemon binary declares `default-features = false, features = ["audio", "control", "sources"]` and CI greps `cargo tree -e features --bin clitunesd` for `wgpu`/`ratatui`/`crossterm`. **A `docs/backlog.md` item tracks splitting `clitunes-engine` further when real boundaries emerge** so the smaller-crate goal is not lost.
- **PF12** (daemon-client split too aggressive — 18 units, 5 of which are pure daemon plumbing) → user decision: **keep the daemon-client split as designed**. The split is what enables `--pane` clients, status-line embedding (SC3), and the multi-process model that makes the visualiser-first identity defensible against "this is just another TUI player." The 5 daemon plumbing units are the load-bearing infrastructure for everything in slices 3-5.
- **AR-04/10, SG-F2** (only 1 visualiser shipping in v1 contradicts the "plethora of visualisers" identity claim from the brainstorm) → user decision: **ship 3 visualisers (Auralis + Tideline + Cascade), vastly different from each other on multiple axes**. Auralis = GPU spectrum / instantaneous / maximalist. Tideline = GPU waveform / instantaneous / minimal monochrome. Cascade = pure-CPU spectrogram waterfall / historical / unicode blocks. The three v1 visualisers occupy three genuinely different points in design space (frequency vs time, GPU vs CPU, maximalist vs minimal, instantaneous vs historical), so the "plethora" claim is grounded by actual variety, not by counting variations on the same idea. See the rewritten **D8'**.
- **AR-11** (TOML layout DSL is premature — 3 reviewers noted that grid-based layouts could be hardcoded for v1) → user decision: **keep the TOML layout DSL as Unit 15**. The DSL is what enables SC3 (status-line embedding, side-window panes, custom layouts) without requiring a recompile per layout, and it's what the `--pane` story in Unit 16 leans on. Hardcoding 2-3 layouts for v1 would block the layout-as-config promise that makes clitunes feel different from cliamp's fixed UI.
- **AR-12** (frame budget threshold not pre-committed — Unit 1 spike has no go/no-go criteria) → user decision: **apply the pre-committed numerical thresholds to Unit 1**. 60fps bar = `p99 ≤ 16ms` AND `p95 ≤ 14ms`; 30fps bar = `p99 ≤ 33ms` AND `p95 ≤ 25ms`. Decision rule: ≥2 of 4 platforms hit 60fps → commit 60fps; else ≥3 of 4 hit 30fps → commit 30fps; else spike fails and the plan branches to the unicode-block fallback.
- **D-01..04, design lens** (Pane Content Sketches absent — reviewers noted that the plan defines components by name but never specifies their content, layout, or small-terminal behavior) → user decision: **add a Pane Content Sketches section near High-Level Technical Design**. The section was added with detailed content specs for `visualiser`, `now-playing`, `source-browser`, `queue`, `mini-spectrum`, and `command-bar`, each with edge case behavior at small terminal sizes.

**New risks added to the Risks table** (round-2):
- SPMC shm ring complexity (Unit 11 Phase A may fail; Phase B is the fallback; correctness collapses to "tokio mpsc is correct" if the shm ring path is abandoned).
- Cross-process memory ordering on aarch64 (the shm ring path requires verifying that producer/consumer atomic ordering holds across process boundaries on Apple Silicon and Linux ARM, not just within a single process).

## Sources & References

- **Origin document:** [docs/brainstorms/clitunes-requirements.md](../brainstorms/clitunes-requirements.md)
- **Memory:** `feedback_no_taste_imposition.md` (informs D11 + Unit 8's curated list audit)
- **External research findings (Phase 1.3):**
  - librespot: `librespot-org/librespot` repo + Audio Backends wiki + `librespot-playback/src/audio_backend/mod.rs`
  - wgpu: gfx-rs/wgpu releases, Learn wgpu windowless, wgpu#2266 (buffer map latency), wgpu discussion #1438 (per-frame uploads)
  - Kitty graphics: sw.kovidgoyal.net/kitty/graphics-protocol/, kitty discussion #3673 (60 fps confirmed), kitty discussion #5660 (tpix demo), Rio docs
  - ratatui-image: github.com/benjajaja/ratatui-image, docs.rs
  - Symphonia: github.com/pdeljanov/Symphonia, docs.rs
  - cpal: github.com/RustAudio/cpal, cpal#446 (ALSA buffer size)
  - lofty: github.com/Serial-ATA/lofty-rs, docs.rs
  - realfft: github.com/HEnquist/realfft, rustfft docs.rs
  - radio-browser: api.radio-browser.info, faq
  - WezTerm Kitty graphics: wezterm#2756, wezterm#6334
  - MPD protocol: mpd.readthedocs.io, github.com/MusicPlayerDaemon/MPD
  - MPD FIFO skipping: FreeBSD forums thread 63380
  - ncmpcpp: deepwiki ncmpcpp visualiser, ncmpcpp#226
  - cava: github.com/karlstav/cava, cava#197, cava#670 (PipeWire rewrite)
  - mpv IPC: github.com/mpv-player/mpv DOCS/man/ipc.rst
  - PipeWire: docs.pipewire.org page_overview, page_objects_design, Bootlin custom node post
  - ncspot: github.com/hrkfdn/ncspot
  - ringbuf: docs.rs/ringbuf, github.com/agerasev/ringbuf
  - tokio-seqpacket: docs.rs/tokio-seqpacket
  - uds crate, interprocess local_socket, nix UnixCredentials, tokio::net::UnixListener
  - Tokio backpressure: tokio.rs/tokio/tutorial/channels, biriukov.dev async-rust-tokio-io
  - envoy / gpg-agent socket activation, NixOS thread on gpg-agent socket activation
- **K2 Process:** [docs/k2-process.md](../../../../../Documents/GitHub/jackknife/docs/k2-process.md) — informed phase organization + slice-shippability discipline. (Note: this path is intentionally outside the clitunes repo because the K2 process docs live in jackknife.)
