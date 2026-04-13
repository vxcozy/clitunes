//! Baked curated-seed list + override loader.
//!
//! The seed is 12 slots covering a deliberately broad range of genres
//! and regions. The actual station choices are filled in during Slice
//! 2 polish — see `docs/curation/2026-04-11-curated-stations.md` for
//! the live list, rationale per slot, and the engineer-taste audit.
//!
//! This module ships the **shape** (slot count, slot semantics, load
//! precedence, fallback behavior) plus enough placeholder entries to
//! exercise the picker UI, persistence flow, and tests end-to-end.
//! The polish pass replaces placeholder URLs with real
//! `radiobrowser:<uuid>` sentinels without touching any of this code.
//!
//! # Why placeholders instead of real stations right now
//!
//! Choosing 12 representative stations is a curation exercise that
//! must explicitly **not** reflect engineer taste (D11 + the
//! `feedback_no_taste_imposition.md` memory). Doing it half-heartedly
//! at Unit-8 implementation time would violate that rule. Shipping
//! the infrastructure first and curating as a separate tracked pass
//! keeps the two concerns separate.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clitunes_core::CuratedStation;
use serde::Deserialize;
use tracing::warn;

/// Number of curated slots the picker expects. Fixed at 12 so the
/// picker layout math (rows, spacing) has a known worst case, and so
/// removing a slot is a deliberate decision rather than accidental
/// drift. If a Slice 2 polish pass wants to move to 10 or 15, change
/// this here and update the layout tests — don't silently short the
/// list.
pub const CURATED_SLOT_COUNT: usize = 12;

/// A full curated list, always exactly [`CURATED_SLOT_COUNT`] entries
/// in `slot` order (0-indexed internally even though the display
/// numbers them 1..12).
#[derive(Clone, Debug)]
pub struct CuratedList {
    pub stations: Vec<CuratedStation>,
    /// Where the list came from. Used by the picker header so users
    /// can tell at a glance whether their override file loaded.
    pub origin: CuratedOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CuratedOrigin {
    /// Baked-in seed from this crate.
    Baked,
    /// Override file at `~/.config/clitunes/curated_stations.toml`.
    Override,
}

/// What happened during [`load_curated`]. Reported back to the caller
/// so it can log meaningful messages without re-reading the FS.
#[derive(Clone, Debug)]
pub enum CuratedLoadOutcome {
    /// Baked list used; no override file present.
    BakedNoOverride,
    /// Override file loaded successfully.
    OverrideLoaded(PathBuf),
    /// Override file existed but was rejected; fell back to baked.
    /// The string is a human-readable reason for the log.
    OverrideRejected { path: PathBuf, reason: String },
}

/// Resolve the effective curated list. Loading order:
///
/// 1. If `override_path` (or the default at
///    `$XDG_CONFIG_HOME/clitunes/curated_stations.toml`) exists and
///    parses to a non-empty list, use it.
/// 2. Otherwise return the baked list.
///
/// Never returns an error on a missing override — absence is the
/// normal case. Only parse errors or empty lists trigger fallback,
/// and those are reported via [`CuratedLoadOutcome::OverrideRejected`]
/// so the caller can `warn!()` once.
pub fn load_curated(override_path: Option<&Path>) -> (CuratedList, CuratedLoadOutcome) {
    let path = match override_path
        .map(PathBuf::from)
        .or_else(default_override_path)
    {
        Some(p) => p,
        None => return (baked_list(), CuratedLoadOutcome::BakedNoOverride),
    };

    match try_load_override(&path) {
        Ok(Some(list)) => {
            let outcome = CuratedLoadOutcome::OverrideLoaded(path);
            (list, outcome)
        }
        Ok(None) => (baked_list(), CuratedLoadOutcome::BakedNoOverride),
        Err(reason) => {
            warn!(path = %path.display(), error = %reason, "curated override rejected");
            let outcome = CuratedLoadOutcome::OverrideRejected { path, reason };
            (baked_list(), outcome)
        }
    }
}

/// Default override file location: `~/.config/clitunes/curated_stations.toml`
/// on Linux, `~/Library/Application Support/clitunes/curated_stations.toml`
/// on macOS.
pub fn default_override_path() -> Option<PathBuf> {
    dirs::config_dir().map(|base| base.join("clitunes").join("curated_stations.toml"))
}

/// Read and validate an override file. Returns `Ok(None)` when the
/// file is absent (normal), `Ok(Some(list))` on success, and
/// `Err(reason)` for malformed or empty lists so the caller can
/// surface a human-readable warning.
fn try_load_override(path: &Path) -> Result<Option<CuratedList>, String> {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("reading override file: {e}")),
    };
    let parsed: OverrideFile =
        toml::from_str(&raw).map_err(|e| format!("parsing override toml: {e}"))?;
    if parsed.stations.is_empty() {
        return Err("override file has zero stations".into());
    }

    let stations: Vec<CuratedStation> = parsed
        .stations
        .into_iter()
        .enumerate()
        .map(|(i, s)| CuratedStation {
            slot: i as u8,
            name: s.name,
            genre: s.genre,
            country: s.country,
            url: s.url,
            rationale: s.rationale.unwrap_or_default(),
        })
        .collect();

    Ok(Some(CuratedList {
        stations,
        origin: CuratedOrigin::Override,
    }))
}

#[derive(Debug, Deserialize)]
struct OverrideFile {
    stations: Vec<OverrideEntry>,
}

#[derive(Debug, Deserialize)]
struct OverrideEntry {
    name: String,
    genre: String,
    country: String,
    url: String,
    rationale: Option<String>,
}

/// Build the baked-in 12-slot placeholder list. Replaced slot-by-slot
/// during Slice 2 polish with real `radiobrowser:<uuid>` sentinels
/// whose rationale is documented in
/// `docs/curation/2026-04-11-curated-stations.md`.
///
/// The slot ordering corresponds to the genre coverage plan:
///
/// | slot | bucket                              |
/// |------|-------------------------------------|
/// |   0  | ambient / lo-fi                     |
/// |   1  | classical                           |
/// |   2  | jazz                                |
/// |   3  | electronic / dance                  |
/// |   4  | indie / alt rock                    |
/// |   5  | world music                         |
/// |   6  | news / talk (public broadcaster)    |
/// |   7  | classic rock / classic hits         |
/// |   8  | soul / funk / r&b                   |
/// |   9  | hip-hop / instrumental hip-hop      |
/// |  10  | experimental / drone                |
/// |  11  | discovery wildcard (rotates/release)|
pub fn baked_list() -> CuratedList {
    const SLOT_LABELS: [&str; CURATED_SLOT_COUNT] = [
        "ambient",
        "classical",
        "jazz",
        "electronic",
        "indie",
        "world",
        "news/talk",
        "classic rock",
        "soul/funk/r&b",
        "hip-hop",
        "experimental",
        "discovery",
    ];

    let stations = vec![
        CuratedStation {
            slot: 0,
            name: "SomaFM Groove Salad".into(),
            genre: SLOT_LABELS[0].into(),
            country: "US".into(),
            url: "radiobrowser:960cf833-0601-11e8-ae97-52543be04c81".into(),
            rationale: "Top ambient/chillout stream; reliable uptime".into(),
        },
        CuratedStation {
            slot: 1,
            name: "Classic FM".into(),
            genre: SLOT_LABELS[1].into(),
            country: "UK".into(),
            url: "radiobrowser:96063f25-0601-11e8-ae97-52543be04c81".into(),
            rationale: "High-traffic classical station with ICY metadata".into(),
        },
        CuratedStation {
            slot: 2,
            name: "101 Smooth Jazz".into(),
            genre: SLOT_LABELS[2].into(),
            country: "US".into(),
            url: "radiobrowser:d28420a4-eccf-47a2-ace1-088c7e7cb7e0".into(),
            rationale: "Popular jazz stream with track metadata".into(),
        },
        CuratedStation {
            slot: 3,
            name: "Dance Wave!".into(),
            genre: SLOT_LABELS[3].into(),
            country: "HU".into(),
            url: "radiobrowser:962cc6df-0601-11e8-ae97-52543be04c81".into(),
            rationale: "High-energy electronic/dance; good for visualiser demos".into(),
        },
        CuratedStation {
            slot: 4,
            name: "SomaFM Indie Pop Rocks!".into(),
            genre: SLOT_LABELS[4].into(),
            country: "US".into(),
            url: "radiobrowser:96394224-0601-11e8-ae97-52543be04c81".into(),
            rationale: "SomaFM indie stream; reliable ICY metadata".into(),
        },
        CuratedStation {
            slot: 5,
            name: "Radio Nova".into(),
            genre: SLOT_LABELS[5].into(),
            country: "FR".into(),
            url: "radiobrowser:963fb390-0601-11e8-ae97-52543be04c81".into(),
            rationale: "Eclectic world/pop from Paris; broad genre coverage".into(),
        },
        CuratedStation {
            slot: 6,
            name: "NPR Program Stream".into(),
            genre: SLOT_LABELS[6].into(),
            country: "US".into(),
            url: "radiobrowser:7ba4c184-fc2b-11e9-bbf2-52543be04c81".into(),
            rationale: "24-hour NPR news/talk; standard public radio pick".into(),
        },
        CuratedStation {
            slot: 7,
            name: "RdMix Classic Rock".into(),
            genre: SLOT_LABELS[7].into(),
            country: "CA".into(),
            url: "radiobrowser:7afae7e3-8d06-42f5-b59e-a52d6e09e60e".into(),
            rationale: "High-click classic rock stream spanning 60s-90s".into(),
        },
        CuratedStation {
            slot: 8,
            name: "Jazz Radio".into(),
            genre: SLOT_LABELS[8].into(),
            country: "FR".into(),
            url: "radiobrowser:96136fe5-0601-11e8-ae97-52543be04c81".into(),
            rationale: "French jazz/soul station with reliable stream".into(),
        },
        CuratedStation {
            slot: 9,
            name: "100 Hip Hop and RNB".into(),
            genre: SLOT_LABELS[9].into(),
            country: "US".into(),
            url: "radiobrowser:dba1b7bc-6b92-409c-a543-8b42eec25636".into(),
            rationale: "Dedicated hip-hop/R&B stream".into(),
        },
        CuratedStation {
            slot: 10,
            name: "BBC Radio 6 Music".into(),
            genre: SLOT_LABELS[10].into(),
            country: "UK".into(),
            url: "radiobrowser:1c6dcd6f-88c6-4fd4-8191-078435168e85".into(),
            rationale: "BBC eclectic/experimental; broadest genre range".into(),
        },
        CuratedStation {
            slot: 11,
            name: "Chillofi Radio".into(),
            genre: SLOT_LABELS[11].into(),
            country: "FR".into(),
            url: "radiobrowser:9afb0f28-5ff1-4547-8eb7-7edc0e48e1d0".into(),
            rationale: "Lo-fi/chillhop discovery station".into(),
        },
    ];

    CuratedList {
        stations,
        origin: CuratedOrigin::Baked,
    }
}

/// Render-friendly helper: given a curated list, return the
/// user-facing slot label string ("1. Classical — Radio Paradise" etc.)
/// Used by the picker paint code and tests.
pub fn display_line(station: &CuratedStation) -> String {
    let slot = station.slot + 1;
    format!(
        "{slot:>2}. {genre:<14} {name}",
        slot = slot,
        genre = truncate(&station.genre, 14),
        name = station.name,
    )
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Opportunistic helper used by the higher-level picker code to
/// persist which slot the user landed on. Safe to call with a slot
/// that's out of range — returns `None` so the caller can surface a
/// "slot no longer exists" fallback.
pub fn station_at_slot(list: &CuratedList, slot: u8) -> Option<&CuratedStation> {
    list.stations.iter().find(|s| s.slot == slot)
}

/// Sanity-check that an in-memory list has the expected shape. Runs
/// in tests only; real code should treat a mis-sized override as a
/// reason to fall back, not to panic.
#[cfg(test)]
fn expect_well_formed(list: &CuratedList) {
    assert_eq!(list.stations.len(), CURATED_SLOT_COUNT);
    for (i, s) in list.stations.iter().enumerate() {
        assert_eq!(s.slot as usize, i, "slot index must match vector position");
        assert!(!s.name.is_empty());
        assert!(!s.url.is_empty());
    }
}

/// Load-state convenience helper: read the override file explicitly
/// (used from tests). Public so integration tests in
/// `tests/tui_picker_tests.rs` can exercise it directly without going
/// through the full precedence logic.
pub fn load_override_only(path: &Path) -> Result<CuratedList> {
    try_load_override(path)
        .map_err(|reason| anyhow::anyhow!(reason))?
        .context("override file is absent")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn baked_list_is_twelve_slots() {
        let baked = baked_list();
        assert_eq!(baked.origin, CuratedOrigin::Baked);
        expect_well_formed(&baked);
    }

    #[test]
    fn baked_slot_labels_are_unique() {
        let baked = baked_list();
        let mut seen = std::collections::HashSet::new();
        for s in &baked.stations {
            assert!(
                seen.insert(s.genre.clone()),
                "duplicate genre {:?}",
                s.genre
            );
        }
    }

    #[test]
    fn override_with_missing_file_falls_back_to_baked() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nope.toml");
        let (list, outcome) = load_curated(Some(&path));
        assert_eq!(list.origin, CuratedOrigin::Baked);
        assert!(matches!(outcome, CuratedLoadOutcome::BakedNoOverride));
    }

    #[test]
    fn override_with_valid_entries_loads() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("curated.toml");
        fs::write(
            &path,
            r#"
[[stations]]
name = "Radio Alpha"
genre = "jazz"
country = "FR"
url = "http://alpha.example/stream"

[[stations]]
name = "Radio Beta"
genre = "ambient"
country = "JP"
url = "radiobrowser:abc-123"
rationale = "covers the ambient slot for testing"
"#,
        )
        .unwrap();

        let (list, outcome) = load_curated(Some(&path));
        assert_eq!(list.origin, CuratedOrigin::Override);
        assert!(matches!(outcome, CuratedLoadOutcome::OverrideLoaded(_)));
        assert_eq!(list.stations.len(), 2);
        assert_eq!(list.stations[0].slot, 0);
        assert_eq!(list.stations[1].slot, 1);
        assert_eq!(
            list.stations[1].rationale,
            "covers the ambient slot for testing"
        );
    }

    #[test]
    fn override_with_empty_list_is_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("curated.toml");
        fs::write(&path, "stations = []\n").unwrap();
        let (list, outcome) = load_curated(Some(&path));
        assert_eq!(list.origin, CuratedOrigin::Baked);
        match outcome {
            CuratedLoadOutcome::OverrideRejected { reason, .. } => {
                assert!(reason.contains("zero stations"), "reason was {reason}");
            }
            other => panic!("expected OverrideRejected, got {other:?}"),
        }
    }

    #[test]
    fn override_with_malformed_toml_is_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("curated.toml");
        fs::write(&path, "this is :: not toml").unwrap();
        let (list, outcome) = load_curated(Some(&path));
        assert_eq!(list.origin, CuratedOrigin::Baked);
        assert!(matches!(
            outcome,
            CuratedLoadOutcome::OverrideRejected { .. }
        ));
    }

    #[test]
    fn station_at_slot_bounds() {
        let baked = baked_list();
        assert!(station_at_slot(&baked, 0).is_some());
        assert!(station_at_slot(&baked, (CURATED_SLOT_COUNT - 1) as u8).is_some());
        assert!(station_at_slot(&baked, 99).is_none());
    }

    #[test]
    fn display_line_truncates_long_genres() {
        let s = CuratedStation {
            slot: 0,
            name: "Testing".into(),
            genre: "this-genre-label-is-too-long-for-column".into(),
            country: "US".into(),
            url: "http://example.com".into(),
            rationale: String::new(),
        };
        let line = display_line(&s);
        assert!(line.contains("…"));
        assert!(line.contains("Testing"));
    }

    #[test]
    fn load_override_only_errors_on_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nope.toml");
        assert!(load_override_only(&path).is_err());
    }
}
