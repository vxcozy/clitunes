//! Auto-spawn helper: connect to the clitunesd control socket, launching
//! the daemon if it isn't running yet.
//!
//! The client's happy path is simple:
//!   1. Try to `connect(2)` to `$runtime_dir/clitunesd.sock`.
//!   2. On `ENOENT` / `ECONNREFUSED`, locate the `clitunesd` binary —
//!      first next to the current executable (cargo / brew install
//!      layout), then via `$PATH`.
//!   3. Spawn it with stdin/stdout/stderr → /dev/null so the shell is
//!      not tethered to the daemon's lifetime.
//!   4. Poll the socket every 100 ms for up to ~2 s until a connect
//!      succeeds (or bail with a clear error).
//!
//! Unit 10 wires the control-socket wire protocol on top of this; for
//! Unit 9 we deliberately return just a raw `UnixStream` because the
//! protocol spec isn't stable yet.

use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clitunes_engine::daemon::runtime_dir;

/// How often we re-try the `connect(2)` while waiting for a freshly
/// spawned daemon to bind its socket.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Upper bound on how long to wait before giving up.
pub const DEFAULT_SPAWN_TIMEOUT: Duration = Duration::from_secs(2);

/// Where the control socket lives within the runtime dir.
pub const SOCKET_NAME: &str = "clitunesd.sock";
/// Daemon binary name, used for both the sibling-exe and `$PATH` lookups.
pub const DAEMON_BIN: &str = "clitunesd";

/// Outcome of a successful connection attempt.
#[derive(Debug)]
pub struct Connected {
    pub stream: UnixStream,
    pub socket_path: PathBuf,
    pub spawned_daemon: bool,
}

/// Errors that can stop the auto-spawn flow.
#[derive(Debug, thiserror::Error)]
pub enum AutoSpawnError {
    #[error("resolve runtime dir: {0}")]
    RuntimeDir(#[source] io::Error),

    #[error(
        "could not find clitunesd; check installation (looked next to {client_exe:?} and on $PATH)"
    )]
    DaemonNotFound { client_exe: Option<PathBuf> },

    #[error("spawn clitunesd ({path:?}): {source}")]
    Spawn {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("clitunesd did not accept connections within {timeout:?} after spawn")]
    SpawnTimeout { timeout: Duration },

    #[error("connect {socket:?}: {source}")]
    Connect {
        socket: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Connect to the default socket, spawning the daemon if needed.
pub fn connect_or_spawn() -> Result<Connected, AutoSpawnError> {
    let dir = runtime_dir().map_err(AutoSpawnError::RuntimeDir)?;
    let socket = dir.join(SOCKET_NAME);
    connect_or_spawn_at(&socket, &SpawnConfig::default())
}

/// Configuration knobs for unit tests — lets us point at a fake daemon
/// binary, override the poll timeout, and supply an explicit daemon
/// lookup hint.
#[derive(Clone, Debug)]
pub struct SpawnConfig {
    /// Maximum time to wait for the daemon to start listening.
    pub timeout: Duration,
    /// Explicit override for the daemon binary path. When `None`, the
    /// sibling-exe + `$PATH` lookup runs as usual.
    pub daemon_override: Option<PathBuf>,
    /// Extra args to pass to the spawned daemon. Tests use this to
    /// point a fake daemon at the socket it should bind; production
    /// leaves it empty so the daemon picks its default lock / socket
    /// paths.
    pub extra_args: Vec<String>,
    /// When true, skip spawning and return a connect error if the
    /// socket is missing. Useful for "is the daemon alive?" checks.
    pub skip_spawn: bool,
}

impl Default for SpawnConfig {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_SPAWN_TIMEOUT,
            daemon_override: None,
            extra_args: Vec::new(),
            skip_spawn: false,
        }
    }
}

/// Same as [`connect_or_spawn`] but at an explicit socket path and with
/// injectable spawn config. Used by integration tests to stand up a
/// sandbox-local daemon without touching `$XDG_RUNTIME_DIR`.
pub fn connect_or_spawn_at(socket: &Path, cfg: &SpawnConfig) -> Result<Connected, AutoSpawnError> {
    if let Some(stream) = try_connect(socket)? {
        return Ok(Connected {
            stream,
            socket_path: socket.to_path_buf(),
            spawned_daemon: false,
        });
    }

    if cfg.skip_spawn {
        return Err(AutoSpawnError::Connect {
            socket: socket.to_path_buf(),
            source: io::Error::from(io::ErrorKind::NotFound),
        });
    }

    let daemon = resolve_daemon_path(cfg.daemon_override.as_deref())?;
    spawn_daemon(&daemon, &cfg.extra_args)?;

    let deadline = Instant::now() + cfg.timeout;
    while Instant::now() < deadline {
        if let Some(stream) = try_connect(socket)? {
            return Ok(Connected {
                stream,
                socket_path: socket.to_path_buf(),
                spawned_daemon: true,
            });
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    Err(AutoSpawnError::SpawnTimeout {
        timeout: cfg.timeout,
    })
}

/// Attempt a single non-blocking connect. Returns `Ok(Some(stream))` on
/// success, `Ok(None)` on the benign "socket not there yet" errors, and
/// `Err(...)` for anything else (permission denied, etc.).
fn try_connect(socket: &Path) -> Result<Option<UnixStream>, AutoSpawnError> {
    match UnixStream::connect(socket) {
        Ok(s) => Ok(Some(s)),
        Err(e) => match e.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => Ok(None),
            _ => Err(AutoSpawnError::Connect {
                socket: socket.to_path_buf(),
                source: e,
            }),
        },
    }
}

/// Locate the daemon binary. First try the directory next to the
/// running client (cargo/homebrew layouts put siblings there), then
/// fall back to `$PATH`. An explicit override short-circuits both.
pub fn resolve_daemon_path(daemon_override: Option<&Path>) -> Result<PathBuf, AutoSpawnError> {
    if let Some(p) = daemon_override {
        if p.exists() {
            return Ok(p.to_path_buf());
        }
        return Err(AutoSpawnError::DaemonNotFound {
            client_exe: Some(p.to_path_buf()),
        });
    }

    let client_exe = std::env::current_exe().ok();
    if let Some(exe) = client_exe.as_deref() {
        if let Some(parent) = exe.parent() {
            let sibling = parent.join(DAEMON_BIN);
            if sibling.exists() {
                return Ok(sibling);
            }
        }
    }

    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(DAEMON_BIN);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    Err(AutoSpawnError::DaemonNotFound { client_exe })
}

/// `Command::spawn` wrapper with stdio → /dev/null so the daemon does
/// not inherit the client's tty. The child process keeps running after
/// we return; flock + the daemon's own idle exit manage its lifetime.
fn spawn_daemon(path: &Path, extra_args: &[String]) -> Result<(), AutoSpawnError> {
    let mut cmd = Command::new(path);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .args(extra_args);
    cmd.spawn()
        .map(|_child| ())
        .map_err(|source| AutoSpawnError::Spawn {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_override_missing_errors() {
        let missing = PathBuf::from("/nonexistent/clitunesd");
        let err = resolve_daemon_path(Some(&missing)).unwrap_err();
        assert!(matches!(err, AutoSpawnError::DaemonNotFound { .. }));
    }

    #[test]
    fn resolve_override_present_returns_path() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("clitunesd");
        std::fs::write(&bin, b"#!/bin/sh\nexit 0\n").unwrap();
        let resolved = resolve_daemon_path(Some(&bin)).unwrap();
        assert_eq!(resolved, bin);
    }

    #[test]
    fn try_connect_returns_none_on_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let socket = tmp.path().join("absent.sock");
        let out = try_connect(&socket).unwrap();
        assert!(out.is_none());
    }

    #[test]
    fn skip_spawn_surfaces_connect_error_when_socket_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let socket = tmp.path().join("absent.sock");
        let cfg = SpawnConfig {
            skip_spawn: true,
            ..SpawnConfig::default()
        };
        let err = connect_or_spawn_at(&socket, &cfg).unwrap_err();
        assert!(matches!(err, AutoSpawnError::Connect { .. }));
    }
}
