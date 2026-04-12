# Security model

## Threat scope

clitunes is a local desktop application. It makes outbound network
connections (HTTPS to radio-browser.info, HTTP/HTTPS to radio streams)
but exposes no listening network ports. The only IPC surface is a Unix
domain socket for daemon control.

## What clitunes protects against

### Other local UIDs (multi-user hosts)

The daemon's control socket is protected by three layers:

1. **Umask-atomic bind (SEC-001).** Before `bind(2)`, the process sets
   `umask(0o177)` so the socket inode is created with mode `0600`
   atomically. There is no TOCTOU window between bind and chmod.

2. **Runtime directory permissions.** The socket lives under
   `$XDG_RUNTIME_DIR/clitunes/` (Linux, mode 0700 by systemd-logind) or
   `$TMPDIR/clitunes-$USER/` (macOS, mode 0700 created by clitunes).
   Even if the socket mode were wrong, the parent directory blocks
   traversal by other UIDs.

3. **Peercred UID gate.** On every `accept(2)`, the daemon queries the
   peer's UID via `SO_PEERCRED` (Linux) or `LOCAL_PEERCRED` (macOS). If
   the peer UID does not match `getuid()`, the connection is closed
   immediately — before any data is read. This is the last line of
   defence if both umask and directory permissions somehow fail.

### Singleton enforcement

The flock at `$runtime_dir/clitunesd.lock` prevents a second daemon from
binding to the same socket path. Unlike pidfile-based locking, the
kernel releases the flock on process death so a crashed daemon cannot
leave a stale lock behind.

### Terminal escape injection (D20)

Radio station metadata and ICY stream titles pass through the
`UntrustedString` sanitiser (clitunes-core) before reaching the TUI.
The sanitiser strips C0 controls (except whitespace), C1 codepoints,
ESC sequences, OSC sequences (including clipboard/title attacks), and
CSI sequences. This prevents a malicious radio stream from injecting
terminal control sequences into the user's display.

### Control bus DoS (SEC-007)

The control bus uses `LinesCodec::new_with_max_length(65_536)`. A
client sending a line longer than 64KB is disconnected immediately.
Without this limit, a single malicious pre-peercred connection could OOM
the daemon by sending a multi-gigabyte line.

### Slow client DoS

Each connected client has a per-client mpsc channel with capacity 64.
If a client falls behind (doesn't read events), the channel fills and
the daemon disconnects that client. Other clients are unaffected.
`broadcast::Sender` is not used anywhere (D14) because it silently
drops messages, hiding backpressure failures.

### Supply chain

`cargo-deny` runs in CI with a conservative license allow-list and
advisory database checks. Known advisories are documented with
justification in `deny.toml`. This narrows (but does not eliminate)
supply chain risk.

## What clitunes does NOT protect against

### Root on the same machine

Root can read any socket, attach to any process, and access any file.
clitunes does not attempt to defend against a compromised root account.

### Network attackers

clitunes makes outbound HTTPS connections only (radio-browser.info
discovery, radio stream URLs). It does not listen on any network port.
A network attacker who can MITM the HTTPS connection could serve
malicious stream data, but the audio decoder (symphonia) is a
memory-safe Rust library and stream metadata is sanitised before
display.

### Compromised CA chain

clitunes trusts the system CA store (via rustls + webpki-roots) for
HTTPS connections to radio-browser.info. Compromise of a trusted CA
could allow a MITM to serve a malicious mirror list. The blast radius
is limited to radio stream discovery; local playback is unaffected.

### Compromised upstream dependency

cargo-deny reduces the surface by flagging advisories and restricting
licenses, but a zero-day in any transitive dependency could
compromise the process. This is inherent to software supply chains.

### Compromised user account

If the user's own account is compromised, the attacker has the same UID
and passes the peercred check. clitunes does not sandbox itself beyond
the user's own privileges. The threat model assumes the user account
is trusted.

## Peercred mechanism

### Linux

```c
struct ucred cred;
socklen_t len = sizeof(cred);
getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &cred, &len);
// cred.uid, cred.gid, cred.pid
```

The kernel fills the `ucred` at connection time. The values cannot be
spoofed by the peer.

### macOS

```c
// PID
pid_t pid;
socklen_t len = sizeof(pid);
getsockopt(fd, SOL_LOCAL, LOCAL_PEERPID, &pid, &len);

// UID + GID
struct xucred cred;
cred.cr_version = XUCRED_VERSION;
socklen_t len = sizeof(cred);
getsockopt(fd, SOL_LOCAL, LOCAL_PEERCRED, &cred, &len);
// cred.cr_uid, cred.cr_groups[0]
```

## Verifying your socket permissions

```sh
stat $XDG_RUNTIME_DIR/clitunes/clitunesd.sock
# Should show: srw------- (mode 0600)

ls -ld $XDG_RUNTIME_DIR/clitunes/
# Should show: drwx------ (mode 0700)
```
