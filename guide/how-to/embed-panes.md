# Embed clitunes panes in tmux, WezTerm, or Ghostty

clitunes can run as standalone single-component panes, making it easy to embed
in terminal multiplexer layouts alongside other tools.

## Available panes

| Pane | Description |
|------|-------------|
| `visualiser` | Fullscreen visualiser (default: Auralis) |
| `now-playing` | Track info strip (1–3 rows) |
| `mini-spectrum` | Unicode block spectrum bars (1 row, for status lines) |

## Basic usage

```
clitunes --pane visualiser
clitunes --pane visualiser --viz cascade
clitunes --pane now-playing
clitunes --pane mini-spectrum
```

Each pane connects to the running daemon independently — you can have multiple
panes in different terminal splits showing different components.

## tmux example

```bash
# Main pane: visualiser
tmux new-session -d -s music 'clitunes --pane visualiser'

# Bottom strip: now-playing
tmux split-window -v -l 3 'clitunes --pane now-playing'

# Attach
tmux attach -t music
```

## WezTerm example

In your `wezterm.lua`:

```lua
local wezterm = require 'wezterm'

wezterm.on('gui-startup', function(cmd)
  local tab, pane, window = wezterm.mux.spawn_window {
    args = { 'clitunes', '--pane', 'visualiser' },
  }
  pane:split {
    direction = 'Bottom',
    size = 0.1,
    args = { 'clitunes', '--pane', 'now-playing' },
  }
end)
```

## Ghostty example

In a Ghostty split layout, run each pane command in its own split.

## Notes

- All panes share the same daemon — switching stations in the TUI updates
  every connected pane.
- If the daemon isn't running, each pane auto-spawns it.
- Panes respond to the same keyboard shortcuts as the full TUI (`n`/`p` for
  visualiser cycling, `q` to quit the pane).
