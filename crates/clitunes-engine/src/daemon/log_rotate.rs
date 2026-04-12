//! Size-based log rotation for `~/.cache/clitunes/clitunesd.log`.
//!
//! The daemon uses this as the writer backing its tracing subscriber.
//! On every write we check the current file size; when it exceeds the
//! configured threshold we shift `clitunesd.log.N` → `clitunesd.log.N+1`
//! (dropping anything past the max backup count) and truncate the main
//! log to zero.
//!
//! Rotation is size-triggered rather than time-triggered because the
//! daemon is expected to be long-running and we care about bounding the
//! disk footprint, not aligning files to wall-clock boundaries.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Default rotation threshold: 10 MiB matches the bead spec.
pub const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// Default number of rotated backups to keep.
pub const DEFAULT_MAX_BACKUPS: usize = 5;

/// Thread-safe rotating log writer.
pub struct RotatingLog {
    inner: Mutex<Inner>,
}

struct Inner {
    path: PathBuf,
    file: File,
    written: u64,
    max_bytes: u64,
    max_backups: usize,
}

impl RotatingLog {
    /// Open (or create) `path` with default size/backup caps.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        Self::open_with(path, DEFAULT_MAX_BYTES, DEFAULT_MAX_BACKUPS)
    }

    /// Open (or create) `path` with explicit caps. `max_bytes = 0`
    /// disables rotation (test-only escape hatch).
    pub fn open_with(
        path: impl Into<PathBuf>,
        max_bytes: u64,
        max_backups: usize,
    ) -> io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(false)
            .open(&path)?;
        let written = file.metadata()?.len();
        Ok(Self {
            inner: Mutex::new(Inner {
                path,
                file,
                written,
                max_bytes,
                max_backups,
            }),
        })
    }

    /// Active log path (test introspection).
    pub fn path(&self) -> PathBuf {
        self.inner.lock().unwrap().path.clone()
    }
}

impl Write for &RotatingLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut g = self.inner.lock().unwrap();
        if g.max_bytes > 0 && g.written + buf.len() as u64 >= g.max_bytes {
            g.rotate()?;
        }
        let n = g.file.write(buf)?;
        g.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.lock().unwrap().file.flush()
    }
}

impl Write for RotatingLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&*self).flush()
    }
}

/// `tracing_subscriber::fmt` expects a `MakeWriter`. We implement it on
/// `&'static RotatingLog` so the daemon can `Box::leak` a single
/// rotating handle and pass the reference into the fmt layer. Tests use
/// the `Write` impl on the owned value directly.
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for &'static RotatingLog {
    type Writer = &'static RotatingLog;

    fn make_writer(&'a self) -> Self::Writer {
        *self
    }
}

impl Inner {
    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        // Sync data to disk before renaming — on CI overlay filesystems
        // (overlayfs/AUFS) a rename can race with buffered data visibility.
        self.file.sync_all()?;
        drop_and_recreate(&mut self.file, &self.path, self.max_backups)?;
        self.written = 0;
        Ok(())
    }
}

fn drop_and_recreate(file: &mut File, path: &Path, max_backups: usize) -> io::Result<()> {
    // Shift existing backups upward: clitunesd.log.(N-1) → .N, then
    // .N-2 → .N-1 ... then main log → .1. Anything past `max_backups`
    // gets unlinked.
    if max_backups > 0 {
        let oldest = backup_path(path, max_backups);
        if oldest.exists() {
            std::fs::remove_file(&oldest)?;
        }
        for i in (1..max_backups).rev() {
            let src = backup_path(path, i);
            let dst = backup_path(path, i + 1);
            if src.exists() {
                std::fs::rename(&src, &dst)?;
            }
        }
        let first_backup = backup_path(path, 1);
        if path.exists() {
            std::fs::rename(path, &first_backup)?;
        }
    } else if path.exists() {
        // max_backups == 0: just drop the main log.
        std::fs::remove_file(path)?;
    }
    // Reopen a fresh empty main log. Re-ensure parent dir exists in case
    // the filesystem lost it (observed as a flake on CI overlay filesystems).
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    *file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(false)
        .open(path)?;
    Ok(())
}

fn backup_path(main: &Path, n: usize) -> PathBuf {
    let mut s = main.as_os_str().to_os_string();
    s.push(format!(".{n}"));
    PathBuf::from(s)
}

/// Resolve the default daemon log path: `~/.cache/clitunes/clitunesd.log`.
/// Falls back to `/tmp/clitunes-$uid/clitunesd.log` if the home cache dir
/// isn't available (e.g. `$HOME` is unset in a minimal container).
pub fn default_log_path() -> PathBuf {
    if let Some(dir) = dirs::cache_dir() {
        return dir.join("clitunes").join("clitunesd.log");
    }
    let uid = unsafe { libc::geteuid() };
    PathBuf::from(format!("/tmp/clitunes-{uid}/clitunesd.log"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_once_threshold_exceeded() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("clitunesd.log");
        let log = RotatingLog::open_with(&path, 16, 3).unwrap();
        let mut w: &RotatingLog = &log;
        w.write_all(b"aaaaaaaa").unwrap(); // 8 bytes, under threshold
        w.write_all(b"bbbbbbbb").unwrap(); // would hit 16, triggers rotate
        w.write_all(b"cc").unwrap();
        w.flush().unwrap();

        let backup = backup_path(&path, 1);
        assert!(
            backup.exists(),
            "clitunesd.log.1 should exist after rotation"
        );
        let backup_contents = std::fs::read(&backup).unwrap();
        // First 8 bytes landed, then rotation fired before the second
        // 8-byte write, so the backup has the first batch.
        assert_eq!(backup_contents, b"aaaaaaaa");

        let main_contents = std::fs::read(&path).unwrap();
        assert_eq!(main_contents, b"bbbbbbbbcc");
    }

    #[test]
    fn rotation_respects_max_backups() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("clitunesd.log");
        let log = RotatingLog::open_with(&path, 4, 2).unwrap();
        let mut w: &RotatingLog = &log;
        // Force 4 rotations.
        for _ in 0..5 {
            w.write_all(b"xxxx").unwrap();
        }
        w.flush().unwrap();
        let b1 = backup_path(&path, 1);
        let b2 = backup_path(&path, 2);
        let b3 = backup_path(&path, 3);
        assert!(b1.exists());
        assert!(b2.exists());
        assert!(!b3.exists(), "max_backups=2 means .3 must never exist");
    }

    #[test]
    fn zero_max_bytes_disables_rotation() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("clitunesd.log");
        let log = RotatingLog::open_with(&path, 0, 5).unwrap();
        let mut w: &RotatingLog = &log;
        for _ in 0..100 {
            w.write_all(b"hello\n").unwrap();
        }
        w.flush().unwrap();
        assert!(!backup_path(&path, 1).exists());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 600);
    }

    #[test]
    fn default_log_path_uses_cache_dir() {
        // Don't assert on OS specifics; just sanity-check it ends in
        // clitunes/clitunesd.log so upgrades can't silently move it.
        let p = default_log_path();
        let s = p.to_string_lossy();
        assert!(
            s.ends_with("clitunes/clitunesd.log"),
            "unexpected path: {s}"
        );
    }
}
