//! `state.toml` — the small, private file that remembers what the user
//! was last listening to so clitunes can auto-resume on next launch.
//!
//! # Why a separate file from `config.toml`
//!
//! `config.toml` holds user preferences (palettes, keybinds, layouts)
//! that the user edits by hand. `state.toml` holds **ephemeral runtime
//! state** that clitunes itself rewrites constantly (last station
//! played, last visualiser chosen). Keeping them separate means:
//!
//! - A user who hand-edits `config.toml` never races clitunes's writer.
//! - Blowing away `state.toml` to "reset" never loses config.
//! - The picker's first-run detection is a simple existence check on
//!   `state.toml`, independent of whether `config.toml` is present.
//!
//! # Security posture
//!
//! `state.toml` contains listening history (station UUIDs and tomorrow
//! potentially track names) — nothing catastrophic, but not something
//! we want other local users to read on a shared machine. The write
//! path enforces:
//!
//! - Parent dir `~/.config/clitunes/` exists with mode **0700** (created
//!   if absent).
//! - The final file and the intermediate temp file are both chmod'd to
//!   **0600** before persist, so even a crash between `create` and
//!   `persist` never leaves a world-readable temp artifact.
//!
//! Resolves SEC-011 from the round-2 document review.
//!
//! # Crash safety
//!
//! Writes are atomic: we create a `NamedTempFile` in the **same
//! directory** as the destination (so `persist` is a same-filesystem
//! `rename(2)`, not a copy), chmod both the temp and the destination,
//! write + flush + sync, then rename. A crash at any point leaves
//! either the old file or the new file intact — never a truncated one.
//!
//! # Corruption recovery
//!
//! If `state.toml` exists but fails to parse, [`load_state`] returns
//! `Ok(Recovery::Corrupt)` so the caller can log a warning, delete the
//! file, and fall through to the first-run picker flow. That matches
//! the plan's edge case: "state.toml is corrupt TOML → code logs a
//! warning, deletes it, shows the picker."

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

/// Source selected on the last run. Stringly-typed on disk so adding a
/// new source variant in a future release doesn't break deserialisation
/// of old files — unknown values are handled at load time by the
/// caller. The main binary maps this back to `SourceChoice` itself.
pub const SOURCE_TONE: &str = "tone";
pub const SOURCE_RADIO: &str = "radio";
pub const SOURCE_SPOTIFY: &str = "spotify";

/// Everything clitunes remembers between runs.
///
/// All fields are `Option` so a partially-populated file (e.g. an old
/// version that didn't know about `last_layout` yet) round-trips
/// cleanly: `serde(default)` fills missing fields with `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// UUID of the last radio station the user played. `None` means
    /// "first run, or the user has never played radio yet" — the
    /// picker overlay will show on next launch.
    pub last_station_uuid: Option<String>,

    /// Human-readable station name, cached so the first frame on
    /// resume has something to display before the HTTP headers for the
    /// stream come back. Sanitized by `StationDb` before it gets here
    /// so there's nothing to re-escape.
    pub last_station_name: Option<String>,

    /// `"tone"` or `"radio"`. Strings rather than an enum for
    /// forward-compat; see [`SOURCE_TONE`] / [`SOURCE_RADIO`].
    pub last_source: Option<String>,

    /// Visualiser id (e.g. `"auralis"`, `"plasma"`). Loose strings
    /// again so a future rename doesn't silently wipe the user's pick.
    pub last_visualiser: Option<String>,

    /// Layout id from Slice 3. Present now so Unit 8 + Unit 16 don't
    /// need a schema migration.
    pub last_layout: Option<String>,

    /// Spotify URI last played (e.g. `spotify:track:4PTG3Z6ehGkBFwjybzWkR8`).
    /// Stored so auto-resume can restart Spotify playback on next launch.
    pub last_spotify_uri: Option<String>,
}

impl State {
    /// Convenience constructor for callers that only want to record
    /// "the user just picked this station".
    pub fn with_station(uuid: impl Into<String>, name: Option<String>) -> Self {
        Self {
            last_station_uuid: Some(uuid.into()),
            last_station_name: name,
            last_source: Some(SOURCE_RADIO.into()),
            ..Self::default()
        }
    }

    /// Convenience constructor for callers that only want to record
    /// "the user just played this Spotify URI".
    pub fn with_spotify(uri: impl Into<String>) -> Self {
        Self {
            last_spotify_uri: Some(uri.into()),
            last_source: Some(SOURCE_SPOTIFY.into()),
            ..Self::default()
        }
    }

    /// True when there's nothing worth persisting — we skip writes on
    /// an empty state so a ToneSource-only run doesn't create a
    /// zero-content file.
    pub fn is_empty(&self) -> bool {
        self.last_station_uuid.is_none()
            && self.last_station_name.is_none()
            && self.last_source.is_none()
            && self.last_visualiser.is_none()
            && self.last_layout.is_none()
            && self.last_spotify_uri.is_none()
    }
}

/// Result of attempting to read `state.toml`.
#[derive(Debug)]
pub enum Recovery {
    /// File parsed cleanly.
    Loaded(State),
    /// File does not exist — normal first-run.
    Missing,
    /// File exists but failed to parse. The caller should log a
    /// warning, delete the file, and fall through to first-run flow.
    /// The raw parse error is included for logs.
    Corrupt(String),
}

/// Default state file path: `$XDG_CONFIG_HOME/clitunes/state.toml` on
/// Linux, `~/Library/Application Support/clitunes/state.toml` on macOS,
/// both via the `dirs` crate. Returns `None` if the OS has no config
/// dir (e.g. extremely stripped-down CI containers) — the caller
/// should treat that as "disable persistence this run" rather than
/// crashing.
pub fn default_state_path() -> Option<PathBuf> {
    dirs::config_dir().map(|base| base.join("clitunes").join("state.toml"))
}

/// Read `state.toml` from `path`. Never panics on malformed TOML — it
/// returns [`Recovery::Corrupt`] so the caller can fall back to the
/// first-run picker.
pub fn load_state(path: &Path) -> Result<Recovery> {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Recovery::Missing),
        Err(e) => {
            return Err(e).with_context(|| format!("reading state file {}", path.display()));
        }
    };
    match toml::from_str::<State>(&raw) {
        Ok(state) => Ok(Recovery::Loaded(state)),
        Err(e) => Ok(Recovery::Corrupt(e.to_string())),
    }
}

/// Write `state` to `path` atomically with 0600 perms. Creates the
/// parent directory with 0700 if it doesn't exist.
///
/// Rationale for each step:
///
/// 1. **Parent dir mode 0700** — anything under `~/.config/clitunes/`
///    should be user-private; a newly created dir would otherwise
///    inherit the process umask, which is often 022 (world-readable).
/// 2. **Temp file in the same dir** — `NamedTempFile::new_in(parent)`
///    guarantees `persist()` is a `rename(2)` on the same filesystem,
///    which is atomic. Using `std::env::temp_dir()` would risk a
///    cross-device copy + unlink, which is not atomic.
/// 3. **Chmod 0600 before persist** — set on the temp file before the
///    rename, so the final file's perms are right from the instant it
///    becomes visible. Doing it after persist would leave a brief
///    window with default perms.
/// 4. **Flush + sync_all** — ensures the bytes are on disk before the
///    rename makes them visible, so a power loss between write and
///    persist can't yield a zero-length `state.toml`.
pub fn save_state(state: &State, path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("state path has no parent: {}", path.display()))?;
    ensure_parent_dir(parent)?;

    let serialized = toml::to_string_pretty(state).context("serialize state to toml")?;

    let mut tmp = NamedTempFile::new_in(parent)
        .with_context(|| format!("creating temp file in {}", parent.display()))?;

    // Tighten the temp file's perms before we write anything sensitive.
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o600))
        .context("chmod 0600 state tempfile")?;

    tmp.write_all(serialized.as_bytes())
        .context("write state bytes")?;
    tmp.as_file().sync_all().context("fsync state tempfile")?;

    tmp.persist(path)
        .map_err(|e| anyhow::anyhow!("persist state file: {}", e))?;

    // Belt-and-braces: `persist` preserves the temp perms on Linux/macOS,
    // but reassert 0600 in case a filesystem quirk reset them.
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .context("chmod 0600 state file")?;
    Ok(())
}

/// Create `dir` (and any missing parents) with mode 0700. If it
/// already exists, only re-apply 0700 when the current mode is more
/// permissive than 0700 — we don't want to fight a user who has
/// deliberately tightened it to 0500 or 0600.
fn ensure_parent_dir(dir: &Path) -> Result<()> {
    if !dir.exists() {
        fs::create_dir_all(dir)
            .with_context(|| format!("creating state parent dir {}", dir.display()))?;
    }
    let meta =
        fs::metadata(dir).with_context(|| format!("stat state parent dir {}", dir.display()))?;
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        // Group or other bits set — tighten to 0700.
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("chmod 0700 {}", dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn roundtrip_happy_path() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join("state.toml");

        let state = State {
            last_station_uuid: Some("abc-123".into()),
            last_station_name: Some("SomaFM Groove Salad".into()),
            last_source: Some(SOURCE_RADIO.into()),
            last_visualiser: Some("auralis".into()),
            last_layout: Some("default".into()),
            last_spotify_uri: None,
        };
        save_state(&state, &path).unwrap();

        match load_state(&path).unwrap() {
            Recovery::Loaded(loaded) => assert_eq!(loaded, state),
            other => panic!("expected Loaded, got {:?}", other),
        }
    }

    #[test]
    fn missing_file_reports_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nope.toml");
        match load_state(&path).unwrap() {
            Recovery::Missing => {}
            other => panic!("expected Missing, got {:?}", other),
        }
    }

    #[test]
    fn corrupt_file_reports_corrupt() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.toml");
        fs::write(&path, "this is [not valid\ntoml at all").unwrap();
        match load_state(&path).unwrap() {
            Recovery::Corrupt(_) => {}
            other => panic!("expected Corrupt, got {:?}", other),
        }
    }

    #[test]
    fn partial_state_backfills_with_none() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.toml");
        fs::write(&path, "last_station_uuid = \"only-this\"\n").unwrap();
        match load_state(&path).unwrap() {
            Recovery::Loaded(s) => {
                assert_eq!(s.last_station_uuid.as_deref(), Some("only-this"));
                assert!(s.last_visualiser.is_none());
            }
            other => panic!("expected Loaded, got {:?}", other),
        }
    }

    #[test]
    fn persisted_file_is_mode_0600() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("clitunes").join("state.toml");
        save_state(
            &State::with_station("uuid-1", Some("Test FM".into())),
            &path,
        )
        .unwrap();

        let meta = fs::metadata(&path).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "state.toml must be chmod 0600");
    }

    #[test]
    fn parent_dir_is_mode_0700_when_created() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("clitunes").join("state.toml");
        save_state(&State::with_station("uuid-1", None), &path).unwrap();

        let parent = path.parent().unwrap();
        let meta = fs::metadata(parent).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "clitunes config dir must be chmod 0700");
    }

    #[test]
    fn loose_parent_dir_is_tightened_on_save() {
        let dir = tempdir().unwrap();
        let parent = dir.path().join("clitunes");
        fs::create_dir_all(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();

        let path = parent.join("state.toml");
        save_state(&State::with_station("u", None), &path).unwrap();

        let meta = fs::metadata(&parent).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "loose parent dir should be tightened to 0700");
    }

    #[test]
    fn overwrite_is_atomic_no_partial_file() {
        // Write once, then write again; the second write must produce
        // a valid file, not a truncated one, and content must match.
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.toml");
        save_state(&State::with_station("first", None), &path).unwrap();
        save_state(&State::with_station("second", None), &path).unwrap();

        match load_state(&path).unwrap() {
            Recovery::Loaded(s) => {
                assert_eq!(s.last_station_uuid.as_deref(), Some("second"));
            }
            other => panic!("expected Loaded, got {:?}", other),
        }
    }

    #[test]
    fn state_is_empty_reports_empty_for_default() {
        assert!(State::default().is_empty());
        assert!(!State::with_station("u", None).is_empty());
        assert!(!State::with_spotify("spotify:track:abc").is_empty());
    }

    #[test]
    fn spotify_state_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join("state.toml");

        let state = State::with_spotify("spotify:track:4PTG3Z6ehGkBFwjybzWkR8");
        save_state(&state, &path).unwrap();

        match load_state(&path).unwrap() {
            Recovery::Loaded(loaded) => {
                assert_eq!(
                    loaded.last_spotify_uri.as_deref(),
                    Some("spotify:track:4PTG3Z6ehGkBFwjybzWkR8")
                );
                assert_eq!(loaded.last_source.as_deref(), Some(SOURCE_SPOTIFY));
                assert!(loaded.last_station_uuid.is_none());
            }
            other => panic!("expected Loaded, got {:?}", other),
        }
    }

    #[test]
    fn full_state_with_spotify_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.toml");

        let state = State {
            last_station_uuid: Some("abc-123".into()),
            last_station_name: Some("SomaFM Groove Salad".into()),
            last_source: Some(SOURCE_SPOTIFY.into()),
            last_visualiser: Some("auralis".into()),
            last_layout: Some("default".into()),
            last_spotify_uri: Some("spotify:track:4PTG3Z6ehGkBFwjybzWkR8".into()),
        };
        save_state(&state, &path).unwrap();

        match load_state(&path).unwrap() {
            Recovery::Loaded(loaded) => assert_eq!(loaded, state),
            other => panic!("expected Loaded, got {:?}", other),
        }
    }

    #[test]
    fn default_state_path_under_config_dir() {
        if let Some(p) = default_state_path() {
            assert!(p.ends_with("clitunes/state.toml"));
        }
    }
}
