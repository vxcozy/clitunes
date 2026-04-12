# clitunes

A terminal music player with internet radio and real-time visualisers.

Daemon/client architecture — `clitunesd` handles audio while `clitunes` renders
visualisers at 30 fps using half-block ANSI in any terminal that supports
24-bit color.

## Install

**Homebrew** (macOS):

```
brew install vxcozy/tap/clitunes
```

**Cargo** (any platform):

```
cargo install clitunes
```

**From source**:

```
git clone https://github.com/vxcozy/clitunes.git
cd clitunes
cargo build --release
```

Binaries land in `target/release/clitunes` and `target/release/clitunesd`.

## Usage

```
clitunes                          # full TUI — picker + visualiser carousel
clitunes play|pause|next|prev     # headless playback control
clitunes volume 75                # set volume
clitunes viz auralis              # switch visualiser
clitunes source radio <uuid>      # tune to a radio station
clitunes source local <path>      # play a local file or directory
clitunes status --json            # current state as JSON
clitunes --pane visualiser        # standalone fullscreen visualiser
```

On first run, `clitunes` auto-starts the daemon and shows a station picker.
Pick a genre, and audio starts streaming with the default **Auralis** visualiser.

### Keys

| Key | Action |
|-----|--------|
| `n` / `p` | Next / previous visualiser |
| `Up` / `Down` (or `j` / `k`) | Move picker selection |
| `Enter` | Confirm picker selection |
| `s` | Show / hide station picker |
| `q` / `Esc` | Quit |

## Visualisers

Eight visualisers ship with v1, all reactive to the audio spectrum:

- **Auralis** — vertical frequency bands with amplitude-driven color (default)
- **Tideline** — horizontal waveform with a receding shoreline effect
- **Cascade** — waterfall spectrogram scrolling downward
- **Plasma** — classic plasma field modulated by bass energy
- **Ripples** — concentric rings expanding from beat transients
- **Tunnel** — fly-through tunnel warped by mid-range frequencies
- **Metaballs** — floating blobs that merge and split with the music
- **Starfield** — depth-sorted stars accelerated by audio intensity

Cycle through them with `n`/`p` or switch directly: `clitunes viz cascade`.

## Architecture

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

- **Shared-memory SPMC ring** delivers PCM to multiple clients with zero copies
- **Daemon auto-starts** on first `clitunes` invocation, idles down after clients disconnect
- **State persistence** remembers your last station across sessions

## Requirements

- Any terminal with 24-bit color (Ghostty, Kitty, WezTerm, iTerm2, Alacritty, etc.)
- macOS or Linux
- Audio output device (for the daemon)

## License

[MIT](LICENSE)
