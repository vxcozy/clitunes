#![cfg(all(unix, feature = "daemon"))]

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};

use clitunes_engine::daemon::{check_peer, my_uid, peer_cred, set_socket_umask, AcceptGuard};

#[test]
fn socket_bound_after_umask_is_mode_0600() {
    let tmp = tempfile::tempdir().unwrap();
    let sock_path = tmp.path().join("test.sock");

    let prev = set_socket_umask();
    let _listener = UnixListener::bind(&sock_path).unwrap();
    unsafe { libc::umask(prev) };

    let meta = std::fs::metadata(&sock_path).unwrap();
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "socket should be 0600, got {mode:o}");
}

#[test]
fn peercred_on_bound_socket_returns_our_uid() {
    let tmp = tempfile::tempdir().unwrap();
    let sock_path = tmp.path().join("peercred.sock");

    let listener = UnixListener::bind(&sock_path).unwrap();
    let _client = UnixStream::connect(&sock_path).unwrap();
    let (server_stream, _addr) = listener.accept().unwrap();

    let cred = peer_cred(&server_stream).unwrap();
    assert_eq!(cred.uid, my_uid());
    assert!(cred.pid > 0);
}

#[test]
fn check_peer_allows_same_uid() {
    let tmp = tempfile::tempdir().unwrap();
    let sock_path = tmp.path().join("guard.sock");

    let listener = UnixListener::bind(&sock_path).unwrap();
    let _client = UnixStream::connect(&sock_path).unwrap();
    let (server_stream, _addr) = listener.accept().unwrap();

    match check_peer(server_stream) {
        AcceptGuard::Allowed(_) => {}
        other => panic!("expected Allowed, got {other:?}"),
    }
}

#[test]
fn runtime_dir_parent_is_0700() {
    let tmp = tempfile::tempdir().unwrap();
    let prior_xdg = std::env::var_os("XDG_RUNTIME_DIR");
    std::env::set_var("XDG_RUNTIME_DIR", tmp.path());

    let dir = clitunes_engine::daemon::runtime_dir().unwrap();
    let meta = std::fs::metadata(&dir).unwrap();
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(mode, 0o700, "runtime dir should be 0700, got {mode:o}");

    if let Some(v) = prior_xdg {
        std::env::set_var("XDG_RUNTIME_DIR", v);
    } else {
        std::env::remove_var("XDG_RUNTIME_DIR");
    }
}

#[test]
fn stale_socket_is_removed_on_rebind() {
    let tmp = tempfile::tempdir().unwrap();
    let sock_path = tmp.path().join("stale.sock");

    let _l1 = UnixListener::bind(&sock_path).unwrap();
    drop(_l1);

    assert!(
        sock_path.exists(),
        "socket file should still exist after listener drop"
    );

    std::fs::remove_file(&sock_path).unwrap();
    let _l2 = UnixListener::bind(&sock_path).unwrap();
    assert!(sock_path.exists());
}
