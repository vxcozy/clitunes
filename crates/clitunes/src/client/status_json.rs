//! `clitunes status --json` — one-shot status query.
//!
//! Connects, sends the `Status` verb, collects state events (PcmTap,
//! StateChanged, NowPlayingChanged), serialises them into a single
//! JSON object, and prints to stdout.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;

use super::control_session::ControlSession;
use clitunes_engine::proto::events::{Event, PlayState};

#[derive(Serialize)]
struct StatusOutput {
    state: String,
    source: Option<String>,
    station_or_path: Option<String>,
    artist: Option<String>,
    title: Option<String>,
    album: Option<String>,
    position_secs: Option<f64>,
    duration_secs: Option<f64>,
    volume: Option<u8>,
    visualiser: Option<String>,
    shm_name: Option<String>,
    sample_rate: Option<u32>,
}

pub async fn run(socket_path: &Path) -> Result<()> {
    let mut session = ControlSession::connect(socket_path)
        .await
        .context("connect to daemon")?;

    session.request_status().await.context("request status")?;

    let mut output = StatusOutput {
        state: "unknown".into(),
        source: None,
        station_or_path: None,
        artist: None,
        title: None,
        album: None,
        position_secs: None,
        duration_secs: None,
        volume: None,
        visualiser: None,
        shm_name: None,
        sample_rate: None,
    };

    // Collect events until we get CommandResult (end of status response).
    for _ in 0..50 {
        match session.recv_event_timeout(Duration::from_millis(100)).await {
            Some(Event::StateChanged {
                state,
                source,
                station_or_path,
                position_secs,
                duration_secs,
            }) => {
                output.state = match state {
                    PlayState::Playing => "playing",
                    PlayState::Paused => "paused",
                    PlayState::Stopped => "stopped",
                }
                .into();
                output.source = source;
                output.station_or_path = station_or_path;
                output.position_secs = position_secs;
                output.duration_secs = duration_secs;
            }
            Some(Event::NowPlayingChanged {
                artist,
                title,
                album,
                ..
            }) => {
                output.artist = artist;
                output.title = title;
                output.album = album;
            }
            Some(Event::VolumeChanged { volume }) => {
                output.volume = Some(volume);
            }
            Some(Event::VizChanged { name }) => {
                output.visualiser = Some(name);
            }
            Some(Event::PcmTap {
                shm_name,
                sample_rate,
                ..
            }) => {
                output.shm_name = Some(shm_name);
                output.sample_rate = Some(sample_rate);
            }
            Some(Event::CommandResult { ok: true, .. }) => break,
            Some(Event::CommandResult {
                ok: false, error, ..
            }) => {
                let msg = error.unwrap_or_else(|| "unknown error".into());
                anyhow::bail!("status failed: {msg}");
            }
            Some(_) => continue,
            None => continue,
        }
    }

    let json = serde_json::to_string_pretty(&output).context("serialise status")?;
    println!("{json}");
    Ok(())
}
