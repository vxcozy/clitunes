use std::path::Path;
use std::sync::atomic::AtomicU64;

use anyhow::Result;
use clitunes_engine::proto::client::ControlClient;
use clitunes_engine::proto::events::Event;
use clitunes_engine::proto::verbs::{SourceArg, Verb, VerbEnvelope};
use tokio::time::Duration;

static CMD_SEQ: AtomicU64 = AtomicU64::new(1);

fn next_cmd_id() -> String {
    let n = CMD_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("c-{n}")
}

pub struct ControlSession {
    client: ControlClient,
}

impl ControlSession {
    pub async fn connect(socket_path: &Path) -> Result<Self> {
        // "browse" is in the default subscription list so `Search`,
        // `BrowseLibrary`, and `BrowsePlaylist` result events reach
        // both the TUI (Search/Library tabs) and headless CLI
        // (`clitunes search`, `clitunes browse`). The daemon drops
        // broadcasts for unsubscribed topics, so omitting this meant
        // browse verbs timed out waiting for results that were produced
        // but never delivered.
        let subscriptions = vec![
            "state".into(),
            "now_playing".into(),
            "pcm_meta".into(),
            "errors".into(),
            "browse".into(),
        ];
        let client = ControlClient::connect(socket_path, "clitunes-tui", subscriptions).await?;
        Ok(Self { client })
    }

    pub async fn send_verb(&mut self, verb: Verb) -> Result<()> {
        let envelope = VerbEnvelope {
            cmd_id: next_cmd_id(),
            verb,
        };
        self.client.send(envelope).await
    }

    pub async fn source_radio(&mut self, uuid: &str) -> Result<()> {
        self.send_verb(Verb::Source(SourceArg::Radio {
            uuid: uuid.to_owned(),
        }))
        .await
    }

    pub async fn play(&mut self) -> Result<()> {
        self.send_verb(Verb::Play).await
    }

    pub async fn pause(&mut self) -> Result<()> {
        self.send_verb(Verb::Pause).await
    }

    pub async fn quit_daemon(&mut self) -> Result<()> {
        self.send_verb(Verb::Quit).await
    }

    pub async fn request_status(&mut self) -> Result<()> {
        self.send_verb(Verb::Status).await
    }

    pub async fn set_viz(&mut self, name: &str) -> Result<()> {
        self.send_verb(Verb::Viz {
            name: name.to_owned(),
        })
        .await
    }

    pub async fn recv_event(&mut self) -> Option<Event> {
        self.client.recv().await
    }

    pub async fn recv_event_timeout(&mut self, dur: Duration) -> Option<Event> {
        self.client.recv_timeout(dur).await
    }
}
