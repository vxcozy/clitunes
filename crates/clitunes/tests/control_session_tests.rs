use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use clitunes::client::control_session::ControlSession;
use clitunes_engine::proto::events::{Event, PlayState};
use clitunes_engine::proto::server::ControlServer;
use clitunes_engine::proto::verbs::Verb;

static SOCKET_COUNTER: AtomicU64 = AtomicU64::new(0);

fn tmp_socket() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("clitunes-test");
    std::fs::create_dir_all(&dir).unwrap();
    let n = SOCKET_COUNTER.fetch_add(1, Ordering::SeqCst);
    dir.join(format!(
        "ctrl-session-{}-{}.sock",
        std::process::id(),
        n
    ))
}

#[tokio::test]
async fn control_session_connect_and_play() {
    let path = tmp_socket();
    let (server, mut verb_rx) = ControlServer::bind(&path, vec!["play".into()]).unwrap();

    tokio::spawn(server.run());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut session = ControlSession::connect(&path).await.unwrap();

    tokio::spawn(async move {
        while let Some((envelope, reply_tx)) = verb_rx.recv().await {
            let _ = reply_tx.try_send(Event::command_ok(&envelope.cmd_id));
        }
    });

    session.play().await.unwrap();

    let event = session
        .recv_event_timeout(Duration::from_secs(2))
        .await
        .unwrap();
    match event {
        Event::CommandResult { ok, .. } => assert!(ok),
        other => panic!("expected CommandResult, got {other:?}"),
    }

    std::fs::remove_file(&path).ok();
}

#[tokio::test]
async fn control_session_receives_broadcast() {
    let path = tmp_socket();
    let (server, _verb_rx) = ControlServer::bind(&path, vec![]).unwrap();
    let event_tx = server.event_sender();

    tokio::spawn(server.run());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut session = ControlSession::connect(&path).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let state_event = Event::StateChanged {
        state: PlayState::Playing,
        source: Some("radio".into()),
        station_or_path: Some("Test FM".into()),
        position_secs: None,
        duration_secs: None,
    };
    event_tx.send(state_event.clone()).await.unwrap();

    let received = session
        .recv_event_timeout(Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(received, state_event);

    std::fs::remove_file(&path).ok();
}

#[tokio::test]
async fn control_session_status_request() {
    let path = tmp_socket();
    let (server, mut verb_rx) = ControlServer::bind(&path, vec!["status".into()]).unwrap();

    tokio::spawn(server.run());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut session = ControlSession::connect(&path).await.unwrap();

    tokio::spawn(async move {
        while let Some((envelope, reply_tx)) = verb_rx.recv().await {
            if matches!(envelope.verb, Verb::Status) {
                let _ = reply_tx.try_send(Event::PcmTap {
                    shm_name: "test-shm".into(),
                    sample_rate: 48000,
                    channels: 2,
                    capacity: 65536,
                });
                let _ = reply_tx.try_send(Event::command_ok(&envelope.cmd_id));
            }
        }
    });

    session.request_status().await.unwrap();

    let event = session
        .recv_event_timeout(Duration::from_secs(2))
        .await
        .unwrap();
    match event {
        Event::PcmTap {
            shm_name,
            sample_rate,
            ..
        } => {
            assert_eq!(shm_name, "test-shm");
            assert_eq!(sample_rate, 48000);
        }
        other => panic!("expected PcmTap, got {other:?}"),
    }

    std::fs::remove_file(&path).ok();
}

#[tokio::test]
async fn control_session_eof_returns_none() {
    let path = tmp_socket();
    let (server, _verb_rx) = ControlServer::bind(&path, vec![]).unwrap();

    let server_handle = tokio::spawn(server.run());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut session = ControlSession::connect(&path).await.unwrap();

    server_handle.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = session
        .recv_event_timeout(Duration::from_millis(500))
        .await;
    assert!(result.is_none());

    std::fs::remove_file(&path).ok();
}
