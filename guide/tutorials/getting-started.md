# Getting started with clitunes

This tutorial walks you through installing clitunes, picking your first radio
station, and exploring the visualisers. By the end you'll have music playing
in your terminal.

## Prerequisites

- A terminal with 24-bit color support (Ghostty, Kitty, WezTerm, iTerm2,
  Alacritty, or similar)
- macOS or Linux
- An audio output device

## Install

Pick one:

```
brew install vxcozy/tap/clitunes    # Homebrew (macOS)
cargo install clitunes              # from crates.io
```

Or build from source:

```
git clone https://github.com/vxcozy/clitunes.git
cd clitunes
cargo build --release
```

## Launch

```
clitunes
```

On first run, clitunes auto-starts its daemon (`clitunesd`) and shows a station
picker over the Auralis visualiser:

```
╭──────────────────────────────────────────╮
│       First time? Pick a starting point. │
│            You can change it anytime.    │
│                                          │
│  1. ambient                              │
│  2. classical                            │
│  3. jazz                                 │
│  ...                                     │
│                                          │
│  ↑/↓ move   enter select   s hide   q   │
│  n/p cycle viz · auralis · tideline      │
╰──────────────────────────────────────────╯
```

Use the arrow keys to highlight a genre, then press **Enter**. Audio starts
streaming and the visualiser responds to the music.

## Explore visualisers

Press **n** to cycle to the next visualiser, **p** to go back. There are eight:

1. **Auralis** — vertical frequency bands (default)
2. **Tideline** — horizontal waveform
3. **Cascade** — waterfall spectrogram
4. **Plasma** — bass-modulated plasma field
5. **Ripples** — concentric rings on beat transients
6. **Tunnel** — fly-through depth effect
7. **Metaballs** — merging/splitting blobs
8. **Starfield** — depth-sorted stars

Or switch directly from another terminal:

```
clitunes viz cascade
```

## Control playback

From any terminal (you don't need to be in the TUI):

```
clitunes pause
clitunes play
clitunes volume 50
clitunes next        # next station in picker order
clitunes prev        # previous station
```

## Quit

Press **q** or **Esc** in the TUI. The daemon stays running for 30 seconds in
case you relaunch — after that it exits automatically.

## Next steps

- [Embed in tmux/WezTerm/Ghostty](../how-to/embed-panes.md) — use standalone
  panes for multi-panel layouts
- [Play local files](../how-to/play-local-files.md)
- [Control bus reference](../reference/control-bus.md) — the JSON protocol
  powering headless commands
