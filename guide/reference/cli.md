# CLI reference

## Modes

### Full TUI (default)

```
clitunes [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--source <auto\|tone\|radio>` | Audio source (default: `auto` — resume last or show picker) |
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
```

### Status query

```
clitunes status [--json]
```

Returns current playback state as JSON.

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
| `auralis` | Vertical frequency bands with amplitude-driven color (default) |
| `tideline` | Horizontal waveform with receding shoreline effect |
| `cascade` | Waterfall spectrogram scrolling downward |
| `plasma` | Classic plasma field modulated by bass energy |
| `ripples` | Concentric rings expanding from beat transients |
| `tunnel` | Fly-through tunnel warped by mid-range frequencies |
| `metaballs` | Floating blobs that merge and split with the music |
| `starfield` | Depth-sorted stars accelerated by audio intensity |

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success |
| `1` | Error (daemon connection failed, invalid arguments, etc.) |
