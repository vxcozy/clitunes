//! Integration tests for the station database wire-format conversion +
//! sanitisation. These tests do **not** hit the network. They construct a
//! `StationDb` against an empty mirror list (so any `get_*` call would
//! return an error) and exercise the parsing path via the public surface
//! by spinning up a local in-process HTTP responder when needed.
//!
//! For this round we cover the cheap pure-function checks that don't need
//! a server. The full HTTP path is exercised by the manual verification
//! step in the bead spec ("look up BBC Radio 6 Music's UUID against the
//! live database").

#![cfg(feature = "radio")]

use clitunes_core::sanitize;
use clitunes_engine::sources::radio::station_db::{StationDb, MAX_MIRRORS_TRIED, USER_AGENT};
use clitunes_engine::sources::radio::Mirror;

#[test]
fn user_agent_includes_version_and_repo() {
    assert!(USER_AGENT.starts_with("clitunes/"));
    assert!(USER_AGENT.contains("github.com"));
}

#[test]
fn max_mirrors_tried_is_three() {
    assert_eq!(MAX_MIRRORS_TRIED, 3);
}

#[test]
fn station_db_constructs_with_empty_mirrors() {
    // We allow construction with an empty list — the failure surfaces at
    // call time as a clear "no mirrors" error rather than a panic.
    let db = StationDb::new(vec![]);
    assert!(db.is_ok());
}

#[test]
fn station_db_constructs_with_baked_in_mirrors() {
    let mirrors = clitunes_engine::sources::radio::discovery::baked_in_mirrors();
    let db = StationDb::new(mirrors);
    assert!(db.is_ok());
}

#[tokio::test]
async fn empty_mirror_db_surfaces_clear_error() {
    let db = StationDb::new(vec![]).unwrap();
    let result = db.get_station_by_uuid("does-not-matter").await;
    assert!(result.is_err(), "empty mirror set must error, not panic");
    let msg = format!("{}", result.unwrap_err());
    // The error string should be operator-actionable.
    assert!(
        msg.to_lowercase().contains("network")
            || msg.to_lowercase().contains("radio-browser")
            || msg.to_lowercase().contains("reach"),
        "error message should mention network or radio-browser: {}",
        msg
    );
}

#[test]
fn sanitiser_strips_terminal_escapes_from_metadata() {
    // This test guards the SEC-004 chokepoint: a hostile station name
    // containing CSI clear-screen must come out clean.
    let hostile = "\x1b[2J\x1b[1;1HHostile Radio";
    let cleaned = sanitize(hostile);
    assert!(!cleaned.contains('\x1b'));
    assert!(cleaned.ends_with("Hostile Radio"));
}

#[test]
fn sanitiser_preserves_unicode_station_names() {
    let name = "Радио Энергия 104.7 ✨";
    assert_eq!(sanitize(name), name);
}

#[test]
fn mirror_ordering_test_helper_produces_expected_shape() {
    // Sanity check on the public Mirror type so downstream code can rely
    // on the field shape.
    let m = Mirror {
        host: "de1.api.radio-browser.info".into(),
        priority: 0,
        weight: 0,
    };
    assert_eq!(m.https_base(), "https://de1.api.radio-browser.info");
}
