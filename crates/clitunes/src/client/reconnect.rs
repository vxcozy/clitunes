use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clitunes_engine::proto::events::Event;
use clitunes_engine::proto::verbs::Verb;
use tokio::time::sleep;

use super::control_session::ControlSession;
use crate::auto_spawn::{self, SpawnConfig};

const RECONNECT_DELAY: Duration = Duration::from_millis(200);
const MAX_RECONNECT_ATTEMPTS: usize = 5;

pub struct ReconnectingSession {
    session: ControlSession,
    socket_path: PathBuf,
    /// Fires once per successful reconnect so the render loop can
    /// rebuild any client-side state that assumed continuity with the
    /// prior daemon process (e.g. pending Spotify auth). `None` for
    /// headless callers that don't care.
    reconnect_notify: Option<std::sync::mpsc::Sender<()>>,
}

impl ReconnectingSession {
    pub async fn connect(socket_path: PathBuf) -> Result<Self> {
        let session = ControlSession::connect(&socket_path).await?;
        Ok(Self {
            session,
            socket_path,
            reconnect_notify: None,
        })
    }

    /// Install a reconnect notifier. Called by the TUI bridge so the
    /// render loop can react to socket drops that survived auto-spawn
    /// recovery. Headless paths leave this unset.
    pub fn set_reconnect_notifier(&mut self, tx: std::sync::mpsc::Sender<()>) {
        self.reconnect_notify = Some(tx);
    }

    pub async fn send_verb(&mut self, verb: Verb) -> Result<()> {
        self.session.send_verb(verb).await
    }

    pub async fn request_status(&mut self) -> Result<()> {
        self.session.request_status().await
    }

    pub async fn recv_event(&mut self) -> Option<Event> {
        match self.session.recv_event().await {
            Some(ev) => Some(ev),
            None => {
                tracing::warn!(target: "clitunes", "control socket EOF; attempting reconnect");
                if self.reconnect_loop().await.is_err() {
                    return None;
                }
                self.session.recv_event().await
            }
        }
    }

    pub async fn recv_event_timeout(&mut self, dur: Duration) -> Option<Event> {
        self.session.recv_event_timeout(dur).await
    }

    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }

    async fn reconnect_loop(&mut self) -> Result<()> {
        for attempt in 1..=MAX_RECONNECT_ATTEMPTS {
            sleep(RECONNECT_DELAY).await;

            let socket_path = self.socket_path.clone();
            let spawn_result = tokio::task::spawn_blocking(move || {
                auto_spawn::connect_or_spawn_at(&socket_path, &SpawnConfig::default())
            })
            .await;

            match spawn_result {
                Ok(Ok(connected)) => {
                    drop(connected.stream);
                    match ControlSession::connect(&connected.socket_path).await {
                        Ok(session) => {
                            self.session = session;
                            tracing::info!(target: "clitunes", attempt, "reconnected to daemon");
                            if let Some(tx) = self.reconnect_notify.as_ref() {
                                // Best-effort: the render loop may have
                                // dropped the receiver during shutdown.
                                let _ = tx.send(());
                            }
                            return Ok(());
                        }
                        Err(e) => {
                            tracing::warn!(
                                target: "clitunes",
                                attempt,
                                error = %e,
                                "protocol handshake failed after reconnect"
                            );
                        }
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        target: "clitunes",
                        attempt,
                        error = %e,
                        "auto-spawn failed during reconnect"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        target: "clitunes",
                        attempt,
                        error = %e,
                        "reconnect task panicked"
                    );
                }
            }
        }

        tracing::error!(
            target: "clitunes",
            attempts = MAX_RECONNECT_ATTEMPTS,
            "exhausted reconnect attempts"
        );
        anyhow::bail!("reconnect failed after {MAX_RECONNECT_ATTEMPTS} attempts")
    }
}
