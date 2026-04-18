# Visualiser design

## The rendering pipeline

Every visualiser implements the same `Visualiser` trait:

```
FFT snapshot → Visualiser::render_tui(ctx, snapshot) → CellGrid → AnsiWriter → stdout
```

The `FftTap` produces a frequency-domain snapshot from the PCM ring at ~30 fps.
Each visualiser reads the snapshot's magnitude bins, peak energy, and bass/mid/high
band levels to drive its animation.

## Why twenty-three visualisers?

clitunes aims to be the "Ghostty of TUI music apps" — a tool where the visual
experience is a first-class feature, not an afterthought. Different music
calls for different aesthetics. The catalogue spans four rendering families;
a sampler of each:

| Visualiser | Character | Best with |
|------------|-----------|-----------|
| **Plasma** | Psychedelic, warm (default) | Bass-heavy genres, downtempo |
| **Ripples** | Rhythmic, expanding | Percussion-forward music, jazz |
| **Tunnel** | Hypnotic, depth | Mid-range-rich music, vocals |
| **Metaballs** | Organic, morphing | Experimental, textural music |
| **Wave** | Braille oscilloscope | Anything — a clean signal view |
| **ClassicPeak** | Winamp-style bars | Dynamic-range-rich music |
| **Sakura** | Cherry blossom particles | Ambient, contemplative listening |
| **Retro** | 80s synthwave scene | Driving, atmospheric music |

See `guide/reference/cli.md` for the full catalogue.

## CPU-only by design

All visualisers are pure-CPU implementations using Unicode half-block
characters (`▀`), density-ramp glyphs (`.,:;+*#%@`), or braille sub-pixels
(U+2800 block for 2×4 dots per cell) with 24-bit ANSI color. Each cell in
the grid carries a foreground + background colour and a glyph.

This was a deliberate choice after the Phase 0 spike showed that GPU rendering
via wgpu + Kitty graphics protocol was slower in practice due to readback
latency and base64 encoding overhead. The CPU approach is:

- **More portable** — works in any terminal with 24-bit color
- **Lower power** — no GPU wake for a background music player
- **Simpler** — no shader compilation, no graphics API surface

## Audio reactivity

Each visualiser pulls different features from the FFT snapshot:

- **Magnitude bins** — per-frequency amplitude, the raw spectrum
- **Band energy** — aggregated bass, mid, high levels
- **Peak tracking** — smoothed peak for envelope following
- **Beat detection** — transient energy spikes above a running threshold

The snapshot is computed once per frame and shared across all visualisers in the
carousel, so switching visualisers is instantaneous.
