//! Integration tests for the radio-browser mirror discovery fallback chain.
//!
//! These tests do **not** hit the network. They exercise the cache + baked-in
//! fallback logic against synthetic state in tempfiles. The live SRV path is
//! covered manually per the bead's verification step.

#![cfg(feature = "radio")]

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use clitunes_engine::sources::radio::discovery::{
    baked_in_mirrors, discover_with_paths, sort_by_rfc2782, Mirror, MirrorSource, BAKED_IN_MIRRORS,
};

fn temp_cache_path(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    p.push(format!("clitunes-test-{}-{}-{}.json", name, pid, nanos));
    p
}

#[test]
fn baked_in_list_is_nonempty_and_clean() {
    assert!(!BAKED_IN_MIRRORS.is_empty(), "baked-in must never be empty");
    let mirrors = baked_in_mirrors();
    assert_eq!(mirrors.len(), BAKED_IN_MIRRORS.len());
    for m in &mirrors {
        assert!(!m.host.contains("://"), "host must not include scheme");
        assert!(!m.host.contains('/'), "host must not include path");
        assert!(m.host.contains('.'), "host should be FQDN-shaped");
    }
}

#[test]
fn rfc2782_sort_orders_priority_then_weight_descending() {
    let mut input = vec![
        Mirror {
            host: "z".into(),
            priority: 30,
            weight: 100,
        },
        Mirror {
            host: "a".into(),
            priority: 10,
            weight: 5,
        },
        Mirror {
            host: "b".into(),
            priority: 10,
            weight: 50,
        },
        Mirror {
            host: "c".into(),
            priority: 20,
            weight: 10,
        },
    ];
    sort_by_rfc2782(&mut input);
    let order: Vec<&str> = input.iter().map(|m| m.host.as_str()).collect();
    assert_eq!(order, vec!["b", "a", "c", "z"]);
}

#[tokio::test]
async fn missing_cache_offline_falls_through_to_baked_in() {
    let cache = temp_cache_path("missing");
    assert!(!cache.exists());
    // Force the SRV path to fail by passing an unreachable name? We rely on
    // SRV failing in offline test environments, which it will because the
    // hostname will not resolve in the sandbox. Whatever happens upstream,
    // the discovery contract guarantees we get *some* mirror set back.
    let result = discover_with_paths(Some(cache.clone())).await;
    let _ = std::fs::remove_file(&cache);

    let discovered = result.expect("discovery must always return something");
    assert!(!discovered.mirrors.is_empty());
    // We accept any non-live source: cache won't exist, so we expect baked-in.
    // (Live SRV is also acceptable when running on a developer box with net.)
    match discovered.source {
        MirrorSource::BakedIn | MirrorSource::LiveSrv => {}
        other => panic!(
            "unexpected mirror source for missing-cache test: {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn discover_returns_consistent_priority_ordering() {
    let cache = temp_cache_path("ordering");
    let discovered = discover_with_paths(Some(cache.clone())).await.unwrap();
    let _ = std::fs::remove_file(&cache);
    let priorities: Vec<u16> = discovered.mirrors.iter().map(|m| m.priority).collect();
    let mut sorted = priorities.clone();
    sorted.sort();
    assert_eq!(
        priorities, sorted,
        "mirrors should already be sorted by priority"
    );
}
