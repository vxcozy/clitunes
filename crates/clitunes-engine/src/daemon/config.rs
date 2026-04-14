//! Daemon configuration loaded from `daemon.toml`.
//!
//! The daemon reads a single TOML file on startup. Today only the
//! `[connect]` section is populated (for the Spotify Connect receiver
//! landing in units 2–4). The shape is additive: new sections can be
//! added later without breaking existing files, because every field at
//! every level is `#[serde(default)]` and missing or empty files
//! resolve to `DaemonConfig::default()`.
//!
//! ## Resolution order
//!
//! 1. An explicit path (passed in by a test, or plumbed through a CLI
//!    flag in a later unit) wins outright.
//! 2. `$CLITUNES_CONFIG` — allows users to redirect the file without
//!    symlinking.
//! 3. `<config_dir>/clitunes/daemon.toml` — e.g.
//!    `~/.config/clitunes/daemon.toml` on Linux/macOS.
//!
//! A missing file is not an error. It resolves to defaults so the
//! daemon boots with no config on a fresh install. A *malformed* file
//! is a hard error — we'd rather surface a typo than silently drop a
//! user's intent.
//!
//! ## Testability
//!
//! Mirroring the pattern in [`super::lifecycle::runtime_dir_from`], the
//! pure resolver [`resolve_config_path`] takes env values and the
//! default-dir result as explicit parameters so tests never mutate
//! process-global env. Parallel `cargo test --workspace` runs are
//! notorious for flaking on env mutation; keep it off-limits.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// Environment variable name users can set to override the config path.
pub const CONFIG_PATH_ENV: &str = "CLITUNES_CONFIG";

/// Root config document. Every field is defaulted so a missing file or
/// a file containing only `[connect]` still parses.
///
/// Note: `deny_unknown_fields` is intentionally *not* applied here. The
/// root document is designed to be additive — later units introduce new
/// top-level sections (`[audio]`, `[sources]`, …) and a daemon running
/// an older binary must still start against a newer config file.
/// Typo-catching still applies inside each section (see
/// [`ConnectConfig`]).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DaemonConfig {
    pub connect: ConnectConfig,
}

/// Spotify Connect receiver settings.
///
/// The receiver is disabled by default — enabling it opens a service
/// on the local network (see `bind`). We never surface a LAN-visible
/// Spotify endpoint without the user's explicit consent.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ConnectConfig {
    /// Master switch. `false` keeps the receiver off entirely.
    pub enabled: bool,

    /// Device name shown in Spotify Connect pickers. Defaults to the
    /// application name ("clitunes") — a neutral identifier, not a
    /// curated label. Users rename via the config file.
    pub name: String,

    /// Which interfaces to bind mDNS / HTTP discovery on.
    ///
    /// - `Loopback` (default): the receiver is reachable only from the
    ///   same machine. Safe default for anyone who enables Connect to
    ///   try it out before widening.
    /// - `All`: bind every interface. Required for phones/tablets to
    ///   discover the receiver over the LAN.
    pub bind: BindMode,

    /// TCP port for the `/addUser` discovery HTTP endpoint. `0` asks
    /// the OS to pick an ephemeral port (typical for Spotify Connect
    /// receivers; the port is advertised via mDNS).
    pub port: u16,

    /// Volume the receiver announces on startup, as a 0–100 percentage
    /// of Spotify's 16-bit range. `50` is librespot's own default and a
    /// sane mid-point.
    pub initial_volume: u8,

    /// Device type hint shown in Spotify clients (speaker icon, phone
    /// icon, …). `"speaker"` is librespot's default and matches how
    /// most users will think about clitunes.
    pub device_type: String,
}

impl Default for ConnectConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            name: "clitunes".into(),
            bind: BindMode::Loopback,
            port: 0,
            initial_volume: 50,
            device_type: "speaker".into(),
        }
    }
}

/// Network exposure choice for the Spotify Connect discovery service.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BindMode {
    /// Bind only `127.0.0.1`. Default — the receiver stays local.
    Loopback,
    /// Bind every interface. Required for LAN discovery from phones.
    All,
}

impl DaemonConfig {
    /// Load configuration, resolving `explicit_path` → `$CLITUNES_CONFIG`
    /// → `<config_dir>/clitunes/daemon.toml`. A missing file at the
    /// resolved path returns defaults; a malformed file is an error.
    ///
    /// # Examples
    ///
    /// Parse a config embedded in a doctest (no filesystem touched):
    ///
    /// ```
    /// use clitunes_engine::daemon::{DaemonConfig, BindMode};
    /// let cfg: DaemonConfig = toml::from_str(r#"
    ///     [connect]
    ///     enabled = true
    ///     name = "Living Room"
    ///     bind = "all"
    /// "#).unwrap();
    /// assert!(cfg.connect.enabled);
    /// assert_eq!(cfg.connect.name, "Living Room");
    /// assert_eq!(cfg.connect.bind, BindMode::All);
    /// // Unspecified fields fall back to defaults.
    /// assert_eq!(cfg.connect.initial_volume, 50);
    /// ```
    pub fn load(explicit_path: Option<&Path>) -> Result<Self> {
        let resolved = resolve_config_path(
            explicit_path,
            std::env::var_os(CONFIG_PATH_ENV),
            default_config_path(),
        );

        let Some(path) = resolved else {
            // No HOME / no config dir available: treat as "no config".
            return Ok(Self::default());
        };

        Self::load_from(&path)
    }

    /// Load a specific file path. A missing file returns defaults; an
    /// empty file returns defaults; a malformed file is an error.
    /// Exposed for tests and for callers that want to skip env
    /// resolution entirely.
    pub fn load_from(path: &Path) -> Result<Self> {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(e).with_context(|| format!("read {}", path.display()));
            }
        };

        if raw.trim().is_empty() {
            return Ok(Self::default());
        }

        let parsed: Self =
            toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
        parsed
            .validate()
            .with_context(|| format!("validate {}", path.display()))?;
        Ok(parsed)
    }

    /// Range-check fields whose types are wider than the domain allows.
    /// TOML has no subrange types, so e.g. `initial_volume: u8` accepts
    /// 0–255 at parse time even though the domain is 0–100. We want
    /// loud failure at load, not silent misbehaviour deep inside
    /// librespot at runtime.
    fn validate(&self) -> Result<()> {
        if self.connect.initial_volume > 100 {
            bail!(
                "connect.initial_volume must be 0..=100 (got {})",
                self.connect.initial_volume
            );
        }
        Ok(())
    }
}

/// Pure path resolver. Takes env values and the default-path candidate
/// as parameters so tests exercise the priority order without touching
/// process-global env. Priority: explicit (CLI flag, test input) →
/// `$CLITUNES_CONFIG` → platform config dir.
pub fn resolve_config_path(
    explicit: Option<&Path>,
    env_value: Option<OsString>,
    default: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(PathBuf::from(p));
    }
    if let Some(raw) = env_value {
        let trimmed = raw.to_string_lossy().trim().to_owned();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    default
}

/// Default config path: `<config_dir>/clitunes/daemon.toml`. Returns
/// `None` on platforms where `dirs` cannot resolve a config directory
/// (extremely rare on the platforms we target, but not worth panicking
/// over — we just fall back to defaults).
pub fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("clitunes").join("daemon.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn missing_file_returns_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("does-not-exist.toml");
        let cfg = DaemonConfig::load_from(&path).unwrap();
        assert_eq!(cfg, DaemonConfig::default());
    }

    #[test]
    fn empty_file_returns_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(tmp.path(), "daemon.toml", "");
        let cfg = DaemonConfig::load_from(&path).unwrap();
        assert_eq!(cfg, DaemonConfig::default());
    }

    #[test]
    fn whitespace_only_file_returns_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(tmp.path(), "daemon.toml", "\n   \n\t\n");
        let cfg = DaemonConfig::load_from(&path).unwrap();
        assert_eq!(cfg, DaemonConfig::default());
    }

    #[test]
    fn full_connect_section_parses() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            tmp.path(),
            "daemon.toml",
            r#"
                [connect]
                enabled = true
                name = "Kitchen"
                bind = "all"
                port = 4070
                initial_volume = 80
                device_type = "computer"
            "#,
        );
        let cfg = DaemonConfig::load_from(&path).unwrap();
        assert!(cfg.connect.enabled);
        assert_eq!(cfg.connect.name, "Kitchen");
        assert_eq!(cfg.connect.bind, BindMode::All);
        assert_eq!(cfg.connect.port, 4070);
        assert_eq!(cfg.connect.initial_volume, 80);
        assert_eq!(cfg.connect.device_type, "computer");
    }

    #[test]
    fn partial_connect_section_fills_in_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            tmp.path(),
            "daemon.toml",
            r#"
                [connect]
                enabled = true
            "#,
        );
        let cfg = DaemonConfig::load_from(&path).unwrap();
        assert!(cfg.connect.enabled);
        // Untouched fields fall back to ConnectConfig::default().
        assert_eq!(cfg.connect.name, "clitunes");
        assert_eq!(cfg.connect.bind, BindMode::Loopback);
        assert_eq!(cfg.connect.initial_volume, 50);
    }

    #[test]
    fn malformed_toml_is_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(tmp.path(), "daemon.toml", "this is = not valid = toml =");
        let err = DaemonConfig::load_from(&path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("parse"), "unexpected error: {msg}");
    }

    #[test]
    fn unknown_field_is_error() {
        // deny_unknown_fields catches typos — better to fail loud than
        // drop a misnamed setting on the floor.
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            tmp.path(),
            "daemon.toml",
            r#"
                [connect]
                enabld = true
            "#,
        );
        let err = DaemonConfig::load_from(&path).unwrap_err();
        assert!(format!("{err:#}").to_lowercase().contains("unknown"));
    }

    #[test]
    fn bind_mode_rejects_unknown_variant() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            tmp.path(),
            "daemon.toml",
            r#"
                [connect]
                bind = "wifi"
            "#,
        );
        assert!(DaemonConfig::load_from(&path).is_err());
    }

    #[test]
    fn load_with_explicit_path_bypasses_env() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            tmp.path(),
            "daemon.toml",
            r#"
                [connect]
                name = "Explicit"
            "#,
        );
        let cfg = DaemonConfig::load(Some(&path)).unwrap();
        assert_eq!(cfg.connect.name, "Explicit");
    }

    #[test]
    fn default_config_path_ends_with_expected_suffix() {
        if let Some(p) = default_config_path() {
            let s = p.to_string_lossy();
            assert!(s.ends_with("clitunes/daemon.toml"), "got {s}");
        }
    }

    #[test]
    fn unknown_root_section_is_accepted() {
        // The root document must stay forward-compatible — a daemon on
        // an older binary should not crash because a newer config file
        // mentions a section the old binary doesn't know about.
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            tmp.path(),
            "daemon.toml",
            r#"
                [connect]
                enabled = true

                [future_section]
                something = "new"
            "#,
        );
        let cfg = DaemonConfig::load_from(&path).unwrap();
        assert!(cfg.connect.enabled);
    }

    #[test]
    fn initial_volume_out_of_range_is_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            tmp.path(),
            "daemon.toml",
            r#"
                [connect]
                initial_volume = 150
            "#,
        );
        let err = DaemonConfig::load_from(&path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("initial_volume") && msg.contains("0..=100"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn initial_volume_100_is_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            tmp.path(),
            "daemon.toml",
            r#"
                [connect]
                initial_volume = 100
            "#,
        );
        let cfg = DaemonConfig::load_from(&path).unwrap();
        assert_eq!(cfg.connect.initial_volume, 100);
    }

    #[test]
    fn resolver_prefers_explicit_over_env_and_default() {
        let explicit = PathBuf::from("/tmp/explicit.toml");
        let resolved = resolve_config_path(
            Some(&explicit),
            Some(OsString::from("/tmp/env.toml")),
            Some(PathBuf::from("/tmp/default.toml")),
        );
        assert_eq!(resolved, Some(explicit));
    }

    #[test]
    fn resolver_prefers_env_over_default() {
        let resolved = resolve_config_path(
            None,
            Some(OsString::from("/tmp/env.toml")),
            Some(PathBuf::from("/tmp/default.toml")),
        );
        assert_eq!(resolved, Some(PathBuf::from("/tmp/env.toml")));
    }

    #[test]
    fn resolver_falls_back_to_default_when_env_missing_or_empty() {
        let default = Some(PathBuf::from("/tmp/default.toml"));
        assert_eq!(
            resolve_config_path(None, None, default.clone()),
            default.clone(),
        );
        // Empty env value should be treated as "unset".
        assert_eq!(
            resolve_config_path(None, Some(OsString::from("")), default.clone()),
            default,
        );
        // Whitespace-only too.
        assert_eq!(
            resolve_config_path(None, Some(OsString::from("   \n")), None),
            None,
        );
    }

    #[test]
    fn resolver_returns_none_when_nothing_available() {
        assert_eq!(resolve_config_path(None, None, None), None);
    }
}
