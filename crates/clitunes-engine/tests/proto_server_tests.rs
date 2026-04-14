#![cfg(feature = "control")]

use std::time::Duration;

use clitunes_engine::proto::banner::{ClientBanner, ServerBanner, PROTOCOL_VERSION};
use clitunes_engine::proto::client::ControlClient;
use clitunes_engine::proto::events::{Event, PlayState};
use clitunes_engine::proto::server::ControlServer;
use clitunes_engine::proto::verbs::{Verb, VerbEnvelope};

use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

static SOCKET_COUNTER: AtomicU64 = AtomicU64::new(0);

fn tmp_socket() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("clitunes-test");
    std::fs::create_dir_all(&dir).unwrap();
    let n = SOCKET_COUNTER.fetch_add(1, AtomicOrdering::SeqCst);
    dir.join(format!("test-{}-{}.sock", std::process::id(), n))
}

#[tokio::test]
async fn banner_exchange_happy_path() {
    let path = tmp_socket();
    let caps = vec!["radio".into(), "local".into()];
    let (server, _verb_rx) = ControlServer::bind(&path, caps.clone()).unwrap();

    tokio::spawn(server.run());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = ControlClient::connect(&path, "test-client", vec![])
        .await
        .unwrap();
    let banner = client.server_banner();
    assert_eq!(banner.version, PROTOCOL_VERSION);
    assert_eq!(banner.capabilities, caps);

    std::fs::remove_file(&path).ok();
}

#[tokio::test]
async fn verb_dispatch_and_command_result() {
    let path = tmp_socket();
    let (server, mut verb_rx) = ControlServer::bind(&path, vec![]).unwrap();

    tokio::spawn(server.run());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut client = ControlClient::connect(&path, "test", vec![]).await.unwrap();

    tokio::spawn(async move {
        while let Some((envelope, reply_tx)) = verb_rx.recv().await {
            let _ = reply_tx.try_send(Event::command_ok(&envelope.cmd_id));
        }
    });

    client
        .send(VerbEnvelope {
            cmd_id: "cmd-1".into(),
            verb: Verb::Play,
        })
        .await
        .unwrap();

    let event = client.recv_timeout(Duration::from_secs(2)).await.unwrap();
    match event {
        Event::CommandResult { cmd_id, ok, .. } => {
            assert_eq!(cmd_id, "cmd-1");
            assert!(ok);
        }
        other => panic!("expected CommandResult, got {other:?}"),
    }

    std::fs::remove_file(&path).ok();
}

#[tokio::test]
async fn subscribe_and_receive_broadcast_event() {
    let path = tmp_socket();
    let (server, _verb_rx) = ControlServer::bind(&path, vec![]).unwrap();
    let event_tx = server.event_sender();

    tokio::spawn(server.run());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut client = ControlClient::connect(&path, "test", vec!["now_playing".into()])
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let event = Event::NowPlayingChanged {
        artist: Some("Test Artist".into()),
        title: Some("Test Song".into()),
        album: None,
        station: None,
        raw_stream_title: None,
        art_url: None,
    };
    event_tx.send(event.clone()).await.unwrap();

    let received = client.recv_timeout(Duration::from_secs(2)).await.unwrap();
    assert_eq!(received, event);

    std::fs::remove_file(&path).ok();
}

#[tokio::test]
async fn unsubscribed_client_does_not_receive_event() {
    let path = tmp_socket();
    let (server, _verb_rx) = ControlServer::bind(&path, vec![]).unwrap();
    let event_tx = server.event_sender();

    tokio::spawn(server.run());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut client = ControlClient::connect(&path, "test", vec![]).await.unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let event = Event::NowPlayingChanged {
        artist: Some("Nobody".into()),
        title: None,
        album: None,
        station: None,
        raw_stream_title: None,
        art_url: None,
    };
    event_tx.send(event).await.unwrap();

    let received = client.recv_timeout(Duration::from_millis(200)).await;
    assert!(
        received.is_none(),
        "unsubscribed client should not get event"
    );

    std::fs::remove_file(&path).ok();
}

#[tokio::test]
async fn quit_verb_disconnects_cleanly() {
    let path = tmp_socket();
    let (server, _verb_rx) = ControlServer::bind(&path, vec![]).unwrap();

    tokio::spawn(server.run());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut client = ControlClient::connect(&path, "test", vec![]).await.unwrap();

    client
        .send(VerbEnvelope {
            cmd_id: "q-1".into(),
            verb: Verb::Quit,
        })
        .await
        .unwrap();

    let event = client.recv_timeout(Duration::from_secs(2)).await.unwrap();
    match event {
        Event::CommandResult { cmd_id, ok, .. } => {
            assert_eq!(cmd_id, "q-1");
            assert!(ok);
        }
        other => panic!("expected CommandResult for quit, got {other:?}"),
    }

    let after = client.recv_timeout(Duration::from_millis(200)).await;
    assert!(after.is_none(), "stream should be closed after quit");

    std::fs::remove_file(&path).ok();
}

#[tokio::test]
async fn malformed_json_returns_parse_error() {
    use futures_util::SinkExt;
    use tokio::net::UnixStream;
    use tokio_util::codec::Framed;

    let path = tmp_socket();
    let (server, _verb_rx) = ControlServer::bind(&path, vec![]).unwrap();

    tokio::spawn(server.run());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let stream = UnixStream::connect(&path).await.unwrap();
    let mut framed = Framed::new(stream, clitunes_engine::proto::control_codec());

    use futures_util::StreamExt;

    let server_banner = framed.next().await.unwrap().unwrap();
    let _banner = ServerBanner::from_line(&server_banner).unwrap();

    let client_banner = ClientBanner::new("raw-test", "0.1");
    framed.send(client_banner.to_line()).await.unwrap();

    framed.send("this is not json".to_string()).await.unwrap();

    let response = framed.next().await.unwrap().unwrap();
    let event: Event = Event::from_line(&response).unwrap();
    match event {
        Event::CommandResult { ok, error, .. } => {
            assert!(!ok);
            assert!(error.unwrap().contains("parse error"));
        }
        other => panic!("expected CommandResult error, got {other:?}"),
    }

    std::fs::remove_file(&path).ok();
}

#[tokio::test]
async fn subscribe_verb_adds_subscription_dynamically() {
    let path = tmp_socket();
    let (server, _verb_rx) = ControlServer::bind(&path, vec![]).unwrap();
    let event_tx = server.event_sender();

    tokio::spawn(server.run());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut client = ControlClient::connect(&path, "test", vec![]).await.unwrap();

    client
        .send(VerbEnvelope {
            cmd_id: "sub-1".into(),
            verb: Verb::Subscribe {
                topic: "state".into(),
            },
        })
        .await
        .unwrap();

    let ack = client.recv_timeout(Duration::from_secs(2)).await.unwrap();
    match &ack {
        Event::CommandResult { ok, .. } => assert!(ok),
        other => panic!("expected ack, got {other:?}"),
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    let state_event = Event::StateChanged {
        state: PlayState::Playing,
        source: Some("radio".into()),
        station_or_path: None,
        position_secs: None,
        duration_secs: None,
    };
    event_tx.send(state_event.clone()).await.unwrap();

    let received = client.recv_timeout(Duration::from_secs(2)).await.unwrap();
    assert_eq!(received, state_event);

    std::fs::remove_file(&path).ok();
}

#[tokio::test]
async fn multiple_clients_all_receive_broadcast() {
    let path = tmp_socket();
    let (server, _verb_rx) = ControlServer::bind(&path, vec![]).unwrap();
    let event_tx = server.event_sender();

    tokio::spawn(server.run());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut clients = Vec::new();
    for _ in 0..4 {
        let c = ControlClient::connect(&path, "test", vec!["now_playing".into()])
            .await
            .unwrap();
        clients.push(c);
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    let event = Event::NowPlayingChanged {
        artist: Some("Broadcast Test".into()),
        title: Some("Fan Out".into()),
        album: None,
        station: None,
        raw_stream_title: None,
        art_url: None,
    };
    event_tx.send(event.clone()).await.unwrap();

    for client in &mut clients {
        let received = client.recv_timeout(Duration::from_secs(2)).await.unwrap();
        assert_eq!(received, event);
    }

    std::fs::remove_file(&path).ok();
}

#[tokio::test]
async fn on_connect_and_disconnect_callbacks_fire() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    let path = tmp_socket();
    let (mut server, _verb_rx) = ControlServer::bind(&path, vec![]).unwrap();

    let connect_count = Arc::new(AtomicU32::new(0));
    let disconnect_count = Arc::new(AtomicU32::new(0));

    let cc = Arc::clone(&connect_count);
    server.on_connect(move || {
        cc.fetch_add(1, Ordering::SeqCst);
    });
    let dc = Arc::clone(&disconnect_count);
    server.on_disconnect(move || {
        dc.fetch_add(1, Ordering::SeqCst);
    });

    tokio::spawn(server.run());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut client = ControlClient::connect(&path, "test", vec![]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(connect_count.load(Ordering::SeqCst), 1);

    client
        .send(VerbEnvelope {
            cmd_id: "q".into(),
            verb: Verb::Quit,
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(disconnect_count.load(Ordering::SeqCst), 1);

    std::fs::remove_file(&path).ok();
}
