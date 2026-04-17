# clitunes

A terminal music player with internet radio, Spotify, and real-time visualisers.

Daemon/client architecture — `clitunesd` handles audio while `clitunes` renders
visualisers at 30 fps using half-block ANSI in any terminal that supports
24-bit color.

## Install

**Homebrew** (macOS + Linux):

```
brew install vxcozy/tap/clitunes
```

**Direct download** (macOS + Linux, all four architectures):

Grab the pre-built tarball from the
[v1.0.0 release page](https://github.com/vxcozy/clitunes/releases/tag/v1.0.0)
and drop the binaries on your PATH.

**Cargo** (any Rust toolchain):

```
cargo install --git https://github.com/vxcozy/clitunes --tag v1.0.0 --locked
```

System prerequisites: `libasound2-dev` + `pkg-config` on Linux, Xcode Command
Line Tools on macOS.

> Plain `cargo install clitunes` from crates.io is pending the next librespot
> release (upstream vergen-gitcl fix already merged, awaiting publication).

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
clitunes source spotify:<uri>     # play a Spotify track (Premium required)
clitunes connect disconnect       # stop an active Spotify Connect session
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

Twenty-three visualisers ship with v1, all reactive to the audio spectrum.
They fall into four families:

- **Spectrum / core** — Auralis (default), Tideline, Cascade, Plasma,
  Ripples, Tunnel, Metaballs, Starfield, Fire, Matrix, Moire, Vortex
- **Oscilloscopes (braille)** — Wave, Scope, Heartbeat
- **Spectrum variants** — ClassicPeak, BarsDot, BarsOutline, Binary
- **Particle / scene** — Scatter, Terrain, Butterfly, Pulse, Rain, Sakura,
  Firework, Retro

Cycle through them with `n`/`p` or switch directly: `clitunes viz cascade`.
Full catalogue and rendering notes in
[guide/reference/cli.md](guide/reference/cli.md).

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
- **State persistence** remembers your last source across sessions
- **Spotify playback** via [librespot](https://github.com/librespot-org/librespot)
  (Premium required) with 44100→48000 Hz sinc resampling and OAuth2 PKCE auth.
  Includes Spotify Connect receiver — opt-in via daemon config
  (`[connect].enabled = true`) so clitunes appears in the Spotify device
  picker on the LAN.

## Requirements

- Any terminal with 24-bit color (Ghostty, Kitty, WezTerm, iTerm2, Alacritty, etc.)
- macOS or Linux
- Audio output device (for the daemon)

## Documentation

Full docs in the [guide/](guide/) directory, organised using the
[Diataxis](https://diataxis.fr/) framework:

- **[Getting started](guide/tutorials/getting-started.md)** — first launch to music in 2 minutes
- **[Embed panes](guide/how-to/embed-panes.md)** — tmux, WezTerm, Ghostty layouts
- **[Play local files](guide/how-to/play-local-files.md)**
- **[Play Spotify tracks](guide/how-to/play-spotify.md)** — auth, playback, troubleshooting
- **[Customise stations](guide/how-to/customise-stations.md)**
- **[CLI reference](guide/reference/cli.md)** — every flag, verb, and visualiser
- **[Control bus protocol](guide/reference/control-bus.md)** — JSON wire format
- **[Security model](guide/reference/security.md)** — threat scope, socket hardening, peercred
- **[Architecture](guide/explanation/architecture.md)** — daemon/client split, SPMC ring, D15
- **[Visualiser design](guide/explanation/visualisers.md)** — rendering pipeline, audio reactivity

## License

[MIT](LICENSE)
