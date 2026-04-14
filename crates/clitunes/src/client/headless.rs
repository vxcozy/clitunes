//! One-shot headless verb dispatch.
//!
//! Connects to the daemon, sends a single verb, waits for the
//! `CommandResult`, prints errors if any, and exits.
//!
//! Two public entry points share a single implementation
//! ([`dispatch_inner`]):
//!
//! - [`dispatch`] is for simple playback verbs with a short timeout.
//! - [`dispatch_browse`] is for `Search` / `BrowseLibrary` /
//!   `BrowsePlaylist` verbs; it uses a longer timeout and additionally
//!   prints any `SearchResults`, `LibraryResults`, or `PlaylistResults`
//!   event as a single JSON line on stdout before the terminating
//!   `CommandResult`. This is how `clitunes search "…"` and
//!   `clitunes browse …` deliver their payload.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};

use super::control_session::ControlSession;
use clitunes_engine::proto::events::Event;
use clitunes_engine::proto::verbs::Verb;

const RESULT_TIMEOUT: Duration = Duration::from_secs(5);
/// Browse verbs (search/library/playlist) can take longer than simple
/// playback verbs because they round-trip to the Spotify CDN. 30s is
/// generous — any slower and something is wrong anyway.
const BROWSE_TIMEOUT: Duration = Duration::from_secs(30);

/// Shared implementation for both [`dispatch`] and [`dispatch_browse`].
///
/// Connects to the daemon, sends `verb`, then polls for events in 100ms
/// ticks up to `timeout`. When `print_results` is true, result-payload
/// events (`SearchResults` / `LibraryResults` / `PlaylistResults`) are
/// written to stdout as JSON lines before the terminating
/// `CommandResult`.
async fn dispatch_inner(
    socket_path: &Path,
    verb: Verb,
    timeout: Duration,
    print_results: bool,
) -> Result<()> {
    let mut session = ControlSession::connect(socket_path)
        .await
        .context("connect to daemon")?;

    session.send_verb(verb).await.context("send verb")?;

    let max_iters = (timeout.as_millis() / 100) as u32;
    for _ in 0..max_iters {
        match session.recv_event_timeout(Duration::from_millis(100)).await {
            Some(ev @ Event::SearchResults { .. })
            | Some(ev @ Event::LibraryResults { .. })
            | Some(ev @ Event::PlaylistResults { .. }) => {
                if print_results {
                    println!("{}", ev.to_line());
                }
            }
            Some(Event::CommandResult { ok: true, .. }) => return Ok(()),
            Some(Event::CommandResult {
                ok: false, error, ..
            }) => {
                let msg = error.unwrap_or_else(|| "unknown error".into());
                anyhow::bail!("{msg}");
            }
            Some(_) => continue,
            None => continue,
        }
    }

    anyhow::bail!("daemon did not respond within {}s", timeout.as_secs())
}

/// Dispatch a simple playback verb and wait for its `CommandResult`.
pub async fn dispatch(socket_path: &Path, verb: Verb) -> Result<()> {
    dispatch_inner(socket_path, verb, RESULT_TIMEOUT, false).await
}

/// Dispatch a browse verb (Search / BrowseLibrary / BrowsePlaylist).
///
/// Prints any result event as a single JSON line on stdout, then waits
/// for the terminating `CommandResult`. Errors are surfaced via
/// `anyhow::bail!` so the CLI exits non-zero — callers (tests, shell
/// pipelines) can distinguish success from a failed round-trip.
pub async fn dispatch_browse(socket_path: &Path, verb: Verb) -> Result<()> {
    dispatch_inner(socket_path, verb, BROWSE_TIMEOUT, true).await
}
