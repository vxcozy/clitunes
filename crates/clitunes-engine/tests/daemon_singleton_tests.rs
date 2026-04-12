//! Cross-process tests for the clitunesd singleton flock.
//!
//! The in-module unit tests in `singleton.rs` cover in-process
//! acquire/release sequencing. This integration test verifies the
//! *cross-process* property that matters in production: flock is
//! released by the kernel when the holding process dies, so a crashed
//! daemon can't leave a stale lock. We use `libc::fork(2)` to stand up
//! a second process that holds the lock and communicates via a pipe
//! rather than pulling in a test-framework dependency to re-invoke the
//! test binary.

#![cfg(all(unix, feature = "daemon"))]

use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::sync::Mutex;
use std::time::Duration;

// Serialise fork-based tests so two forks never race across test
// threads. fork(2) from a multithreaded process is already a sharp
// edge — running two in parallel would compound the risk for no gain.
static FORK_GUARD: Mutex<()> = Mutex::new(());

use clitunes_engine::daemon::{acquire_at, AcquireOutcome};

/// Fork helper. Returns `(child_pid, parent_stdin_pipe, parent_stdout_pipe)`
/// in the parent; the child never returns — it runs `child_fn` and
/// exits with the returned code.
fn fork_with_pipes<F: FnOnce(std::fs::File, std::fs::File) -> i32>(
    child_fn: F,
) -> (libc::pid_t, std::fs::File, std::fs::File) {
    // Two pipes: one for parent→child (release), one for child→parent
    // (ready signal). Using bytes on pipes beats polling filesystems.
    let mut parent_to_child = [0_i32; 2];
    let mut child_to_parent = [0_i32; 2];
    // SAFETY: pipe(2) on a valid [i32; 2] is safe.
    unsafe {
        assert_eq!(libc::pipe(parent_to_child.as_mut_ptr()), 0, "pipe 1");
        assert_eq!(libc::pipe(child_to_parent.as_mut_ptr()), 0, "pipe 2");
    }

    // SAFETY: fork is safe but the child must avoid async-signal-unsafe
    // APIs. We use only plain syscalls via libc / std::fs on an open fd.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed");

    if pid == 0 {
        // Child: close the parent-side fds, take the child-side as
        // owned File handles, invoke child_fn, exit.
        unsafe {
            libc::close(parent_to_child[1]);
            libc::close(child_to_parent[0]);
        }
        let rx_from_parent = unsafe { std::fs::File::from_raw_fd(parent_to_child[0]) };
        let tx_to_parent = unsafe { std::fs::File::from_raw_fd(child_to_parent[1]) };
        let code = child_fn(rx_from_parent, tx_to_parent);
        // Bypass any atexit handlers / rust test harness teardown.
        unsafe { libc::_exit(code) };
    }

    // Parent: close the child-side fds, wrap the parent-side.
    unsafe {
        libc::close(parent_to_child[0]);
        libc::close(child_to_parent[1]);
    }
    let tx_to_child = unsafe { std::fs::File::from_raw_fd(parent_to_child[1]) };
    let rx_from_child = unsafe { std::fs::File::from_raw_fd(child_to_parent[0]) };
    (pid, tx_to_child, rx_from_child)
}

fn wait_child(pid: libc::pid_t) -> i32 {
    let mut status: libc::c_int = 0;
    // SAFETY: waitpid on a live child pid is safe.
    let rc = unsafe { libc::waitpid(pid, &mut status, 0) };
    assert!(rc >= 0, "waitpid failed");
    if (status & 0x7f) == 0 {
        (status >> 8) & 0xff
    } else {
        -1
    }
}

fn wait_for_byte(rx: &mut std::fs::File, _timeout: Duration) {
    // Blocking read on a single byte. Tests rely on the child writing
    // its ready byte promptly after fork; if the child dies first we
    // observe EOF and fail loudly rather than wedging CI.
    let mut buf = [0u8; 1];
    rx.read_exact(&mut buf).expect("read ready byte from child");
}

#[test]
fn child_holds_flock_parent_sees_already_running_then_recovers() {
    let _g = FORK_GUARD.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let lock = tmp.path().join("clitunesd.lock");

    let lock_for_child = lock.clone();
    let (pid, mut release_tx, mut ready_rx) =
        fork_with_pipes(move |mut release_rx, mut ready_tx| {
            // Child: acquire the lock, signal ready, block on a byte.
            let outcome = match acquire_at(&lock_for_child) {
                Ok(o) => o,
                Err(_) => return 10,
            };
            let _held = match outcome {
                AcquireOutcome::Acquired(l) => l,
                AcquireOutcome::AlreadyRunning => return 11,
            };
            if ready_tx.write_all(b"R").is_err() {
                return 12;
            }
            let mut buf = [0u8; 1];
            let _ = release_rx.read_exact(&mut buf);
            0
        });

    wait_for_byte(&mut ready_rx, Duration::from_secs(5));

    // Parent observes AlreadyRunning while the child still holds it.
    let outcome = acquire_at(&lock).expect("parent acquire");
    assert!(
        matches!(outcome, AcquireOutcome::AlreadyRunning),
        "expected AlreadyRunning while child holds flock"
    );

    // Release: send a byte so the child exits.
    release_tx.write_all(b"x").expect("write release");
    drop(release_tx);
    let code = wait_child(pid);
    assert_eq!(code, 0, "child exit code");

    // Kernel released the lock on child exit.
    let after = acquire_at(&lock).expect("post-exit acquire");
    assert!(
        matches!(after, AcquireOutcome::Acquired(_)),
        "expected Acquired after child exit"
    );
}

#[test]
fn dead_child_lock_does_not_persist_across_kill() {
    let _g = FORK_GUARD.lock().unwrap_or_else(|e| e.into_inner());
    // Second check on the kernel-release guarantee: SIGKILL the child
    // so it can't run any Drop impls, and confirm the lock is still
    // released (this is what protects against daemon panics / OOMs).
    let tmp = tempfile::tempdir().unwrap();
    let lock = tmp.path().join("clitunesd.lock");

    let lock_for_child = lock.clone();
    let (pid, _release_tx, mut ready_rx) = fork_with_pipes(move |_release_rx, mut ready_tx| {
        let _held = match acquire_at(&lock_for_child).expect("acquire") {
            AcquireOutcome::Acquired(l) => l,
            AcquireOutcome::AlreadyRunning => return 11,
        };
        let _ = ready_tx.write_all(b"R");
        // Spin forever waiting for the kill.
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    });

    wait_for_byte(&mut ready_rx, Duration::from_secs(5));

    // Confirm contention first.
    let during = acquire_at(&lock).expect("parent acquire");
    assert!(matches!(during, AcquireOutcome::AlreadyRunning));

    // SAFETY: SIGKILL on a child we just forked is safe.
    unsafe { libc::kill(pid, libc::SIGKILL) };
    let _ = wait_child(pid);

    let after = acquire_at(&lock).expect("post-kill acquire");
    assert!(
        matches!(after, AcquireOutcome::Acquired(_)),
        "expected Acquired after SIGKILL"
    );
}
