//! clitunes-engine — functional engine for clitunes.
//!
//! Everything here is behind a Cargo feature gate. The daemon binary
//! (`clitunesd`) only enables `audio`, `sources`, `control`; the full
//! `clitunes` binary enables the visualiser, tui, and layout features too.
//! This preserves the D15 invariant that the daemon never pulls wgpu,
//! ratatui, or crossterm into its dependency tree.
//!
//! The CI grep `cargo tree -e features --bin clitunesd | grep -qE 'wgpu|ratatui|crossterm'`
//! must return non-zero. See `.github/workflows/ci.yml`.

pub mod observability;

#[cfg(feature = "audio")]
pub mod audio;

#[cfg(feature = "sources")]
pub mod sources;

#[cfg(feature = "visualiser")]
pub mod visualiser;
