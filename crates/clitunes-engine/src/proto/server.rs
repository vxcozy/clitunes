use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};
use tokio_util::codec::Framed;

use super::banner::{ClientBanner, ServerBanner};
use super::codec::ControlCodec;
use super::events::Event;
use super::verbs::VerbEnvelope;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const CLIENT_EVENT_CAPACITY: usize = 64;

pub type VerbSender = mpsc::Sender<(VerbEnvelope, mpsc::Sender<Event>)>;
pub type VerbReceiver = mpsc::Receiver<(VerbEnvelope, mpsc::Sender<Event>)>;

pub struct ControlServer {
    listener: UnixListener,
    capabilities: Vec<String>,
    verb_tx: VerbSender,
    event_broadcast_tx: mpsc::Sender<Event>,
    event_broadcast_rx: mpsc::Receiver<Event>,
    on_connect: Option<Box<dyn Fn() + Send + Sync>>,
    on_disconnect: Option<Box<dyn Fn() + Send + Sync>>,
}

impl ControlServer {
    pub fn bind(
        path: &Path,
        capabilities: Vec<String>,
    ) -> anyhow::Result<(Self, VerbReceiver)> {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        let listener = std::os::unix::net::UnixListener::bind(path)?;
        listener.set_nonblocking(true)?;
        let listener = UnixListener::from_std(listener)?;

        let (verb_tx, verb_rx) = mpsc::channel(64);
        let (event_broadcast_tx, event_broadcast_rx) = mpsc::channel(256);

        Ok((
            Self {
                listener,
                capabilities,
                verb_tx,
                event_broadcast_tx,
                event_broadcast_rx,
                on_connect: None,
                on_disconnect: None,
            },
            verb_rx,
        ))
    }

    pub fn on_connect<F: Fn() + Send + Sync + 'static>(&mut self, f: F) {
        self.on_connect = Some(Box::new(f));
    }

    pub fn on_disconnect<F: Fn() + Send + Sync + 'static>(&mut self, f: F) {
        self.on_disconnect = Some(Box::new(f));
    }

    pub fn event_sender(&self) -> mpsc::Sender<Event> {
        self.event_broadcast_tx.clone()
    }

    pub async fn run(mut self) {
        let clients: Arc<tokio::sync::Mutex<HashMap<u64, ClientHandle>>> =
            Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let mut next_id: u64 = 0;

        let clients_bc = Arc::clone(&clients);
        tokio::spawn(async move {
            while let Some(event) = self.event_broadcast_rx.recv().await {
                let mut clients = clients_bc.lock().await;
                let mut dead = Vec::new();
                for (&id, handle) in clients.iter() {
                    let dominated = event.topic() == "command";
                    if dominated {
                        continue;
                    }
                    if !handle.subscriptions.contains(event.topic()) {
                        continue;
                    }
                    if handle.event_tx.try_send(event.clone()).is_err() {
                        tracing::warn!(client_id = id, "client event queue full; disconnecting");
                        dead.push(id);
                    }
                }
                for id in dead {
                    clients.remove(&id);
                }
            }
        });

        let on_connect: Option<Arc<dyn Fn() + Send + Sync>> =
            self.on_connect.take().map(Arc::from);
        let on_disconnect: Option<Arc<dyn Fn() + Send + Sync>> =
            self.on_disconnect.take().map(Arc::from);

        loop {
            let (stream, _addr) = match self.listener.accept().await {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::warn!(error = %e, "accept failed");
                    continue;
                }
            };

            let id = next_id;
            next_id += 1;

            let caps = self.capabilities.clone();
            let verb_tx = self.verb_tx.clone();
            let clients = Arc::clone(&clients);
            let on_conn = on_connect.clone();
            let on_disc = on_disconnect.clone();

            tokio::spawn(async move {
                if let Some(ref f) = on_conn {
                    f();
                }
                tracing::info!(client_id = id, "client connected");

                match handle_client(stream, id, caps, verb_tx, &clients).await {
                    Ok(()) => tracing::info!(client_id = id, "client disconnected cleanly"),
                    Err(e) => tracing::warn!(client_id = id, error = %e, "client session error"),
                }

                clients.lock().await.remove(&id);
                if let Some(ref f) = on_disc {
                    f();
                }
            });
        }
    }
}

struct ClientHandle {
    subscriptions: HashSet<String>,
    event_tx: mpsc::Sender<Event>,
}

async fn handle_client(
    stream: UnixStream,
    id: u64,
    capabilities: Vec<String>,
    verb_tx: VerbSender,
    clients: &Arc<tokio::sync::Mutex<HashMap<u64, ClientHandle>>>,
) -> anyhow::Result<()> {
    let mut framed = Framed::new(stream, ControlCodec::new());

    let banner = ServerBanner::new(capabilities);
    framed.send(banner.to_line()).await?;

    let client_banner_line = match timeout(HANDSHAKE_TIMEOUT, framed.next()).await {
        Ok(Some(Ok(line))) => line,
        Ok(Some(Err(e))) => anyhow::bail!("handshake read error: {e}"),
        Ok(None) => anyhow::bail!("client disconnected before banner"),
        Err(_) => anyhow::bail!("handshake timeout ({}s)", HANDSHAKE_TIMEOUT.as_secs()),
    };

    let client_banner = ClientBanner::from_line(&client_banner_line)
        .map_err(|e| anyhow::anyhow!("malformed client banner: {e}"))?;

    if !client_banner.version.is_empty() {
        tracing::info!(
            client_id = id,
            client = %client_banner.client,
            version = %client_banner.version,
            "handshake complete"
        );
    }

    let (event_tx, mut event_rx) = mpsc::channel(CLIENT_EVENT_CAPACITY);

    let mut subscriptions: HashSet<String> = client_banner.subscribe.into_iter().collect();

    {
        let mut clients = clients.lock().await;
        clients.insert(
            id,
            ClientHandle {
                subscriptions: subscriptions.clone(),
                event_tx: event_tx.clone(),
            },
        );
    }

    let (mut sink, mut stream) = framed.split();

    let writer = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            if sink.send(event.to_line()).await.is_err() {
                break;
            }
        }
    });

    while let Some(result) = stream.next().await {
        let line = match result {
            Ok(line) => line,
            Err(e) => {
                tracing::debug!(client_id = id, error = %e, "read error");
                break;
            }
        };

        tracing::trace!(client_id = id, line = %line, "recv");

        let envelope = match VerbEnvelope::from_line(&line) {
            Ok(env) => env,
            Err(e) => {
                let err_event = Event::command_err("", format!("parse error: {e}"));
                let _ = event_tx.try_send(err_event);
                continue;
            }
        };

        let cmd_id = envelope.cmd_id.clone();

        match &envelope.verb {
            super::verbs::Verb::Quit => {
                let _ = event_tx.try_send(Event::command_ok(&cmd_id));
                tokio::time::sleep(Duration::from_millis(10)).await;
                break;
            }
            super::verbs::Verb::Subscribe { topic } => {
                subscriptions.insert(topic.clone());
                {
                    let mut clients = clients.lock().await;
                    if let Some(handle) = clients.get_mut(&id) {
                        handle.subscriptions.insert(topic.clone());
                    }
                }
                let _ = event_tx.try_send(Event::command_ok(&cmd_id));
            }
            super::verbs::Verb::Unsubscribe { topic } => {
                subscriptions.remove(topic);
                {
                    let mut clients = clients.lock().await;
                    if let Some(handle) = clients.get_mut(&id) {
                        handle.subscriptions.remove(topic);
                    }
                }
                let _ = event_tx.try_send(Event::command_ok(&cmd_id));
            }
            super::verbs::Verb::Capabilities => {
                let _ = event_tx.try_send(Event::command_ok(&cmd_id));
            }
            _ => {
                if verb_tx.send((envelope, event_tx.clone())).await.is_err() {
                    let _ = event_tx.try_send(Event::command_err(
                        &cmd_id,
                        "daemon shutting down",
                    ));
                    break;
                }
            }
        }
    }

    writer.abort();
    Ok(())
}
