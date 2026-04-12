//! flock-based singleton enforcement for clitunesd.
//!
//! A daemon startup acquires an exclusive, non-blocking [`libc::flock`] on
//! `$runtime_dir/clitunesd.lock`. If another daemon is already running, the
//! flock fails with `EWOULDBLOCK` and the new process exits 0 silently — this
//! is the expected "something already handled it" path for auto-spawn.
//!
//! Unlike pidfile locking, flock is released by the kernel on process death
//! (Linux and macOS both guarantee this), so a crashed daemon cannot leave a
//! stale lock. The file itself persists but its lock state does not.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use super::lifecycle::{open_private, runtime_dir};

/// A held flock. Dropping it closes the fd, which causes the kernel to
/// release the lock. The file on disk stays, but the lock state does not,
/// so the next daemon start observes a free lock.
#[derive(Debug)]
pub struct DaemonLock {
    _file: File,
    path: PathBuf,
}

impl DaemonLock {
    /// Path of the lockfile we are holding.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Outcome of a lock acquisition attempt.
#[derive(Debug)]
pub enum AcquireOutcome {
    /// We now hold the lock and should continue booting the daemon.
    Acquired(DaemonLock),
    /// Another daemon is already running. Caller should exit 0 silently.
    AlreadyRunning,
}

/// Try to acquire the clitunesd singleton lock at the default location
/// (`runtime_dir()/clitunesd.lock`).
pub fn acquire_default() -> io::Result<AcquireOutcome> {
    let dir = runtime_dir()?;
    let lock_path = dir.join("clitunesd.lock");
    acquire_at(&lock_path)
}

/// Try to acquire the singleton lock at an explicit path. Used by tests
/// and by callers that want to live outside `$XDG_RUNTIME_DIR` (e.g. a
/// custom sandbox).
pub fn acquire_at(lock_path: &Path) -> io::Result<AcquireOutcome> {
    let file = open_private(lock_path)?;
    // SAFETY: flock on a valid open fd is safe; we own the file.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(AcquireOutcome::Acquired(DaemonLock {
            _file: file,
            path: lock_path.to_path_buf(),
        }));
    }
    let err = io::Error::last_os_error();
    // EWOULDBLOCK and EAGAIN are the same value on Linux/macOS, so one arm suffices.
    match err.raw_os_error() {
        Some(libc::EWOULDBLOCK) => Ok(AcquireOutcome::AlreadyRunning),
        _ => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_acquire_wins_second_observes_already_running() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("clitunesd.lock");

        let first = acquire_at(&path).unwrap();
        assert!(matches!(first, AcquireOutcome::Acquired(_)));

        let second = acquire_at(&path).unwrap();
        assert!(
            matches!(second, AcquireOutcome::AlreadyRunning),
            "second flock must fail while first is held"
        );
    }

    #[test]
    fn release_on_drop_allows_reacquire() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("clitunesd.lock");

        {
            let held = acquire_at(&path).unwrap();
            assert!(matches!(held, AcquireOutcome::Acquired(_)));
            // held drops here, closing the fd and releasing the flock.
        }

        let again = acquire_at(&path).unwrap();
        assert!(
            matches!(again, AcquireOutcome::Acquired(_)),
            "post-drop reacquisition must succeed"
        );
    }

    #[test]
    fn acquired_reports_lock_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("clitunesd.lock");
        match acquire_at(&path).unwrap() {
            AcquireOutcome::Acquired(lock) => assert_eq!(lock.path(), path),
            AcquireOutcome::AlreadyRunning => panic!("unexpected already-running"),
        }
    }
}
