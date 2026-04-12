//! One-shot headless verb dispatch.
//!
//! Connects to the daemon, sends a single verb, waits for the
//! `CommandResult`, prints errors if any, and exits.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};

use super::control_session::ControlSession;
use clitunes_engine::proto::events::Event;
use clitunes_engine::proto::verbs::Verb;

const RESULT_TIMEOUT: Duration = Duration::from_secs(5);

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
