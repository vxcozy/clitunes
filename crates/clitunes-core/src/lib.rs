//! clitunes-core — pure types shared by the engine and the binary crates.
//!
//! This crate intentionally has **no I/O**, **no feature gates**, **no async**,
//! **no GPU**, and **no platform-specific code**. Everything here must compile
//! on any target that supports `std`.
//!
//! The daemon-must-not-depend-on-visualiser invariant (D15) is enforced via
//! feature gates in `clitunes-engine`; this crate is safe to include from any
//! downstream binary.

pub mod pcm;
pub mod state;
pub mod station;
pub mod visualiser;

pub use pcm::{PcmFormat, StereoFrame};
pub use state::State;
pub use station::{CuratedStation, Station, StationUuid};
pub use visualiser::{SurfaceKind, VisualiserId};
