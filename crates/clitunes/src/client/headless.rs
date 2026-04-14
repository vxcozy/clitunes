//! One-shot headless verb dispatch.
//!
//! Connects to the daemon, sends a single verb, waits for the
//! `CommandResult`, prints errors if any, and exits.
//!
//! The [`dispatch_browse`] variant additionally prints any
//! `SearchResults`, `LibraryResults`, or `PlaylistResults` event
//! received before the final `CommandResult` as a single JSON line
//! on stdout — this is how `clitunes search "…"` and
//! `clitunes browse …` deliver their payload.

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

pub async fn dispatch(socket_path: &Path, verb: Verb) -> Result<()> {
    let mut session = ControlSession::connect(socket_path)
        .await
        .context("connect to daemon")?;

    session.send_verb(verb).await.context("send verb")?;

    // Wait for the CommandResult acknowledging our verb.
    for _ in 0..50 {
        match session.recv_event_timeout(Duration::from_millis(100)).await {
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

    anyhow::bail!(
        "daemon did not respond within {}s",
        RESULT_TIMEOUT.as_secs()
    )
}

/// Dispatch a browse verb (Search / BrowseLibrary / BrowsePlaylist).
///
/// Prints any result event as a single JSON line on stdout, then waits
/// for the terminating `CommandResult`. Errors are surfaced via
/// `anyhow::bail!` so the CLI exits non-zero — callers (tests, shell
/// pipelines) can distinguish success from a failed round-trip.
pub async fn dispatch_browse(socket_path: &Path, verb: Verb) -> Result<()> {
    let mut session = ControlSession::connect(socket_path)
        .await
        .context("connect to daemon")?;

    session.send_verb(verb).await.context("send verb")?;

    // Browse verbs round-trip through Spotify; poll for longer but
    // still bounded so a hung daemon can't lock the CLI forever.
    let max_iters = (BROWSE_TIMEOUT.as_millis() / 100) as u32;
    for _ in 0..max_iters {
        match session.recv_event_timeout(Duration::from_millis(100)).await {
            Some(ev @ Event::SearchResults { .. })
            | Some(ev @ Event::LibraryResults { .. })
            | Some(ev @ Event::PlaylistResults { .. }) => {
                println!("{}", ev.to_line());
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

    anyhow::bail!(
        "daemon did not respond within {}s",
        BROWSE_TIMEOUT.as_secs()
    )
}
