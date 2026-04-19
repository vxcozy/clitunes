//! Subsequence fuzzy matcher for the `:viz <name>` command bar.
//!
//! Purpose-built for a tiny catalogue (22 visualiser names, ~8 chars each)
//! — no external dep, no trained ranking, no Unicode-aware scoring. The
//! trade-off is intentional: adding a crate like `nucleo` for 23 strings
//! would outweigh the savings. If the catalogue grows past ~100, swap the
//! implementation behind [`fuzzy_match`] without changing the call site.
//!
//! # Scoring
//!
//! - A candidate matches iff the query is a subsequence of it (case-
//!   insensitive). Non-matches return `None` and are excluded from results.
//! - Base score = number of matched chars.
//! - `+3` if the query matches at candidate position 0 (prefix bonus).
//! - `+1` per contiguous 2-char run within the matched subsequence.
//! - `-1` per skipped char (gap) between matches.
//!
//! # Ties
//!
//! [`fuzzy_match`] sorts by score descending with a stable sort so equal-
//! score candidates preserve the input order. Callers should pass
//! candidates in registration order so tied matches resolve to the
//! user's expected "first one registered wins."

/// Fuzzy-match `query` against `candidates` and return them ranked by score,
/// highest first. Non-matching candidates are excluded. An empty query
/// returns every candidate with score `0` (no ranking, just passthrough).
pub fn fuzzy_match(query: &str, candidates: &[&'static str]) -> Vec<(&'static str, i32)> {
    if query.is_empty() {
        return candidates.iter().map(|c| (*c, 0)).collect();
    }
    let q = query.to_ascii_lowercase();
    let mut out: Vec<(&'static str, i32)> = candidates
        .iter()
        .filter_map(|c| {
            let score = score_subsequence(&q, &c.to_ascii_lowercase())?;
            Some((*c, score))
        })
        .collect();
    // Stable sort preserves registration order on score ties.
    // (Vec::sort_by_key is documented as stable — same guarantee as sort_by.)
    out.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
    out
}

/// Score a case-folded `query` as a subsequence of case-folded `candidate`.
/// Returns `None` when the query is not a subsequence.
///
/// Both inputs must already be lower-cased — the caller in [`fuzzy_match`]
/// does that once per candidate. This keeps the inner loop allocation-free.
fn score_subsequence(query: &str, candidate: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q_bytes = query.as_bytes();
    let c_bytes = candidate.as_bytes();

    let mut matched: i32 = 0;
    let mut runs_bonus: i32 = 0;
    let mut gap_penalty: i32 = 0;
    let mut prefix_bonus: i32 = 0;

    let mut qi = 0usize;
    let mut ci = 0usize;
    let mut last_match_ci: Option<usize> = None;
    let mut first_match_ci: Option<usize> = None;

    while qi < q_bytes.len() && ci < c_bytes.len() {
        if q_bytes[qi] == c_bytes[ci] {
            matched += 1;
            if let Some(prev) = last_match_ci {
                let gap = ci - prev - 1;
                if gap == 0 {
                    runs_bonus += 1;
                } else {
                    gap_penalty += gap as i32;
                }
            }
            if first_match_ci.is_none() {
                first_match_ci = Some(ci);
            }
            last_match_ci = Some(ci);
            qi += 1;
        }
        ci += 1;
    }

    // Query must be fully consumed — otherwise it isn't a subsequence.
    if qi < q_bytes.len() {
        return None;
    }

    if first_match_ci == Some(0) {
        prefix_bonus = 3;
    }

    Some(matched + runs_bonus - gap_penalty + prefix_bonus)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOGUE: &[&str] = &[
        "plasma",
        "ripples",
        "tunnel",
        "metaballs",
        "vortex",
        "fire",
        "matrix",
        "moire",
        "wave",
        "scope",
        "heartbeat",
        "classicpeak",
        "barsdot",
        "barsoutline",
        "binary",
        "scatter",
        "terrain",
        "butterfly",
        "pulse",
        "rain",
        "sakura",
        "retro",
    ];

    #[test]
    fn sak_matches_sakura_with_prefix_bonus() {
        let results = fuzzy_match("sak", CATALOGUE);
        assert!(!results.is_empty(), "expected at least one match");
        let (top, score) = results[0];
        assert_eq!(top, "sakura");
        // 3 matched chars + 3 prefix + 2 runs (s→a→k contiguous) = 8
        assert_eq!(score, 8);
    }

    #[test]
    fn hbt_matches_heartbeat_as_subsequence_with_prefix_h() {
        let results = fuzzy_match("hbt", CATALOGUE);
        let top = results.first().map(|(n, _)| *n).unwrap_or("<none>");
        assert_eq!(top, "heartbeat");
        // heartbeat = h(0) e(1) a(2) r(3) t(4) b(5) e(6) a(7) t(8).
        // Greedy left-to-right match: h@0, b@5, t@8.
        // matched=3, runs=0, gaps=(5-0-1)+(8-5-1)=4+2=6, prefix=3
        // score = 3 + 0 - 6 + 3 = 0
        assert_eq!(results[0].1, 0);
    }

    #[test]
    fn no_match_returns_empty() {
        let results = fuzzy_match("xyz", CATALOGUE);
        assert!(results.is_empty(), "expected no matches, got {results:?}");
    }

    #[test]
    fn single_char_b_matches_many_in_registration_order() {
        let results = fuzzy_match("b", CATALOGUE);
        // Every candidate containing 'b' matches; registration order among
        // equal-score ties must be preserved.
        let names: Vec<&str> = results.iter().map(|(n, _)| *n).collect();
        let expected: Vec<&str> = CATALOGUE
            .iter()
            .filter(|n| n.contains('b'))
            .copied()
            .collect();
        // Names with 'b' at position 0 get the prefix bonus and should sort
        // to the top; the rest sort stably in their original order.
        let (prefix_hits, non_prefix): (Vec<_>, Vec<_>) =
            expected.iter().partition(|n| n.starts_with('b'));
        let mut want: Vec<&str> = prefix_hits.into_iter().copied().collect();
        want.extend(non_prefix.into_iter().copied());
        assert_eq!(names, want);
    }

    #[test]
    fn case_insensitive() {
        let lower = fuzzy_match("sak", CATALOGUE);
        let upper = fuzzy_match("SAK", CATALOGUE);
        let mixed = fuzzy_match("Sak", CATALOGUE);
        assert_eq!(lower, upper);
        assert_eq!(lower, mixed);
    }

    #[test]
    fn empty_query_returns_all_candidates_score_zero() {
        let results = fuzzy_match("", CATALOGUE);
        assert_eq!(results.len(), CATALOGUE.len());
        for (i, (name, score)) in results.iter().enumerate() {
            assert_eq!(*name, CATALOGUE[i], "order should match input");
            assert_eq!(*score, 0);
        }
    }

    #[test]
    fn exact_match_outscores_partial_prefix() {
        // "sak" vs "sakura" — partial prefix, score from the sak_matches test.
        let partial = fuzzy_match("sak", CATALOGUE)
            .into_iter()
            .find(|(n, _)| *n == "sakura")
            .map(|(_, s)| s)
            .unwrap();
        // Exact name: every char matches contiguously from position 0.
        let exact = fuzzy_match("sakura", CATALOGUE)
            .into_iter()
            .find(|(n, _)| *n == "sakura")
            .map(|(_, s)| s)
            .unwrap();
        assert!(
            exact > partial,
            "exact {exact} should outscore partial {partial}"
        );
    }

    #[test]
    #[ignore = "sanity check for realistic queries — run with --ignored --nocapture"]
    fn print_top5_for_realistic_queries() {
        for q in ["sak", "b", "hrt", "fire", "ret"] {
            let results = fuzzy_match(q, CATALOGUE);
            eprintln!("\nquery={q:?}");
            for (i, (name, score)) in results.iter().take(5).enumerate() {
                eprintln!("  {}: {} ({})", i + 1, name, score);
            }
        }
    }

    #[test]
    fn non_subsequence_rejected_even_with_shared_chars() {
        // "sx" shares 's' with sakura/scatter/scope but 'x' can't be found
        // anywhere after 's' — not a subsequence.
        let results = fuzzy_match("sx", CATALOGUE);
        assert!(results.is_empty());
    }
}
