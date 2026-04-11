//! Tracing + structured logging convention (bead clitunes-xb2).
//!
//! Every binary calls `init_tracing(component)` once at startup. Logs go to
//! stderr using either the compact text format (default) or JSON (for the
//! e2e harness, gated by `CLITUNES_LOG_FORMAT=json`).
//!
//! The logging convention is documented in `docs/conventions/logging.md`:
//! use spans for durations and structured fields for events; never interpolate
//! values into message strings; never log PII (lyrics, full local file paths,
//! user IP addresses).

use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, EnvFilter};

pub mod spans {
    // Top-level components
    pub const DAEMON_STARTUP: &str = "clitunesd.startup";
    pub const DAEMON_IDLE: &str = "clitunesd.idle_timer";
    pub const DAEMON_CLIENT: &str = "clitunesd.client_session";

    // Audio pipeline
    pub const AUDIO_DECODER: &str = "audio.decoder";
    pub const AUDIO_RING_WRITE: &str = "audio.ring.write";
    pub const AUDIO_RING_READ: &str = "audio.ring.read";
    pub const AUDIO_OUTPUT: &str = "audio.output";

    // Radio
    pub const RADIO_STREAM: &str = "radio.stream";
    pub const RADIO_ICY_PARSE: &str = "radio.icy_parse";
    pub const RADIO_RECONNECT: &str = "radio.reconnect";

    // TUI
    pub const TUI_FRAME: &str = "tui.frame";
    pub const TUI_PICKER: &str = "tui.picker";
    pub const TUI_LAYOUT: &str = "tui.layout";

    // Visualisers
    pub const VIZ_AURALIS: &str = "viz.auralis.frame";
    pub const VIZ_TIDELINE: &str = "viz.tideline.frame";
    pub const VIZ_CASCADE: &str = "viz.cascade.frame";

    // Control bus
    pub const CONTROL_REQUEST: &str = "control.request";
    pub const CONTROL_RESPONSE: &str = "control.response";
}

/// Initialise the global tracing subscriber. Returns `Ok(())` on success;
/// returns an error if called twice (reconfiguring tracing at runtime is
/// intentionally not supported).
pub fn init_tracing(component: &str) -> anyhow::Result<()> {
    let default_filter = format!("clitunes=info,clitunes_engine=info,{component}=info,warn");
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));

    let format_env = std::env::var("CLITUNES_LOG_FORMAT").unwrap_or_default();
    let is_json = format_env == "json";

    if is_json {
        tracing_subscriber::registry()
            .with(filter)
            .with(
                fmt::layer()
                    .with_target(true)
                    .with_thread_ids(true)
                    .with_line_number(true)
                    .with_writer(std::io::stderr)
                    .json(),
            )
            .try_init()
            .map_err(|e| anyhow::anyhow!("tracing init: {e}"))?;
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(
                fmt::layer()
                    .with_target(true)
                    .with_thread_ids(false)
                    .with_line_number(false)
                    .with_writer(std::io::stderr),
            )
            .try_init()
            .map_err(|e| anyhow::anyhow!("tracing init: {e}"))?;
    }

    tracing::info!(component = component, "tracing initialised");
    Ok(())
}
