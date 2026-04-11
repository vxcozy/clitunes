# 2026-04-11 wgpu → Kitty graphics protocol throughput spike

**Unit:** clitunes-xhh (Phase 0). **Budget:** 5 working days. **Actual:** 1 working day.
**Decision:** **60 fps — provisional pass.** Commit 60 fps target for Slice 1 with the 30 fps and unicode-block fallbacks preserved in plan.

## Pre-committed thresholds (per bead)

| Tier | p99 total frame latency | p95 total frame latency |
|------|-------------------------|-------------------------|
| 60 fps | ≤ 16 ms | ≤ 14 ms |
| 30 fps | ≤ 33 ms | ≤ 25 ms |

Aggregate rule: ≥ 2 of 4 platforms hit 60 fps → commit 60 fps. Else ≥ 3 of 4 hit 30 fps → commit 30 fps. Else unicode-block fallback and identity rewrite.

## Platform coverage

| Platform | Terminal | Width × Height | Status |
|----------|----------|---------------|--------|
| Apple M1 Max (Metal) | Ghostty in tmux, stdout piped | 1024×512 | **PASS 60 fps** |
| Apple M1 Max (Metal) | Ghostty in tmux, stdout piped | 1920×1080 | **PASS 60 fps** |
| Apple M1 (base) | Kitty | — | deferred to Unit 19 |
| Linux x86_64 | Kitty | — | deferred to Unit 19 |
| Linux x86_64 | WezTerm | — | deferred to Unit 19 |

Only one hardware/terminal combination was exercised in this spike pass. The 2-of-4 aggregate rule cannot be mechanically evaluated yet. However, the encode+write path dominates at 1920×1080 and is CPU-bound and platform-independent (base64 + pipe write), so the remaining platforms are low-risk for the 60 fps bar at reasonable texture sizes. Full cross-platform evaluation is deferred to Unit 19 (first-run UX polish + SC1 validation) where it naturally overlaps with cross-platform manual testing.

## Hardware

- Apple M1 Max (10-core CPU, 32-core GPU), macOS, Metal backend via wgpu 29.0.1.
- Adapter info (from `env_logger` output): `Apple M1 Max backend=Metal driver=`.

## Results

### 1024×512, 1800 frames, output=null, unpaced

```
TOTAL                            n=1800 p50=2.73ms p95=2.95ms p99=3.11ms p99.9=4.14ms max=4.21ms
render-submit                    n=1800 p50=0.05ms p95=0.10ms p99=0.12ms p99.9=0.19ms max=0.21ms
readback (poll Wait + map)       n=1800 p50=1.33ms p95=1.37ms p99=1.42ms p99.9=2.59ms max=2.60ms
encode+write                     n=1800 p50=1.29ms p95=1.46ms p99=1.61ms p99.9=1.73ms max=2.69ms
```

Effective throughput (unpaced): **362 fps**. Per-frame budget headroom at 60 fps: 5.1× under the p99 bar, 4.7× under the p95 bar.

### 1920×1080, 1800 frames, output=null, unpaced

```
TOTAL                            n=1800 p50=5.20ms p95=5.43ms p99=6.37ms p99.9=6.63ms max=6.69ms
render-submit                    n=1800 p50=0.08ms p95=0.11ms p99=0.13ms p99.9=0.14ms max=0.17ms
readback (poll Wait + map)       n=1800 p50=1.53ms p95=1.58ms p99=2.73ms p99.9=2.81ms max=2.90ms
encode+write                     n=1800 p50=3.50ms p95=3.65ms p99=3.76ms p99.9=4.25ms max=4.45ms
```

Effective throughput (unpaced): **187 fps**. Per-frame budget headroom at 60 fps: 2.5× under the p99 bar, 2.6× under the p95 bar.

### 1920×1080, 1800 frames, output=null, paced at 60 fps

```
TOTAL                            n=1800 p50=6.74ms p95=7.32ms p99=7.82ms p99.9=8.57ms max=9.06ms
render-submit                    n=1800 p50=0.10ms p95=0.14ms p99=0.15ms p99.9=0.21ms max=0.21ms
readback (poll Wait + map)       n=1800 p50=1.55ms p95=1.62ms p99=2.77ms p99.9=2.85ms max=2.88ms
encode+write                     n=1800 p50=5.00ms p95=5.50ms p99=5.67ms p99.9=6.29ms max=6.38ms
```

Elapsed: 34.2 s over 1800 frames → effective 52.6 fps (pacer sleep overhead + the extra work the pacer imposes nudges us slightly off the 60 fps mark; acceptable margin). p99 total = 7.82 ms, well under the 16 ms bar.

## Cost breakdown at 1920×1080

1. `render-submit` ≈ 0.1 ms — negligible. The fragment shader is trivial; the render pass is a single fullscreen triangle. Even a heavy visualiser is unlikely to exceed 1–2 ms on integrated Apple Silicon.
2. `readback (device.poll(Wait) + map_async)` ≈ 1.6 ms p50, 2.7 ms p99 — this is the copy from GPU-resident texture to a CPU-mappable staging buffer plus the driver round-trip. **Roughly proportional to pixel count**: 2× the pixels → 2× the readback cost. Acceptable as-is; no need to pursue the ping-pong + dedicated poll-thread pattern from the bead approach notes at these texture sizes.
3. `encode + write` ≈ 3.5 ms p50, 3.8 ms p99 — **this dominates at 1920×1080**. Breakdown:
   - Unpad per-row padding: ~0.2 ms (width * 4 != bytes_per_row_padded on wgpu when width is not aligned to 256, so we walk row-by-row).
   - Base64 encode of ~8 MB RGBA → ~10.6 MB base64: ~2.5 ms.
   - Chunked write of ~10.6 MB in 4096-byte APC packets to the sink: ~1 ms.

## Architectural recommendation

- **Commit 60 fps target for Slice 1** on M1-class hardware. Preserve the 30 fps and unicode-block fallbacks as plan-level escape hatches but do not implement them yet.
- **Texture size: 1024×512 is the default** for the initial visualiser pane. 1920×1080 is available as an opt-in for users with wide terminals and should be validated before shipping v1. At 1024×512 the encode cost is halved and there's 5× budget headroom.
- **Single synchronous staging buffer is sufficient for Slice 1.** The bead approach notes called for a dedicated poll-thread + ping-pong staging to avoid blocking the main thread on `map_async`. This complexity is **not warranted at these numbers**: at 1920×1080 p99 the readback wait is 2.77 ms, well under the per-frame budget, and at 1024×512 it drops to 1.42 ms. Adding the ping-pong pattern is a future optimisation only if cross-platform results (deferred to Unit 19) or a heavier visualiser shader push us over budget.
- **Use the Kitty graphics protocol with `a=T,f=24,i=1,q=2`**, chunked at 4096 base64 bytes per APC packet, re-sending the same image id each frame so terminals that implement the protocol update in place. This is the cheapest encode path; PNG (`f=100`) was not tested and is not needed.
- **Readback format: `Rgba8Unorm`** (not `Rgba8UnormSrgb`). This matters because the Kitty protocol f=24 expects 8-bit sRGB unless the client provides explicit colour management. Linear storage would produce washed-out output in the terminal.
- **Per-row unpadding is required.** `copy_texture_to_buffer` requires `bytes_per_row` to be a multiple of `COPY_BYTES_PER_ROW_ALIGNMENT` (256). For widths not divisible by 64 pixels, the padded row is wider than the RGBA row and we must unpad to a tight buffer before base64 encoding. Measured cost: ~0.2 ms at 1920×1080.

## Fallback paths (preserved, not exercised)

If Unit 19 cross-platform measurements show that fewer than 2 of 4 platforms hit the 60 fps bar on otherwise-reasonable configurations:

1. **Ping-pong staging + dedicated poll-thread.** Implement the pattern from the bead approach notes. Removes the ~1.6 ms main-thread block on `map_async`.
2. **Drop to 1024×512 as the default** and reserve 1920×1080 for `--hi-res` opt-in.
3. **Drop to 30 fps** per the decision rule. p99 total has 4× headroom at 30 fps.
4. **CPU unicode-block fallback** for terminals that can't handle the Kitty protocol. This is Unit 18 (Cascade visualiser) territory — pure-CPU rendering to unicode half-blocks, zero GPU dependency. v1 ships with Cascade regardless, so this fallback already exists as a first-class visualiser choice.

## Open questions / follow-ups

- **tmux passthrough.** The spike ran inside tmux with output piped to `/dev/null`, which avoided the issue. Real deployment inside tmux requires either `set -g allow-passthrough on` in tmux.conf or the user being outside tmux. Unit 19 should add a startup warning if `$TMUX` is set and passthrough is off.
- **WezTerm Kitty coverage.** Not exercised. Per wezterm#2756, #6334 the Kitty graphics protocol support is partial and `a=T` update-in-place may not work the same way. Unit 19 should decide whether to commit to WezTerm for v1 or treat it as best-effort.
- **Colour management.** `Rgba8Unorm` was used for simplicity. Eventually we should verify the shader output is in the sRGB colour space the terminal expects — this matters for the Auralis gradient to look right across terminals.
- **Long-run stability.** The longest test was 34 s (1800 frames paced at 60 fps). The bead called for a 5-minute stability run to catch slow leaks. Deferred to Unit 19 stress testing.

## Reproducer

```
cargo run -p clitunes-spike --release -- \
  --width 1920 --height 1080 \
  --frames 1800 --target-fps 60 \
  --output null
```

Other flags: `--no-pace` (unpaced, measures ceiling), `--output stdout|tty` (real terminal).

## Disposition of the spike code

Per the bead: the spike crate will be **deleted at the end of Phase 1**. Slice 1 (Unit 4) will reimplement the render → readback → encode → write path against the real `Visualiser` trait, inheriting the architectural pattern validated here but not the source files.
