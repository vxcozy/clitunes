use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use clitunes::client::reconnect::ReconnectingSession;
use clitunes_engine::proto::events::{Event, PlayState};
use clitunes_engine::proto::server::ControlServer;

static SOCKET_COUNTER: AtomicU64 = AtomicU64::new(0);

fn tmp_socket() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("clitunes-test");
    std::fs::create_dir_all(&dir).unwrap();
    let n = SOCKET_COUNTER.fetch_add(1, Ordering::SeqCst);
    dir.join(format!("reconnect-{}-{}.sock", std::process::id(), n))
}

#[tokio::test]
async fn reconnecting_session_receives_events() {
    let path = tmp_socket();
    let (server, _verb_rx) = ControlServer::bind(&path, vec![]).unwrap();
    let event_tx = server.event_sender();

    tokio::spawn(server.run());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut session = ReconnectingSession::connect(path.clone()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let event = Event::StateChanged {
        state: PlayState::Playing,
        source: Some("tone".into()),
        station_or_path: None,
        position_secs: None,
        duration_secs: None,
    };
    event_tx.send(event.clone()).await.unwrap();

    let received = session
        .recv_event_timeout(Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(received, event);

    std::fs::remove_file(&path).ok();
}

#[tokio::test]
async fn reconnecting_session_reconnects_after_server_restart() {
    let path = tmp_socket();

    let (server1, _verb_rx1) = ControlServer::bind(&path, vec![]).unwrap();
    let event_tx1 = server1.event_sender();
    let server1_handle = tokio::spawn(server1.run());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut session = ReconnectingSession::connect(path.clone()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let event = Event::StateChanged {
        state: PlayState::Playing,
        source: Some("tone".into()),
        station_or_path: None,
        position_secs: None,
        duration_secs: None,
    };
    event_tx1.send(event.clone()).await.unwrap();
    let received = session
        .recv_event_timeout(Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(received, event);

    server1_handle.abort();
    drop(_verb_rx1);
    drop(event_tx1);
    tokio::time::sleep(Duration::from_millis(100)).await;
    std::fs::remove_file(&path).ok();

    let (server2, _verb_rx2) = ControlServer::bind(&path, vec![]).unwrap();
    let event_tx2 = server2.event_sender();
    tokio::spawn(server2.run());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let reconnected = session.recv_event_timeout(Duration::from_secs(3)).await;

    if reconnected.is_some() {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let event2 = Event::StateChanged {
            state: PlayState::Paused,
            source: Some("radio".into()),
            station_or_path: None,
            position_secs: None,
            duration_secs: None,
        };
        event_tx2.send(event2).await.unwrap();
    }

    std::fs::remove_file(&path).ok();
}
