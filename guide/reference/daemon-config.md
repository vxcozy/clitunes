# Daemon configuration file

`clitunesd` reads a single TOML file on startup to configure optional
features. No file is required — the daemon boots with sensible defaults
on a fresh install.

## Location

Resolved in order, first hit wins:

1. An explicit path passed to the daemon programmatically (a `--config`
   flag is planned for a later release; today this tier is used by
   tests).
2. `$CLITUNES_CONFIG` — absolute or relative path. Redirect the file
   without editing or symlinking.
3. `$XDG_CONFIG_HOME/clitunes/daemon.toml` on Linux, or
   `~/Library/Application Support/clitunes/daemon.toml` on macOS —
   whatever [`dirs::config_dir()`][dirs] returns for your platform.

A missing file at the resolved path is not an error: the daemon treats
it as "accept all defaults". An empty file does the same. A malformed
file (invalid TOML, an out-of-range value, a typo inside a known
section, an unknown enum variant) is a fatal startup error — by design,
so typos surface loud instead of silently dropping the setting you
meant.

The root document is forward-compatible: unknown top-level sections are
accepted and ignored, so a daemon running an older binary still starts
against a newer config file. Typo-catching applies *inside* each known
section (`[connect]`, and any future sections).

[dirs]: https://docs.rs/dirs/latest/dirs/fn.config_dir.html

## Shape

```toml
[connect]
enabled        = false       # Spotify Connect receiver off by default
name           = "clitunes"  # name shown in the Spotify Connect picker
bind           = "loopback"  # "loopback" (local-only) or "all" (LAN-visible)
port           = 0           # 0 = OS picks an ephemeral port
initial_volume = 50          # 0–100, volume announced on startup
device_type    = "speaker"   # icon hint: "speaker", "computer", "tv", …
```

Every field is optional. Omit a field and the default above applies.

## `[connect]` fields

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `false` | Master switch. When `false` the Connect subsystem does not start; no mDNS announcement, no `/addUser` endpoint. |
| `name` | string | `"clitunes"` | Device name other Spotify clients see in the devices list. Pick whatever you want. |
| `bind` | enum | `"loopback"` | `"loopback"` binds only `127.0.0.1` — the receiver is reachable only from the same machine. `"all"` binds every interface, required for phones and tablets on the LAN to see it. |
| `port` | u16 | `0` | TCP port for the `/addUser` HTTP endpoint. `0` asks the OS for an ephemeral port; the chosen port is announced via mDNS so clients find it automatically. |
| `initial_volume` | u8 | `50` | Volume percentage announced when the receiver registers. Clients can change it afterwards. |
| `device_type` | string | `"speaker"` | Device-category hint Spotify uses to pick an icon. Common values: `speaker`, `computer`, `tv`, `smartphone`, `tablet`, `avr`, `stb`. |

## Worked examples

Local-only try-it-out (defaults are already this, but explicit is fine):

```toml
[connect]
enabled = true
```

LAN-visible living-room speaker:

```toml
[connect]
enabled = true
name    = "Living Room"
bind    = "all"
```

Loud entrance — high volume, shown as a TV in the picker:

```toml
[connect]
enabled        = true
name           = "Den TV"
bind           = "all"
initial_volume = 85
device_type    = "tv"
```

## Reloading

The daemon reads the file once at startup. To apply changes, stop and
restart `clitunesd` (the idle-exit timer will take it down ~30 seconds
after the last client disconnects; the next client start re-launches
it).
