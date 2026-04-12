use std::path::Path;

use futures_util::{SinkExt, StreamExt};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};
use tokio_util::codec::Framed;

use super::banner::{ClientBanner, ServerBanner, PROTOCOL_VERSION};
use super::codec::control_codec;
use super::events::Event;
use super::verbs::VerbEnvelope;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

pub struct ControlClient {
    sink:
        futures_util::stream::SplitSink<Framed<UnixStream, tokio_util::codec::LinesCodec>, String>,
    event_rx: mpsc::Receiver<Event>,
    server_banner: ServerBanner,
}

impl ControlClient {
    pub async fn connect(
        path: &Path,
        client_name: &str,
        initial_subscriptions: Vec<String>,
    ) -> anyhow::Result<Self> {
        let stream = UnixStream::connect(path).await?;
        let mut framed = Framed::new(stream, control_codec());

        let server_line = match timeout(HANDSHAKE_TIMEOUT, framed.next()).await {
            Ok(Some(Ok(line))) => line,
            Ok(Some(Err(e))) => anyhow::bail!("handshake read error: {e}"),
            Ok(None) => anyhow::bail!("server closed before banner"),
            Err(_) => anyhow::bail!("handshake timeout"),
        };

        let server_banner = ServerBanner::from_line(&server_line)?;
        if server_banner.version != PROTOCOL_VERSION {
            anyhow::bail!(
                "protocol version mismatch: server={}, client={}",
                server_banner.version,
                PROTOCOL_VERSION
            );
        }

        let client_banner = ClientBanner {
            client: client_name.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            subscribe: initial_subscriptions,
        };
        framed.send(client_banner.to_line()).await?;

        let (sink, stream) = framed.split();

        let (event_tx, event_rx) = mpsc::channel(64);
        tokio::spawn(async move {
            let mut stream = stream;
            while let Some(Ok(line)) = stream.next().await {
                match Event::from_line(&line) {
                    Ok(event) => {
                        if event_tx.send(event).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, line = %line, "unparseable server event");
                    }
                }
            }
        });

        Ok(Self {
            sink,
            event_rx,
            server_banner,
        })
    }

    pub fn server_banner(&self) -> &ServerBanner {
        &self.server_banner
    }

    pub async fn send(&mut self, envelope: VerbEnvelope) -> anyhow::Result<()> {
        self.sink.send(envelope.to_line()).await?;
        Ok(())
    }

    pub async fn recv(&mut self) -> Option<Event> {
        self.event_rx.recv().await
    }

    pub async fn recv_timeout(&mut self, dur: Duration) -> Option<Event> {
        timeout(dur, self.event_rx.recv()).await.ok().flatten()
    }
}
