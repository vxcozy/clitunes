//! Daemon runtime support: process lifecycle, flock-based singleton,
//! idle-exit timer, log rotation.
//!
//! This module is gated behind the `daemon` feature (which implies
//! `control`) so the clitunesd binary can pull it in without dragging
//! visualiser/tui/ratatui/crossterm into its dependency graph (D15).
//!
//! ## Unit 9 scope
//! - `lifecycle`: double-fork, setsid, stdio redirect, umask(0o177)
//!   TOCTOU fix, runtime dir resolution.
//! - `singleton`: exclusive non-blocking flock at
//!   `$runtime_dir/clitunesd.lock`.
//! - `idle_timer`: pure state machine tracking client count and an
//!   injectable clock. Wired up by Unit 10 (control socket).
//! - `log_rotate`: size-triggered file rotation backing the tracing
//!   subscriber.
//!
//! The wire protocol for the control socket is deliberately *not* here
//! — it belongs to Unit 10 — but the idle timer exposes the API that
//! socket code will call (`on_client_connected` / `on_client_disconnected`).

pub mod config;
#[cfg(all(feature = "audio", feature = "radio", feature = "decode"))]
pub mod event_loop;
pub mod idle_timer;
pub mod lifecycle;
pub mod log_rotate;
pub mod peercred;
pub mod singleton;
pub mod socket_security;
#[cfg(feature = "audio")]
pub mod tee_writer;

pub use config::{
    config_path_from_env, default_config_path, resolve_config_path, BindMode, ConnectConfig,
    DaemonConfig, CONFIG_PATH_ENV,
};
pub use idle_timer::{Clock, IdleState, IdleTimer, SystemClock, Tick, DEFAULT_IDLE_WINDOW};
pub use lifecycle::{
    detach, open_private, redirect_stdio_to_devnull, runtime_dir, set_socket_umask, write_pidfile,
    DetachOutcome,
};
pub use log_rotate::{default_log_path, RotatingLog, DEFAULT_MAX_BACKUPS, DEFAULT_MAX_BYTES};
pub use peercred::{my_uid, peer_cred, PeerCred};
pub use singleton::{acquire_at, acquire_default, AcquireOutcome, DaemonLock};
pub use socket_security::{check_peer, AcceptGuard};
#[cfg(feature = "audio")]
pub use tee_writer::TeeWriter;
