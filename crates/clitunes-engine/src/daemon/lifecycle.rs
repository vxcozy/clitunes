//! Daemon lifecycle: double-fork, detach from the controlling terminal,
//! redirect stdio to /dev/null, and (optionally) write an informational pidfile.
//!
//! The classic UNIX double-fork sequence ensures the daemon:
//!   1. cannot reacquire a controlling terminal (the grandchild is orphaned
//!      into pid 1 and has no session leader flag),
//!   2. is not a process-group leader, and
//!   3. detaches cleanly from the shell that launched it.
//!
//! Singleton enforcement is handled separately in [`super::singleton`]: the
//! pidfile here is purely informational and *not* a locking mechanism. Using
//! flock (which the kernel releases on process death) means a crashed daemon
//! can't leave a stale lock behind, which would be the usual failure mode of
//! pid-file-based locking.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// Result of the detach sequence. The daemon grandchild returns
/// [`DetachOutcome::Daemon`]; the original parent returns
/// [`DetachOutcome::Parent`] with the grandchild's pid so the launcher
/// can report it (or exit(0) and let the shell return immediately).
#[derive(Debug)]
pub enum DetachOutcome {
    /// We are the daemon grandchild. Caller should continue with the
    /// event loop.
    Daemon,
    /// We are the original process. Caller should exit(0) immediately so
    /// the shell returns; the grandchild keeps running.
    Parent { child_pid: i32 },
}

/// Perform the double-fork + setsid + stdio redirect dance. Returns
/// [`DetachOutcome::Parent`] in the original caller and
/// [`DetachOutcome::Daemon`] in the detached grandchild.
///
/// Safety: this calls `fork(2)` directly. The caller must not hold any
/// thread-local resources that would break on a post-fork, pre-exec child.
/// Concretely: do not spawn any threads or initialise tokio before calling
/// `detach`. This is why `clitunesd`'s main does argv parsing, then calls
/// `detach`, and *then* boots the runtime.
///
/// # Errors
/// Returns an error if any of the `fork`, `setsid`, or dup2 syscalls fail.
///
/// # Safety
/// Must be called single-threaded, before any tokio runtime or thread
/// spawn. Violating that will trigger undefined behaviour in libc.
pub unsafe fn detach() -> io::Result<DetachOutcome> {
    // First fork: the parent exits, leaving the child without a
    // controlling terminal's session leadership.
    let first = libc::fork();
    if first < 0 {
        return Err(io::Error::last_os_error());
    }
    if first > 0 {
        // Original parent returns so the launcher can exit(0).
        return Ok(DetachOutcome::Parent { child_pid: first });
    }

    // First child: detach from the controlling tty by starting a new
    // session. After setsid, we're a session leader — a second fork
    // drops that so we can never reacquire a tty.
    if libc::setsid() < 0 {
        return Err(io::Error::last_os_error());
    }

    let second = libc::fork();
    if second < 0 {
        return Err(io::Error::last_os_error());
    }
    if second > 0 {
        // First child exits; the grandchild is reparented to init.
        libc::_exit(0);
    }

    // Grandchild: we are the daemon. Clean the file-creation mask so
    // files created by the daemon have predictable permissions. The
    // caller sets a stricter umask right before socket binds (SEC-001).
    libc::umask(0);

    redirect_stdio_to_devnull()?;

    Ok(DetachOutcome::Daemon)
}

/// Open /dev/null and dup2 it over stdin, stdout, stderr. Used by the
/// detach path and by any test that wants to verify the sequence in
/// isolation. Exposed for testing.
pub fn redirect_stdio_to_devnull() -> io::Result<()> {
    let devnull = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")?;
    let fd = devnull.as_raw_fd();
    // SAFETY: dup2 is always safe; the source fd is open (we just
    // opened it) and the destination fds 0/1/2 are valid targets.
    unsafe {
        if libc::dup2(fd, libc::STDIN_FILENO) < 0
            || libc::dup2(fd, libc::STDOUT_FILENO) < 0
            || libc::dup2(fd, libc::STDERR_FILENO) < 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    // `devnull` drops here, closing the extra fd — the dup2'd copies
    // keep the underlying description alive.
    Ok(())
}

/// Write an informational pidfile at `path`. Best-effort: a failure
/// here is logged by the caller but never fatal, because the real
/// singleton enforcement is the flock in [`super::singleton`].
pub fn write_pidfile(path: &Path, pid: i32) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    writeln!(f, "{pid}")?;
    f.sync_all()?;
    Ok(())
}

/// Resolve the daemon's runtime directory. Tries `$XDG_RUNTIME_DIR/clitunes`
/// first (Linux, systemd-managed), then falls back to `$TMPDIR/$USER/clitunes`
/// (macOS, which has no XDG_RUNTIME_DIR by default). The directory is created
/// with mode 0700 if it doesn't exist so socket inodes inside it inherit
/// a private-by-default posture.
pub fn runtime_dir() -> io::Result<PathBuf> {
    let base = if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        PathBuf::from(dir)
    } else {
        let tmp = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let user = std::env::var_os("USER")
            .map(|u| u.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("uid-{}", unsafe { libc::geteuid() }));
        tmp.join(user)
    };
    let dir = base.join("clitunes");
    ensure_dir_700(&dir)?;
    Ok(dir)
}

/// Create `dir` (and parents) if missing, then chmod it to 0700. On a
/// multi-user machine this is what prevents another user from racing
/// the daemon on the socket path.
fn ensure_dir_700(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    // SAFETY: chmod on a path we just created; mode 0o700 is a constant.
    let rc = unsafe { libc::chmod(c.as_ptr(), 0o700) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Set the process umask to 0o177 so that any file created by the daemon
/// (including the AF_UNIX socket inode) is mode 0600 atomically. This
/// closes the SEC-001 TOCTOU between `bind(2)` and a follow-up chmod.
/// Returns the previous umask so the caller can restore it if desired.
pub fn set_socket_umask() -> libc::mode_t {
    // SAFETY: umask is always safe; the return value is the prior mask.
    unsafe { libc::umask(0o177) }
}

/// Open (or create) a file for exclusive use, chmod 0600. Used by both
/// the lockfile and the pidfile so nobody else on the box can read them.
pub fn open_private(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let f = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    // SAFETY: chmod on a path the caller owns; mode is a constant.
    let rc = unsafe { libc::chmod(c.as_ptr(), 0o600) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialise tests that mutate process-global env vars. cargo test
    // runs test fns on multiple threads by default, and concurrent
    // setenv / getenv is undefined behaviour on most libc's.
    static ENV_GUARD: Mutex<()> = Mutex::new(());

    #[test]
    fn runtime_dir_uses_xdg_when_set() {
        let _g = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let prior_xdg = std::env::var_os("XDG_RUNTIME_DIR");
        std::env::set_var("XDG_RUNTIME_DIR", tmp.path());
        let dir = runtime_dir().unwrap();
        assert_eq!(dir, tmp.path().join("clitunes"));
        assert!(dir.exists());
        if let Some(v) = prior_xdg {
            std::env::set_var("XDG_RUNTIME_DIR", v);
        } else {
            std::env::remove_var("XDG_RUNTIME_DIR");
        }
    }

    #[test]
    fn runtime_dir_falls_back_to_tmp() {
        let _g = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let prior_xdg = std::env::var_os("XDG_RUNTIME_DIR");
        let prior_tmpdir = std::env::var_os("TMPDIR");
        let prior_user = std::env::var_os("USER");
        std::env::remove_var("XDG_RUNTIME_DIR");
        std::env::set_var("TMPDIR", tmp.path());
        std::env::set_var("USER", "lifecycle-fallback");
        let dir = runtime_dir().unwrap();
        assert_eq!(dir, tmp.path().join("lifecycle-fallback").join("clitunes"));
        assert!(dir.exists());
        if let Some(v) = prior_xdg {
            std::env::set_var("XDG_RUNTIME_DIR", v);
        }
        if let Some(v) = prior_tmpdir {
            std::env::set_var("TMPDIR", v);
        } else {
            std::env::remove_var("TMPDIR");
        }
        if let Some(v) = prior_user {
            std::env::set_var("USER", v);
        } else {
            std::env::remove_var("USER");
        }
    }

    #[test]
    fn pidfile_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("daemon.pid");
        write_pidfile(&p, 12345).unwrap();
        let contents = std::fs::read_to_string(&p).unwrap();
        assert_eq!(contents.trim(), "12345");
    }

    #[test]
    fn open_private_is_chmod_600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("private.file");
        let _f = open_private(&p).unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600 got {mode:o}");
    }

    #[test]
    fn set_socket_umask_returns_previous() {
        // SAFETY: umask(2) is idempotent; we restore the original mask.
        let prev = set_socket_umask();
        let current = unsafe { libc::umask(prev) };
        assert_eq!(current, 0o177, "expected our mask back, got {current:o}");
    }
}
