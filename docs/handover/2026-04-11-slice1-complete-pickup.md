---
title: clitunes v1 handover — slice 1 complete, slice 2 next
date: 2026-04-11
status: active
author: claude (autonomous session)
---

# clitunes v1 handover

Slice 1 is shipped and runs end-to-end on M1 Max / Metal. This doc is the
single pickup point for the next session: what's done, what's pending, the
commands to re-verify, and the landmines lurking in slices 2–5.

## TL;DR

- `cargo run -p clitunes` boots `clitunes` → calibration tone → PCM ring →
  realfft tap → Auralis wgpu pipeline → Kitty graphics protocol to stdout.
- M1 Max render+readback: 144 frames in 2.6s (~55fps) at 1024×512 on Metal.
- 7/7 unit tests pass. 7/7 e2e assertions pass. `cargo clippy -- -D warnings`
  is clean on all four slice-1 crates.
- D15 is enforced: `cargo tree -e features -p clitunesd` contains zero
  `wgpu`, `ratatui`, or `crossterm`.
- Next: Slice 2 — radio-browser.info source, ICY metadata parser, curated
  12-station picker, state.toml persistence.

## Repository layout

```
clitunes/
├── Cargo.toml                 # workspace: 5 members
├── deny.toml                  # cargo-deny: license + advisory policy
├── .github/workflows/
│   ├── ci.yml                 # fmt, clippy, test matrix, cargo-deny, D15 grep
│   └── e2e.yml                # macos-latest e2e harness
├── crates/
│   ├── clitunes-core/         # pure types: PcmFormat, StereoFrame, Station,
│   │                          #   State, VisualiserId, SurfaceKind
│   ├── clitunes-engine/       # feature-gated: audio | sources | visualiser |
│   │                          #   tui | layout | control
│   ├── clitunes/              # binary: full viz app (features: audio,
│   │                          #   sources, visualiser, control)
│   ├── clitunesd/             # daemon binary (features: audio, sources,
│   │                          #   control ONLY — D15 boundary)
│   └── clitunes-spike/        # Phase 0 throughput spike (archived)
├── docs/
│   ├── plans/2026-04-11-001-feat-clitunes-v1-implementation-plan.md
│   ├── brainstorms/…          # requirements docs
│   ├── spikes/2026-04-11-wgpu-kitty-throughput-spike.md  # PASS record
│   ├── conventions/logging.md # tracing/logging convention
│   └── handover/              # ← this file
├── tests/e2e/
│   ├── run.sh                 # entry point: runs every scenarios/*.sh
│   ├── lib/assertions.sh      # assert_* helpers (bash 3.2-compatible)
│   └── scenarios/01_slice1_calibration.sh
└── .beads/                    # bead store (use `br` / `bv --robot-*`)
```

## Slice 1 — what shipped

### Units closed

| Bead            | Unit | Title                                                           |
|-----------------|------|-----------------------------------------------------------------|
| clitunes-xhh    | 1    | wgpu→Kitty throughput spike                                     |
| clitunes-izq    | 2    | Workspace + 3-crate scaffolding + CI baseline + cargo-deny      |
| clitunes-9v9    | 3    | Calibration tone source + realfft tap + bounded ring            |
| clitunes-0yi    | 4    | Visualiser trait + wgpu pipeline + Kitty writer + Auralis       |
| clitunes-xb2    | —    | Cross-cutting: tracing/logging infrastructure + e2e harness     |
| clitunes-xro    | —    | (Phase 0 support bead)                                          |
| clitunes-ph0    | epic | Phase 1 — Slice 1: Skeleton + Auralis on calibration tone       |

### Key files

- `crates/clitunes-core/src/{pcm,station,state,visualiser}.rs` — pure types
  with no I/O. Shared between clitunes, clitunesd, and future clients.
- `crates/clitunes-engine/src/observability.rs` — `init_tracing(component)`
  with `CLITUNES_LOG_FORMAT=json` toggle; named span constants in
  `observability::spans`.
- `crates/clitunes-engine/src/audio/ring.rs` — `PcmRing` (SPSC, drop-oldest
  on overrun, `parking_lot::Mutex<VecDeque>`). **This is a slice-1 placeholder;
  Unit 11 replaces it with a cross-process SPMC ring.**
- `crates/clitunes-engine/src/audio/tone.rs` — `CalibrationTone`: breathing
  envelope × (sine + fifth × 0.35 + triad × 0.12). A3 base, deliberately
  unmusical so it's visually distinct from real music during e2e tests.
- `crates/clitunes-engine/src/audio/fft_tap.rs` — `FftTap::new(fft_size)` with
  Hann window, realfft real→complex. `snapshot(reader, sample_rate)` for ring
  consumers; `snapshot_from(frames, sample_rate)` for tests.
- `crates/clitunes-engine/src/sources/tone_source.rs` — `ToneSource`
  implements `Source::run(writer, stop)`; sleeps 0.8 × block_dur to avoid
  underrun.
- `crates/clitunes-engine/src/visualiser/wgpu_runtime.rs` — `WgpuRuntime`
  with off-screen RGBA8Unorm target texture, 256-byte-aligned staging buffer,
  `readback()` returning `FrameReadout { render_time, readback_time, pixels }`.
  Uses `wgpu::PollType::wait_indefinitely()` (wgpu 29 API).
- `crates/clitunes-engine/src/visualiser/kitty_writer.rs` — `KittyWriter` with
  4096-byte APC chunks, format `a=T,f=24,s=W,v=H,i=1,q=2,m={0|1}`, image id
  reuse for in-place updates. `cursor_home()` before each frame.
- `crates/clitunes-engine/src/visualiser/auralis.rs` + `auralis.wgsl` — 64
  bars packed as `array<vec4<f32>, 16>` (WGSL uniform packing rule), HSV
  warm→cool gradient, attack 0.6/0.4 release 0.85/0.15 smoothing, top-edge
  glow. Clears to `(0.015, 0.010, 0.025, 1.0)`.
- `crates/clitunes/src/main.rs` — slice-1 driver: observability, SIGINT
  handler via raw libc, ToneSource on a named thread, main render loop with
  ~16ms pacing.
- `crates/clitunesd/src/main.rs` — stub daemon; just inits tracing and logs
  a boot line. D15 boundary.

### Feature gate wiring

```toml
# clitunes-engine/Cargo.toml
[features]
default   = ["audio", "sources", "visualiser", "tui", "control", "layout"]
audio     = ["dep:cpal", "dep:realfft"]
sources   = ["audio"]
control   = []
visualiser = ["audio", "dep:wgpu", "dep:pollster", "dep:bytemuck", "dep:base64"]
tui       = ["dep:ratatui", "dep:crossterm"]
layout    = ["tui"]
```

- `clitunes` crate declares `clitunes-engine` with
  `default-features = false, features = ["audio","sources","visualiser","control"]`.
- `clitunesd` crate declares `clitunes-engine` with
  `default-features = false, features = ["audio","sources","control"]`.
- CI job `d15-invariant` greps `cargo tree -e features -p clitunesd` for
  `wgpu|ratatui|crossterm` and fails the build if any match.

### Spike results (pre-committed thresholds)

- 1024×512 unpaced: p99=3.11ms, p95=2.95ms, max=4.21ms, effective 362fps.
- 1920×1080 unpaced: p99=6.37ms, p95=5.43ms, effective 187fps.
- 1920×1080 paced 60fps: p99=7.82ms, p95=7.32ms — **PASS** (threshold: p99≤16).
- Default render texture: **1024×512** (5× headroom).
- Single synchronous staging buffer is sufficient; no ping-pong needed.
- Full writeup: `docs/spikes/2026-04-11-wgpu-kitty-throughput-spike.md`.
- **Open**: only 1 platform validated (Metal/M1 Max). Plan requires 2-of-4.
  Unit 19 re-measures on Linux (Vulkan) + Windows (DX12) before v1 ship.

## How to re-verify slice 1 in 30 seconds

```bash
cd /Users/coin/Desktop/DesktopFolders/clitunes

# 1. build
cargo build -p clitunes -p clitunesd

# 2. unit tests
cargo test -p clitunes-core -p clitunes-engine

# 3. clippy
cargo clippy -p clitunes -p clitunes-core -p clitunes-engine -p clitunesd \
    --all-targets -- -D warnings

# 4. e2e harness
bash tests/e2e/run.sh

# 5. D15 invariant
cargo tree -e features -p clitunesd | grep -E 'wgpu|ratatui|crossterm' && \
    echo "D15 VIOLATED" || echo "D15 ok"

# 6. visual smoke test (run in a Kitty or Ghostty terminal)
cargo run --release -p clitunes
# ^C to exit
```

## Outstanding slice-1 housekeeping

These are non-blocking but worth doing before starting slice 2:

- [ ] **init git repo and push first commit.** Current directory is
      greenfield with no `.git/`. CI won't run until the repo exists.
      ```bash
      cd /Users/coin/Desktop/DesktopFolders/clitunes
      git init && git branch -m main
      git add . && git commit -m "slice-1: wgpu→Kitty Auralis on calibration tone"
      # then `gh repo create` or push to existing remote
      ```
- [ ] **Run the visual test in a real Kitty terminal** and confirm Auralis
      looks right. The spike and e2e harness only verify byte-level output;
      a human needs to eyeball the bars+gradient once. If tmux is in the
      middle, add `set -g allow-passthrough on` to `~/.tmux.conf`.
- [ ] **Install cargo-deny locally** (`cargo install cargo-deny`) and run
      `cargo deny check` to shake out any license surprises before CI
      catches them.
- [ ] **Run `bv --robot-insights | jq .bottlenecks`** to sanity-check the
      bead graph before diving into slice 2.

## Slice 2 — what's next (ordered)

Slice 2 adds real audio. Goal: `clitunes` boots, shows the 12-station picker,
plays an internet radio station, and updates Auralis from the live PCM stream.

### Unit 5 — radio-browser.info client (clitunes-585)

- DNS SRV lookup for `_api._tcp.radio-browser.info` → healthy mirror.
- Fall back to round-robin through the documented mirror list if SRV fails.
- Per-call retry with a different mirror on any 5xx / network error.
- Returns `clitunes_core::Station` instances.
- Module location: `crates/clitunes-engine/src/sources/radio/client.rs`.
- Dep: `hickory-resolver` (or std + `trust-dns`) + `reqwest` with
  `default-features = false, features = ["rustls-tls","stream"]`.
- `visualiser` feature must NOT pull reqwest. Add `radio` feature and gate
  cleanly.

### Unit 6 — ICY metadata parser (clitunes-cka)

- Parse Icy-MetaInt header + interleaved metadata blocks from the stream.
- Emit `NowPlaying { artist, title, station_uuid }` events on a tokio
  broadcast channel (or `crossbeam_channel` since we're not async yet).
- Aggressive sanitisation: strip HTML, CR/LF, control chars, emoji-bombs.
- Unit-test against fixtures in `tests/fixtures/icy/`.

### Unit 7 — Symphonia decoder (clitunes-ve8)

- Wire `symphonia` with MP3/AAC/OGG feature flags.
- Feed decoded f32 stereo frames into the `PcmRingWriter`.
- Must handle mid-stream format changes (some ICY streams do this).

### Unit 8 — Curated 12-station picker (clitunes-rug)

- **Memory pin from `feedback_no_taste_imposition`**: never hardcode a
  "default" — always present the curated picker on first run.
- 12 `CuratedStation` entries in `crates/clitunes-core/src/station.rs`
  (already stubbed as the `CuratedStation` struct).
- `state.toml` persistence: `last_station_uuid`, `picker_seen`,
  `last_visualiser`, `last_layout`. `State` type already exists.
- ratatui picker overlay. **This is the first thing that introduces the
  ratatui dep into `clitunes`** — verify D15 still holds after adding it.

### Unit 9 / 10 — clitunesd lifecycle + control bus (clitunes-w48 / clitunes-7nu)

- Deferred until Slice 3. Slice 2 keeps everything in-process.

### Bead polish before starting

Before running `bv --robot-next`, do a quick polish pass on Units 5–8:

```
Reread AGENTS.md so it's still fresh in your mind. Check over clitunes-585,
clitunes-cka, clitunes-ve8, clitunes-rug super carefully — are you sure they
make sense? Is anything ambiguous? Include comprehensive unit tests and e2e
scenarios. Use only `br` to modify beads. Use ultrathink.
```

## Known landmines

- **wgpu 29 API is different from every tutorial.** When in doubt, read
  `~/.cargo/registry/src/.../wgpu-29.0.1/` and
  `wgpu-types-29.0.1/src/lib.rs` directly. Key gotchas:
  - `InstanceDescriptor` is struct-init (no `Default`), has `display: None`.
  - `bind_group_layouts: &[Some(&layout)]` — Option-wrapped.
  - `RenderPipelineDescriptor` uses `multiview_mask`, not `multiview`.
  - `RenderPassDescriptor` also has `multiview_mask: None`.
  - `device.poll(PollType::wait_indefinitely())` — convenience constructor.
- **Mac's default bash is 3.2.** No `mapfile`, no `${var,,}`. The e2e
  harness uses `while IFS= read` instead. When writing new scenarios,
  keep this in mind.
- **Claude Code's shell has no controlling terminal.** `./clitunes
  --output tty` fails with `Device not configured (os error 6)`. Pipe to
  a file instead (`./target/release/clitunes > /tmp/out.bin 2> /tmp/err.log`).
- **Signal handler uses raw libc.** No signal-hook dep in slice 1 to keep the
  tree slim. It works but `sa.sa_sigaction = handler as *const () as usize`
  is the only portable way across clippy's `function_casts_as_integer` lint.
- **`cpal` and `realfft` are now optional** behind the `audio` feature.
  Earlier iterations had them unconditional; don't accidentally revert that
  when adding `radio` — it would break the daemon D15 cascade through
  visualiser's `audio` dependency chain.
- **D15 is structural, not advisory.** `clitunesd` lives in its own crate
  (not just a `[[bin]]` entry) because cargo features are package-scoped;
  two binaries in one crate share the feature set. If someone "simplifies"
  by merging them back together, D15 silently dies.
- **Kitty image id is hardcoded to 1** (`KittyWriter::new` sets `image_id:
  1`). This is correct for in-place frame updates. Don't bump it per frame
  or Kitty will OOM the terminal. Unit 16 (standalone pane clients) will
  need per-pane image id allocation.
- **Texture size is a magic number in `clitunes/src/main.rs`**
  (`WIDTH=1024, HEIGHT=512`). Unit 15 (layout DSL) makes this dynamic.
  Don't add a config file for it earlier — YAGNI.
- **Render loop uses `thread::sleep` pacing.** Fine for slice 1, but
  Unit 19 (first-run UX + time-to-first-pixel) may want a proper
  monotonic-clock pacer.

## Phases 2–5 at a glance

| Phase   | Slice | Units              | Theme                                                  |
|---------|-------|--------------------|--------------------------------------------------------|
| Phase 1 | 1     | 1–4 + xb2          | **Done.** Skeleton + Auralis on calibration tone.      |
| Phase 2 | 2     | 5–8                | Radio source + ICY + curated picker + state.toml.      |
| Phase 3 | 3     | 9–13               | Daemon split, control bus, cross-process SPMC ring.    |
| Phase 4 | 4     | 14–18              | Local files, layout DSL, pane clients, viz fleet.      |
| Phase 5 | 5     | 19–20              | First-run polish, distribution, v1.0.0 tag.            |

Beads for every phase are in place (28 originals + 1 xb2 cross-cutting).
`br ready` shows what's actionable; `bv --robot-next` picks the top item.

## Bead workflow for next session

```bash
# See what's ready to start
br ready

# Or let bv pick
bv --robot-next

# Start a unit
br update clitunes-585 --status in_progress

# Close when done (include concrete evidence in --reason)
br close clitunes-585 --reason "radio-browser client done: SRV → mirror with fallback, per-call retry, 5 unit tests against httpbin fixtures"

# Flush + commit beads
br sync --flush-only
git add .beads && git commit -m "beads: progress on slice 2"
```

## Session log (2026-04-11, autonomous)

- Phase 0 spike built + validated on M1 Max Metal (above thresholds).
- Spike doc written: `docs/spikes/2026-04-11-wgpu-kitty-throughput-spike.md`.
- 4-crate workspace bootstrapped. clitunesd split into its own crate for
  structural D15.
- `clitunes-core` types, `clitunes-engine` observability + audio + sources
  + visualiser modules written.
- `clitunes` binary main.rs integrates the whole pipeline.
- 7/7 unit tests pass; 7/7 e2e assertions pass; clippy clean.
- cargo-deny config (`deny.toml`), CI workflow (`.github/workflows/ci.yml`),
  e2e workflow (`.github/workflows/e2e.yml`) added.
- Logging convention doc (`docs/conventions/logging.md`) written.
- All slice-1 beads closed; Phase 1 epic closed.
- **No git repo yet** — the directory is still unversioned. Initialize
  before starting slice 2 (see housekeeping section).

## Open questions for the operator

1. **GitHub remote** — where should this push? `gh repo create coin/clitunes
   --public --source=.`? Confirm visibility and org before pushing.
2. **First-run terminal for visual smoke** — Kitty? Ghostty? Both? The e2e
   harness only asserts byte-level Kitty APC headers; the human-in-the-loop
   verification is deferred.
3. **Cross-platform validation of the Phase 0 spike** — the plan requires
   2-of-4 platforms. Do we park this on a CI runner (macos-latest is
   already there; add ubuntu-latest with Vulkan SwiftShader?) or wait for
   Unit 19 which was already going to re-measure?
4. **Slack / Linear** — nothing is wired to any external tracker yet. Should
   it be?

---

*Slice 1 is load-bearing: every subsequent slice builds on the trait
shape, feature-gate layout, tracing convention, and e2e harness established
here. Tread carefully when altering those pieces.*
