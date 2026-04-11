---
date: 2026-04-10
topic: clitunes-requirements
---

# clitunes — Requirements

## Problem Frame

Existing terminal music players (cliamp, ncmpcpp, cmus, ncspot, spotify-tui) feel dated, single-purpose, and visually flat. None of them treat **the visualiser as the product**. Cliamp ships a handful of unicode-block visualisers and a small radio feature; ncmpcpp is a powerful but text-only library browser; ncspot is a clean Spotify client with no visualiser at all. Meanwhile, modern terminals (Ghostty, Kitty, WezTerm, Foot) now support GPU-class graphics protocols (Kitty graphics, Sixel) that none of the existing players use to anywhere near their potential.

clitunes is a TUI music player whose hero feature is a **visualiser engine** that targets Ghostty-class terminals and renders genuine GPU-quality visuals — spectrum analysers, oscilloscopes, particle fields, and Milkdrop-style geometric warping — by rendering off-screen with `wgpu` and streaming pixel buffers through Kitty graphics protocol. Music playback is the input signal that feeds the visuals, not the other way around.

A second, equally important framing decision: clitunes is **infrastructure**, not a single app. It runs as a daemon (audio sources, decoders, state bus) with renderer clients on top, so any pane — visualiser, now-playing strip, mini-spectrum, oscilloscope — can be launched as its own standalone process and embedded in tmux/wezterm/ghostty splits beside an editor, in a status line, or as a quick-terminal window. This is the differentiator from every existing TUI music player and is the architectural decision most expensive to retrofit, so it must be made on day one.

## Architecture (Conceptual)

```
                ┌────────────────────────────────────────┐
                │            RENDERER CLIENTS            │
                │   (subscribe to daemon over Unix sock) │
                │                                        │
                │  ┌──────────┐  ┌──────────┐  ┌──────┐  │
                │  │ default  │  │ --pane   │  │ ...  │  │
                │  │ tiled UI │  │ visualiser│ │      │  │
                │  └──────────┘  └──────────┘  └──────┘  │
                └────────────────────▲───────────────────┘
                                     │  state events
                                     │  + PCM tap
                                     │
                ┌────────────────────┴───────────────────┐
                │             clitunesd  (daemon)         │
                │                                        │
                │   ┌──────────────────────────────────┐ │
                │   │  state bus  +  PCM ring buffer   │ │
                │   └──────────────────────────────────┘ │
                │      ▲           ▲           ▲         │
                │   ┌──┴───┐   ┌───┴───┐   ┌───┴────┐    │
                │   │radio │   │ local │   │spotify │    │
                │   │source│   │ files │   │librespot│   │
                │   │      │   │       │   │ + sys  │    │
                │   └──────┘   └───────┘   └────────┘    │
                └────────────────────────────────────────┘
```

The daemon owns playback, the state bus, and the PCM ring buffer. Renderers subscribe over a Unix socket using a JSON or msgpack protocol. The default `clitunes` invocation auto-spawns the daemon if one isn't running and attaches a tiled-UI client; subsequent invocations reuse the existing daemon. `clitunes --pane <name>` launches a single-component renderer that subscribes to the same daemon. `clitunes status --json` is a one-shot query for scripting.

## Requirements

**Audio sources**

- R1. clitunes ships with **internet radio as the zero-config default source.** Running `clitunes` for the first time with no arguments and no prior config presents a **taste-neutral curated station picker** — 8–15 broadly varied stations spanning multiple genres, moods, and regions (e.g., one lo-fi/chill, one classical, one jazz, one electronic, one rock, one news/talk, one world music, one ambient, one indie, one classic) — with the visualiser already running on a calibration tone or test signal so the user sees the visuals immediately. The user picks one station; playback begins. **No hardcoded auto-play of a single station.** The user's first choice is persisted, so on every subsequent launch the last-played station auto-resumes within ~3 seconds. The curated picker is reachable any time via `:source radio` or a hotkey.
- R2. The radio source uses the **radio-browser.info** community directory (no API key required) to discover stations and supports browsing by genre, country, language, and popularity.
- R3. The radio source supports **Icecast and Shoutcast HTTP streams** (MP3 and AAC), `.pls` and `.m3u` station files, **HTTP redirects, ICY metadata parsing** for now-playing track info, and automatic reconnection on dropout. ICY metadata strings (`StreamTitle`, `StreamUrl`) **must be sanitized of ANSI escape sequences and C0/C1 control bytes before being rendered to the terminal**, since station operators control these strings and unsanitized rendering is a terminal control-code injection vector.
- R4. The local files source plays files passed as CLI arguments (`clitunes track.flac`) and recursively scans folders (`clitunes ~/Music`). Supported formats are whatever `symphonia` decodes natively: MP3, FLAC, OGG Vorbis, Opus, WAV, AAC. No persistent library index, no tag database, no SQLite.
- R5. The local files source reads ID3v2, Vorbis Comments, and MP4 tags via `lofty` (or equivalent) for the now-playing display. No tag editing.
- R6. The Spotify source supports **librespot full playback** for users with Spotify Premium. First-run authentication is via the standard Spotify Connect / OAuth device flow. Track metadata and album art are pulled from the Spotify Web API. **OAuth refresh tokens and any cached credentials must be stored in the OS keychain** (macOS Keychain, Linux Secret Service / libsecret, Windows Credential Manager) — never in plaintext TOML on disk. The config file may reference a keychain entry but must not contain the secret itself.
- R7. The Spotify source also supports **system audio capture** as a universal fallback for non-Premium users (and as a way to visualise other apps): BlackHole on macOS, PulseAudio loopback on Linux, WASAPI loopback on Windows. The user configures the OS-level audio routing once; clitunes documents the setup per platform.
- R8. The active source is switchable at runtime via `:source <name>` in the command palette and via standalone-pane CLI (`clitunes --pane visualiser --source radio --station bbc6music`).

**Visualiser engine**

- R9. clitunes ships **5 hand-tuned flagship visualisers** in v1, each individually polished to Ghostty-tier visual quality. Each visualiser is implemented against a shared `Visualiser` trait that takes a PCM buffer + FFT bins + a parameter struct, so the v2 plugin layer is mostly *exposing* what already exists, not rewriting it.
- R10. The five v1 flagships are:

| Name | Family | Brief |
|------|--------|-------|
| **Auralis** | GPU spectrum analyser | wgpu fragment shader rendering bars with bloom/glow, frequency-mapped color palette, beat-sync subtle camera response. The headline visualiser. |
| **Tideline** | Oscilloscope | Time-domain waveform with phosphor trails. Toggleable Lissajous (XY) mode for stereo correlation. |
| **Pulse** | Particle field | Particles emit on bass-band onsets, drift with broadband energy, fade with silence. Color responds to spectral centroid. |
| **Aether** | Geometric warp | Milkdrop-style tunneling geometry. Vertex displacement driven by spectrum, hue modulated by onset detection. |
| **Polaris** | Radial spectrum | Spectrum bars arranged in a circle around the album art (rendered via Kitty graphics) at the center. |

- R11. Visualisers render off-screen via `wgpu` to a framebuffer, then encode the framebuffer as PNG (or RGBA) and stream it to the terminal via the **Kitty graphics protocol**. Sixel is a fallback for terminals that don't speak Kitty graphics; pure-unicode rendering is *not* a goal in v1.
- R12. The active visualiser is switchable at runtime (`:vis <name>` or hotkey), persists across daemon restarts, and is configurable per-layout (so a `compact` layout can pin a specific small visualiser).
- R13. Each visualiser exposes a small set of named parameters (e.g., `bloom`, `palette`, `fft_smoothing`) that the user can tune live with `:set vis.<name>.<param> <value>` and persist to `~/.config/clitunes/config.toml`.
- R14. Frame budget target: **30 fps minimum, 60 fps where the terminal sustains it.** Audio↔visual sync must be sample-accurate (no IPC layer between PCM source and FFT input — both live inside the daemon).

**Process model and IPC**

- R15. clitunes is split into **`clitunesd` (the daemon)** and **`clitunes` (the client CLI)**. The daemon owns audio decoding, sources, the PCM ring buffer, and the state bus. Clients render and accept input.
- R16. Running `clitunes` with no daemon active **auto-spawns the daemon** as a child process and attaches the default tiled-UI client. The daemon stays resident as long as at least one client is attached, plus a configurable idle timeout after the last client disconnects.
- R17. The daemon exposes a Unix socket (default: `$XDG_RUNTIME_DIR/clitunes.sock`) speaking a versioned **JSON or msgpack** request/response + pub/sub protocol. The socket file is created with **mode 0600 (owner-only read/write)** and the daemon refuses connections whose peer UID does not match the daemon's own UID, since the daemon exposes media-key control verbs and a PCM tap that should not be reachable by other users on a multi-user host. The PCM ring buffer is exposed via shared memory (or a high-rate stream channel) for low-latency visualiser tap, with the same UID-gated access.
- R18. Any pane can be launched as its own standalone client process: `clitunes --pane visualiser`, `clitunes --pane mini-spectrum`, `clitunes --pane now-playing`, `clitunes --pane oscilloscope`. Each renders a single component and subscribes to the running daemon.
- R19. `clitunes status --json` performs a one-shot daemon query and prints current state (track, source, position, duration, bitrate, visualiser) for shell scripting and status-line use.
- R20. `clitunesctl` (or `clitunes <verb>`) supports headless control verbs without attaching a renderer: `play`, `pause`, `next`, `prev`, `source`, `vis`, `volume`. Suitable for binding to system media keys or custom keybindings.

**Layout and UI**

- R21. clitunes ships with a **tiled ricer layout as the default**, configurable via TOML in `~/.config/clitunes/config.toml`. Users can define multiple **named layouts** (e.g., `default`, `compact`, `minimal`, `pure`, `fullscreen`) and switch between them at runtime via `:layout <name>` or hotkeys.
- R22. Layout configuration is **declarative and recursive**: a layout is a tree of horizontal/vertical splits with ratio weights, leaves are component panes. Each leaf is one of the standard pane components (`visualiser`, `now-playing`, `source-browser`, `queue`, `mini-spectrum`, `command-bar`).
- R23. Layout responds to terminal resize. Each component declares a minimum-size requirement; a pane that no longer fits is hidden gracefully (with a fallback to the next available layout in a configured ladder).
- R24. The default layout is *opinionated and good*: a large visualiser pane (top-left), a context-sensitive source browser (top-right), and a now-playing strip with album art (bottom). The user is not required to write any TOML to get a great-looking screen on first launch. (Per **D11**, "opinionated and good" means *structurally* good — pane composition, sizing, spacing — not aesthetically prescriptive. Color palettes, fonts, glyph styles, and the default visualiser selection are all surfaced through the same curated-picker mechanism as R1, not hardcoded to the engineer's taste.)
- R25. Modal overlays (command palette `:`, search `/`, source switcher `s`, help `?`) are layout-independent and float above the active layout.

**Distribution and onboarding**

- R26. **First-run experience: zero friction, but never paternalistic.** A new user installs clitunes, runs `clitunes`, and within 3 seconds is presented with the curated taste-neutral station picker (R1) over a live visualiser running on a calibration signal. The user picks a station with arrow keys + enter — typically <10 seconds — and music begins. On every subsequent launch, the last-played station auto-resumes within ~3 seconds with no picker shown. **The product never assumes it knows what the user wants to hear**; it only assumes that *some* curated, broadly varied set of starting points beats the empty-room problem of "now type a URL." This is non-negotiable — both the zero-friction *and* the no-taste-imposition halves are load-bearing.
- R27. clitunes is distributed as a **single static binary** per platform (macOS arm64/x86_64, Linux x86_64/arm64, Windows x86_64). Optional package channels: Homebrew (macOS/Linux), `cargo install`, AUR, Nix flake. No required runtime dependencies beyond what the OS provides.
- R28. Configuration lives at `~/.config/clitunes/config.toml` (XDG-respecting). Defaults are baked into the binary; the config file is purely override.

## Success Criteria

- **Day-one wow:** A new user installs clitunes, runs it with no arguments, and within 3 seconds is hearing music and watching a visualiser whose visual quality is unmistakably beyond cliamp/cmus/ncmpcpp/ncspot. Screenshot-worthy on first launch with zero setup.
- **Daily driver test:** The clitunes engineer (user #1, a heavy Spotify user) uses clitunes as their primary music player for 30 consecutive days post-v1 without falling back to the official Spotify client or another player. If the dogfooding test fails, the product has not shipped.
- **Composability test:** A clitunes pane (`--pane mini-spectrum` or `--pane now-playing`) is embedded in a real tmux/wezterm/ghostty workspace alongside an editor and survives a normal coding session: terminal resize, daemon restart, source switching, suspend/resume.
- **Ricer test:** A user who already has a heavily customized terminal aesthetic (a "ricer") can reshape clitunes to match their setup using only TOML config — no source edits — and post a screenshot to r/unixporn that gets >100 upvotes. Adoption among the dotfile community is the canonical leading indicator of product-market fit for this audience.
- **Visualiser variety test:** All 5 v1 flagship visualisers feel like distinct experiences, not variations on a theme. A blind user shown rotating 10-second clips can tell which is which.

## Scope Boundaries

**Explicitly out of scope for v1:**

- **Tag editing** — clitunes reads tags for display, never writes them. Users edit tags in dedicated tools (mp3tag, beets, kid3).
- **Library indexing / SQLite** — there is no persistent music library database. Local files are sourced from CLI args and folder scans, period. Users who want a queryable library use `beets` and feed clitunes the resulting paths.
- **Tag-based search across local files** — implied by the no-library decision. `/` search inside local files is path-based only.
- **Lyrics** — synced or unsynced lyrics are a v1.1 feature, not v1. (Will use `lrclib.net` as a free unauthenticated source when added.)
- **Last.fm scrobbling** — deferred to v1.1 as a daemon plugin that subscribes to the state bus's track-changed event and POSTs to the Last.fm API. Estimated ~200 LOC once the daemon and state bus exist; may be promoted into v1 if the implementation cost stays trivial.
- **MPD client mode** — clitunes is a player and a daemon, not an MPD client. We do not connect to an MPD server.
- **Visualiser plugin DSL / preset authoring** — the v1 visualisers are hand-coded against the `Visualiser` trait. The plugin layer (probably WGSL shader presets + a small Lua/Rhai parameter layer) is a v2 feature. v1 commits *only* to the trait shape that makes v2 extraction feasible.
- **Pure Unicode-block / ANSI-only rendering** — clitunes targets Kitty graphics and Sixel. Users on Apple Terminal, basic xterm, or SSH-into-a-mainframe sessions are not the audience.
- **Spotify free-account playback** — librespot enforces Premium server-side. Free-account users get the system audio capture path or no Spotify integration at all.
- **iOS/Android remote control app** — the daemon's IPC protocol could in principle support this someday, but it is not a v1 commitment.
- **DRM-protected content** — Apple Music, Tidal MQA, etc. require licensed decoders that are out of scope.
- **Cross-fade, gapless, ReplayGain** — all desirable, all deferred to v1.1. v1 is point-to-point playback.

## Key Decisions

- **Visualiser engine first** *(D1)* — Rationale: clitunes wins by being unmistakably better at *one* thing than every existing player. Spreading across 5 feature pillars (player + Spotify + radio + scrobbling + visuals) produces a mediocre Spotify client AND a mediocre visualiser. Concentrating on visuals — the only axis on which TUI players are uniformly weak — produces a distinctive product.
- **Terminal aesthete audience** *(D2)* — Rationale: targeting "any terminal" caps the visual ceiling at unicode blocks, which kills the hero feature. Targeting Kitty/Ghostty/WezTerm users unlocks GPU-class rendering and is the same audience that already runs ricer setups and posts to r/unixporn, who are the natural early adopters.
- **Rust + ratatui + wgpu off-screen + Kitty graphics out** *(D3)* — Rationale: every brick we need (rustfft, cpal, symphonia, ratatui-image, wgpu, librespot) already exists in Rust as a mature crate. Iteration speed on visualisers, not language ceiling, is the binding constraint on visual quality. Zig was the runner-up but every hour spent on infrastructure in Zig is an hour not spent on visualisers.
- **Own the audio pipeline** *(D4)* — Rationale: visualisers need a sample-accurate, deterministic-latency tap on the PCM stream. Wrapping libmpv puts a process boundary between the music and the visuals and inherits mpv's buffer characteristics. HTTP/Icecast handling for radio is ~500–1000 LOC, not a library dependency.
- **5 hand-tuned flagships, plugin layer in v2** *(D5)* — Rationale: gives v1 something concretely impressive to demo while building toward the Milkdrop-style preset playground. Each flagship is architected against a shared trait so v2 extraction is mostly mechanical.
- **Three sources, universal visualiser, radio as zero-config default** *(D6)* — Rationale: radio solves the empty-room problem without auth or setup, demonstrating the hero feature on first launch. Local and Spotify are opt-in alternatives. The visualiser is universal — it doesn't care which source is feeding the buffer.
- **No taste imposition: curated picker, never a hardcoded default station** *(D11)* — Rationale: the engineer's musical taste is not the user's taste. Hardcoding "lo-fi hip hop" or "BBC 6 Music" or "SomaFM Groove Salad" as the default makes the product feel paternalistic and immediately alienates anyone who hates that genre. Instead, present 8–15 broadly varied curated stations across genres on first launch, persist the user's first pick, and auto-resume on every subsequent launch. The picker IS the zero-config experience. Applies to *every* default in clitunes that touches taste — visualiser default, color palette, layout — not just the radio station. This is a hard product principle, not a slice 1 detail.
- **Both librespot AND system audio capture for Spotify** *(D7)* — Rationale: clitunes user #1 lives in Spotify Premium and needs the clean integration; the broader audience needs a non-Premium fallback. The two paths share zero code in the source layer but converge on the same downstream pipeline, so the duplication is contained.
- **v1 = radio + local + Spotify + 5 visualisers** *(D8)* — Rationale: smaller v1s (radio-only, radio+local) fail the dogfooding test because the engineer doesn't use them daily. A v1 the engineer can't live in is a v1 that won't get the polish that makes the product good.
- **Tiled ricer with configurable TOML layouts** *(D9)* — Rationale: users in this audience already think in tiled layouts (Hyprland, i3, sway, tmux). Shipping a layout DSL instead of a hardcoded layout is the right ergonomic for them, and it costs roughly the same to implement.
- **Daemon + clients from day one** *(D10)* — Rationale: this is the architectural decision *most* expensive to retrofit. It unlocks standalone-pane embedding, external scripting, system media key bindings, and `clitunes status --json` for status lines. It is the strongest differentiator from every existing TUI music player and the architectural foundation that makes clitunes feel like infrastructure rather than an app.

## Dependencies / Assumptions

- **Spotify Premium confirmed for clitunes user #1.** Verified 2026-04-10: user #1 is a Premium subscriber and owner of a Spotify Family plan. librespot path is dogfoodable. Family ownership additionally implies multi-user household listening scenarios that may inform v1.1+ shared-session features (not committed for v1).
- **Kitty graphics protocol assumed available in the target terminal.** This is true for Ghostty, Kitty, WezTerm, and some others; Sixel is the fallback. Apple Terminal is explicitly unsupported.
- **macOS/Linux are the primary target platforms** for v1 development. Windows is supported in the build matrix but not the development OS — system audio capture on Windows uses WASAPI loopback and is the least-tested code path.
- **No claims have been verified against an existing clitunes codebase** because the working directory is empty greenfield. All "X exists in Rust" claims about crates (`symphonia`, `cpal`, `rustfft`, `ratatui-image`, `wgpu`, `librespot`, `lofty`, `reqwest`) are assumed based on common knowledge of the 2026 Rust ecosystem and **must be verified during `ce:plan`** before bead creation.

## Outstanding Questions

### Resolve Before Planning

*All blocking questions resolved as of 2026-04-10. Brainstorm is ready to hand off to `/ce:plan`.*

### Deferred to Planning (added)

- **[Affects R1, R26][User decision, low-stakes]** Final composition of the 8–15 station curated picker shipped on first launch. Should span genres (lo-fi, classical, jazz, electronic, rock, indie, world, ambient, news/talk) and use long-running, license-friendly stations (SomaFM family, public broadcasters with permissive streams, established community Icecast stations). The exact list is a curation exercise that can happen during slice 1 polish, not a brainstorm decision. Curation must explicitly avoid loading the picker with stations that reflect any single dominant genre or developer preference.

### Deferred to Planning

- **[Affects R10][Technical]** Final palette/parameter design for each of the 5 flagship visualisers — exact bloom radii, color ramps, FFT smoothing constants, particle counts, etc. Belongs in the visualiser slice's polish phase, not in product requirements.
- **[Affects R11][Needs research]** Concrete benchmark of `wgpu`-rendered framebuffer → PNG → Kitty graphics protocol throughput on Ghostty, Kitty, and WezTerm at 60 fps for 240×80 character cell grids. If sustained 60 fps is impossible on common hardware, the frame budget target in R14 must be revised.
- **[Affects R15–R20][Technical]** Exact IPC protocol design (msgpack vs JSON vs Cap'n Proto), exact PCM ring buffer transport (shared memory vs unix-socket stream), daemon-client versioning strategy, daemon lifecycle (auto-reap vs explicit shutdown), socket location on Windows.
- **[Affects R22][Technical]** Exact TOML schema for declarative layout trees. Multiple Rust crates (`taffy`, `ratatui`'s built-in layout, custom) could underlie this — pick during `ce:plan`.
- **[Affects R6][Needs research]** Current state of `librespot` crate maintenance and protocol stability as of April 2026. If the upstream is unmaintained or broken, slice 2 has to either fix librespot or pivot to system-capture-only.
- **[Affects R7][Needs research]** Per-OS system audio capture setup documentation: BlackHole installation flow on macOS, PulseAudio loopback module activation on Linux, WASAPI loopback enumeration on Windows.
- **[Affects R10, R12][Technical]** Naming the visualisers "Auralis / Tideline / Pulse / Aether / Polaris" is a placeholder; final naming and branding can shift during ce:plan or implementation.
- **[Affects R29 — future][Technical]** Lyrics source for v1.1: `lrclib.net` is the obvious free unauthenticated choice. Verify API stability before committing.

## Slice 1 (Smallest Demo-able PR)

The brainstorm produced enough clarity to define slice 1 concretely. This is captured here for `beads-workflow` to consume directly:

> **Slice 1: "First pixels, first choice."**
>
> A new user runs `clitunes` with no arguments on a Ghostty/Kitty/WezTerm terminal. Within 3 seconds, the **Auralis** spectrum visualiser is filling the terminal at 30+ fps via Kitty graphics protocol — initially driven by a calibration tone or test signal so the user sees the visuals immediately, with no audio waiting on a network round-trip. Overlaid on the visualiser is a **curated taste-neutral station picker** showing 8–15 broadly varied stations across genres. The user picks one with arrow keys + enter; the calibration signal hands off to the live stream within ~2 seconds and the visualiser is now reactive to real audio. Pressing `q` quits cleanly. The choice is persisted to disk so the *second* run skips the picker and auto-resumes the last station within ~3 seconds with the visualiser already running.
>
> What slice 1 is **not**: no layout config, no source switching to local/Spotify, no other panes, no auth, no command palette beyond the picker, no daemon-client split *yet* (slice 1 may use a trivial degenerate split where the daemon runs in-process), no settings UI.
>
> This slice exercises the entire critical path — HTTP stream → symphonia decode → cpal output → rustfft tap → wgpu render → Kitty graphics encode → terminal — *and* establishes the no-taste-imposition product principle from line one. It produces the screenshot that validates the product's identity.

Subsequent slices add: daemon-client split (slice 2), the other 4 visualisers (slice 3), local file source (slice 4), Spotify librespot (slice 5), Spotify system capture (slice 6), tiled layout engine (slice 7), standalone `--pane` clients (slice 8), config file & onboarding polish (slice 9). Last.fm becomes slice 10 / v1.1.

## Review Findings Carried into Planning

The 2026-04-10 document review surfaced 9 findings that the user elected to carry into `/ce:plan` rather than resolve in the brainstorm. `/ce:plan` must address each one explicitly — through scope decisions, technical spikes, or revised success criteria — before bead creation.

**P0 — load-bearing tensions:**

- **PF1. Scope ambition vs visualiser-first identity** — D1 ("concentrate on visuals") and D8 ("v1 = radio + local + librespot + system-capture + daemon + 5 visualisers") are in tension. `/ce:plan` must pick a coherent v1: either visualiser-first (cut sources) or dogfood-first (cut visuals), not both. Document the choice as an updated decision and rewrite D1/D8 to match.
- **PF2. Daemon-day-1 contradicts slice-1's degenerate split** — D10 says daemon is the most expensive thing to retrofit; slice 1 retrofits it. `/ce:plan` must either pull a real (not degenerate) daemon-client split into slice 1, or relax D10 with an explicit retrofit plan. The current "in-process for slice 1, real for slice 2" position is exactly the trap D10 names.
- **PF3. librespot fragility is the largest hidden v1 dependency** — librespot's protocol breaks periodically as Spotify works against it. `/ce:plan` must include a librespot spike (verify current crate health, run `librespot` against a Premium account on macOS arm64, confirm playback latency and metadata callbacks) **before** committing slice 5 to it. If the spike fails, slice 5 must pivot to system-capture-only and the dogfooding test must be revised.
- **PF4. Visual pipeline throughput is unverified and the hero feature depends on it** — `wgpu` → framebuffer → PNG encode → Kitty graphics protocol at 60 fps over a 240×80 cell grid is asserted, not measured. PNG encode at 60 fps is suspect; `ratatui-image` is designed for static images. `/ce:plan` must include a benchmark spike (Ghostty/Kitty/WezTerm × macOS/Linux × common cell grids) **before** any visualiser bead is created. R14's "30 fps minimum, 60 fps where sustained" is provisional until measured. If the spike shows the throughput target is impossible, R11/R14 and the entire visualiser architecture must be revised — possibly switching to a streaming-friendly format (raw RGBA chunks, JPEG, or zlib-compressed framebuffers) or accepting a lower frame budget.
- **PF5. Dogfooding test is structurally unwinnable as written** — User #1 is a Spotify Premium **Family** plan owner. The official Spotify client owns Discover Weekly, Daylist, Spotify Connect handoff, podcasts, Jam, lyrics, library sync, and Family account switching. None of these are in v1 scope. "Replace the official Spotify client for 30 days" is impossible regardless of how good clitunes is. `/ce:plan` must rewrite the dogfooding criterion as something honest and measurable (e.g., "user #1 leaves clitunes running on a second monitor or as a primary visualiser surface for 30 consecutive days" or "user #1 uses clitunes as the primary listening surface for radio + local files and uses official Spotify only for Connect/discovery").

**P1 — measurable proxies and over-scoped slice:**

- **PF6. r/unixporn upvotes is a vanity metric** — measures screenshot virality on a single subreddit, not retention. `/ce:plan` should replace the Ricer Test in Success Criteria with a real adoption signal (active installs, GitHub stars trajectory, dotfile-repo references, or simply "≥3 unrelated users post screenshots without being asked").
- **PF7. Slice 1 contains ~14 subsystems and is not the smallest demo-able PR** — HTTP stream + symphonia + cpal + rustfft + wgpu + PNG encode + Kitty graphics + station picker + persistence + calibration tone + Auralis shaders + reconnect + ICY parsing + clean shutdown is not "smallest." `/ce:plan` should subdivide slice 1 into at least two beads-shippable units. A more honest slice 1 might be: **calibration tone source → wgpu → Kitty graphics, alone**, with the audio path landing in slice 1.5. The station picker, persistence, and reconnect logic come later.
- **PF8. The 5-visualiser slate is brainstormed, not validated** — Auralis/Tideline/Pulse/Aether/Polaris were generated during the brainstorm without playable evidence. `/ce:plan` should commit firmly to **only Auralis** for v1 and treat the other 4 as candidates that earn their slots by demoing well in slice 3, not by appearing in a table. R9 should be revised from "5 hand-tuned flagships" to "1 hand-tuned flagship + N candidates that ship if they hit the visual bar."
- **PF9. "5 distinct visualisers, blind users can tell them apart" is unmeasurable** — no panel, no sample, no threshold. `/ce:plan` should either define a real test (n=5 unrelated terminal users, 10s clips, ≥80% correct identification) or drop the criterion.

These findings are *planning inputs*, not brainstorm regressions. Each one is a concrete question `/ce:plan` must answer with technical context that the brainstorm cannot supply.

## Next Steps

`-> /ce:plan` for structured implementation planning. All blocking product decisions are resolved; the 9 findings above (PF1–PF9) are carried forward as explicit planning inputs that must be addressed during the planning phase before bead creation.
