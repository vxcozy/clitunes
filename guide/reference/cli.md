# CLI reference

## Global options

These work alongside any mode:

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help and exit |
| `-V`, `--version` | Print version (`clitunes <version>`) and exit |

`clitunesd` accepts the same `-h` / `-V` flags.

## Modes

### Full TUI (default)

```
clitunes [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--source <auto\|tone\|radio\|spotify>` | Audio source (default: `auto` — resume last or show picker) |
| `--station <uuid>` | Radio station UUID (with `--source radio`) |
| `--measure-startup` | Print timing breakdown to stderr, exit after first frame |

### Standalone pane

```
clitunes --pane <name> [--viz <visualiser>]
```

| Pane name | Description |
|-----------|-------------|
| `visualiser` | Fullscreen visualiser |
| `now-playing` | Track info strip (1–3 rows) |
| `mini-spectrum` | Unicode block spectrum bars (1 row) |

### Headless verbs

```
clitunes play
clitunes pause
clitunes next
clitunes prev
clitunes volume <0-100>
clitunes viz <name>
clitunes source radio <uuid>
clitunes source local <path>
clitunes source spotify:<uri>
clitunes connect disconnect
```

### Browse and search

```
clitunes search "<query>" [limit]
clitunes browse <category> [limit]
clitunes browse-playlist <id-or-uri> [limit]
```

See [Browse and search Spotify](../how-to/browse-spotify.md) for
details and output format.

### Status query

```
clitunes status [--json]
```

Returns current playback state as JSON.

### Authentication

```
clitunes auth
```

Runs the Spotify OAuth flow and caches credentials. See
[Play Spotify tracks](../how-to/play-spotify.md) for details.

## Keyboard shortcuts

| Key | Action |
|-----|--------|
| `n` | Next visualiser |
| `p` | Previous visualiser |
| `Up` / `k` | Move picker selection up |
| `Down` / `j` | Move picker selection down |
| `Enter` | Confirm picker selection |
| `s` | Show / hide station picker |
| `q` / `Esc` | Quit |

## Visualisers

| Name | Description |
|------|-------------|
| `plasma` | Classic plasma field modulated by bass energy (default) |
| `ripples` | Concentric rings expanding from beat transients |
| `tunnel` | Fly-through tunnel warped by mid-range frequencies |
| `metaballs` | Floating blobs that merge and split with the music |
| `fire` | Cellular automaton fire with audio-driven roar |
| `matrix` | Falling code rain with beat-synced glitch bursts |
| `moire` | Overlapping interference patterns pulsing with bass |
| `vortex` | Spiral tunnel warped by frequency bands |
| `wave` | Braille oscilloscope tracing the raw audio waveform |
| `scope` | Braille Lissajous XY plot with drifting phase offset |
| `heartbeat` | Braille ECG-style scrolling pulse trace |
| `classicpeak` | Winamp-style spectrum bars with falling peak caps |
| `barsdot` | Braille-stippled spectrum bars |
| `barsoutline` | Box-drawing outline tracing the spectrum top edge |
| `binary` | Streaming binary digits scrolling with audio energy |
| `scatter` | Braille particle field twinkling with audio density |
| `terrain` | Braille scrolling mountain range shaped by spectrum |
| `butterfly` | Braille mirrored Rorschach inkblot from frequency bands |
| `pulse` | Braille pulsating circle with shockwave rings on beats |
| `rain` | Box-drawing falling rain streaks driven by frequency bands |
| `sakura` | Braille cherry blossom petals drifting with audio energy |
| `firework` | Braille particle explosions with rising trails and bursts |
| `retro` | Braille synthwave scene with sun, wave, and perspective grid |

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success |
| `1` | Error (daemon connection failed, invalid arguments, etc.) |
