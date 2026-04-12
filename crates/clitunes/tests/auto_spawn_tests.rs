//! Integration tests for `clitunes::auto_spawn`.
//!
//! We want to verify the "socket not there → locate daemon → spawn →
//! poll until alive" happy path without depending on the real clitunesd
//! binary being built in `$PATH`. To get there:
//!
//!   * For the "daemon not found" case we just point the resolver at a
//!     nonexistent override path.
//!   * For the "socket already present" case we bind a Unix socket in
//!     the test and verify `connect_or_spawn_at` returns immediately
//!     with `spawned_daemon = false`.
//!   * For the "spawn a fake daemon and wait for socket" case we write
//!     a tiny shell script that binds a Unix socket with `python3 -c` —
//!     which is available on every dev box and both CI runners — and
//!     point the spawn config at it.
//!
//! The last case is skipped on machines without python3 so CI doesn't
//! block on an environment quirk.

#![cfg(unix)]

use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::time::Duration;

use clitunes::auto_spawn::{connect_or_spawn_at, resolve_daemon_path, AutoSpawnError, SpawnConfig};

#[test]
fn resolver_reports_not_found_when_override_is_absent() {
    let missing = PathBuf::from("/definitely/not/a/real/path/clitunesd");
    let err = resolve_daemon_path(Some(&missing)).unwrap_err();
    assert!(matches!(err, AutoSpawnError::DaemonNotFound { .. }));
}

#[test]
fn connect_succeeds_when_socket_already_bound() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = tmp.path().join("clitunesd.sock");
    let _listener = UnixListener::bind(&socket).unwrap();

    let cfg = SpawnConfig {
        skip_spawn: true,
        ..SpawnConfig::default()
    };
    let connected = connect_or_spawn_at(&socket, &cfg).expect("connect");
    assert_eq!(connected.socket_path, socket);
    assert!(
        !connected.spawned_daemon,
        "must not spawn when socket exists"
    );
}

#[test]
fn spawn_timeout_bubbles_up_when_daemon_never_binds() {
    // Use `true` (the coreutils binary) as the fake daemon: it exits
    // immediately without ever binding the socket, so the poller times
    // out with SpawnTimeout. `/usr/bin/true` exists on every unix.
    let true_bin = PathBuf::from("/usr/bin/true");
    if !true_bin.exists() {
        eprintln!("skipping: /usr/bin/true not present");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let socket = tmp.path().join("never.sock");

    let cfg = SpawnConfig {
        timeout: Duration::from_millis(400),
        daemon_override: Some(true_bin),
        extra_args: Vec::new(),
        skip_spawn: false,
    };
    let err = connect_or_spawn_at(&socket, &cfg).unwrap_err();
    assert!(
        matches!(err, AutoSpawnError::SpawnTimeout { .. }),
        "expected SpawnTimeout, got {err:?}"
    );
}

#[test]
fn spawn_then_poll_succeeds_when_fake_daemon_binds_socket() {
    // Fake daemon: a tiny python3 one-liner that binds the requested
    // socket path after a short delay, then sleeps. Skipped if python3
    // isn't on PATH so we don't wedge environments without it.
    let python3 = which_bin("python3");
    let Some(python3) = python3 else {
        eprintln!("skipping: python3 not in PATH");
        return;
    };

    let tmp = tempfile::tempdir().unwrap();
    let socket = tmp.path().join("fake.sock");
    let script = "import socket,os,time,sys\n\
         time.sleep(0.2)\n\
         s=socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)\n\
         s.bind(sys.argv[1])\n\
         s.listen(1)\n\
         time.sleep(5)\n";

    // Write the script to a real file so the daemon_override can point
    // at a wrapper shell that invokes python3 with the script + socket.
    let script_path = tmp.path().join("fake_daemon.py");
    std::fs::write(&script_path, script).unwrap();

    let cfg = SpawnConfig {
        timeout: Duration::from_secs(3),
        daemon_override: Some(python3),
        extra_args: vec![
            script_path.to_string_lossy().into_owned(),
            socket.to_string_lossy().into_owned(),
        ],
        skip_spawn: false,
    };

    let connected = connect_or_spawn_at(&socket, &cfg).expect("connect_or_spawn_at");
    assert!(connected.spawned_daemon);
    assert_eq!(connected.socket_path, socket);
}

fn which_bin(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}
