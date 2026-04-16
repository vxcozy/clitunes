# Use Spotify Connect

clitunes can appear as a Spotify Connect device, letting you start and
control playback from the Spotify app on your phone, tablet, or desktop.
Audio routes through the daemon's visualiser pipeline — you get the
full clitunes experience controlled from an external device.

This requires a **Spotify Premium** account and a build with the
`connect` feature enabled.

## Build with Connect support

Connect is opt-in. Pass the `connect` feature flag when building the
daemon:

```
cargo build -p clitunesd --features connect
```

The default `cargo build` does not include Connect — you must opt in
explicitly.

## Enable in daemon.toml

Add a `[connect]` section to your daemon config file. On macOS that's
`~/Library/Application Support/clitunes/daemon.toml`; on Linux it's
`~/.config/clitunes/daemon.toml`. Create the file if it doesn't exist.

### Local-only (try it out)

```toml
[connect]
enabled = true
```

With the default `bind = "loopback"`, the receiver is only reachable
from the same machine. Useful for testing or when running Spotify
desktop and clitunes on the same box.

### LAN-visible (phone/tablet control)

```toml
[connect]
enabled = true
name    = "Living Room"
bind    = "all"
```

Setting `bind = "all"` advertises the device via mDNS on all network
interfaces so phones and tablets on the same Wi-Fi can discover it.

See the [daemon config reference](../reference/daemon-config.md) for the
full list of `[connect]` fields.

## Pick the device from your phone

1. Restart `clitunesd` (or let the idle timer expire and relaunch it by
   running any `clitunes` command)
2. Open the Spotify app on your phone
3. Start playing a track, then tap the **Devices Available** icon
4. Select your clitunes device (the name from `daemon.toml`)
5. Audio plays through clitunes — the visualiser responds immediately

## Disconnect

From a terminal:

```
clitunes connect disconnect
```

This tears down the active session but keeps Discovery advertising, so
you can re-pick the device from your phone without restarting the
daemon. The verb is idempotent: disconnecting when nothing is connected
is a silent no-op.

## How it works

When you pick the device, the Spotify app sends credentials to the
daemon's `/addUser` HTTP endpoint (discovered via mDNS). The daemon
builds a librespot Session + Spirc (Spotify's Connect state machine) and
enters passive playback mode: Spirc owns the track lifecycle
(play/pause/skip/seek), the daemon just pumps PCM into the visualiser
pipeline.

If you pick the device again after a disconnect, the daemon builds a
fresh Session from the new credentials. Discovery survives across
sessions — no restart needed.

## Troubleshooting

### Device doesn't appear in the Spotify app

- Check that `bind = "all"` is set in `daemon.toml`. The default
  `"loopback"` restricts mDNS to localhost — phones on the LAN can't
  see it.
- Make sure your phone and the machine running clitunes are on the same
  Wi-Fi network. Enterprise networks and hotel Wi-Fi sometimes block
  mDNS traffic.
- Verify that `clitunesd` is running and the connect receiver started.
  Check the daemon log for `connect: discovery advertising`.

### Device appears but playback hangs on "connecting..."

- Verify your Spotify account is Premium. Free-tier accounts cannot use
  Connect.
- Check the daemon log for `connect: Spirc::new failed` — this usually
  means a credential mismatch or network issue reaching Spotify's
  dealer endpoint.

### Audio plays but there's no visualiser response

The visualiser pipeline is running but may be on a different source.
Check `clitunes status --json` — the `source` field should be
`"connect"`. If it's something else, the phone handoff may not have
triggered `PlayConnect` in the source pipeline.

### Loopback mode for SSH tunnels

If you're running clitunes over SSH, keep `bind = "loopback"` and
forward the discovery port:

```
ssh -L <port>:127.0.0.1:<port> <host>
```

Set a fixed `port` in `daemon.toml` (e.g. `port = 4070`) rather than
the default `0` (ephemeral), so the tunnel target is predictable.

### Overriding the config path

Set `$CLITUNES_CONFIG` to point at a different file:

```
CLITUNES_CONFIG=/path/to/my-daemon.toml clitunesd
```
