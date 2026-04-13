//! TUI overlay layer: persistence + picker + modal paint helpers.
//!
//! Unit 8 in the Slice 2 plan.
//!
//! Historically this module was going to pull in `ratatui` for the picker
//! overlay, but after the visualiser rewrite moved everything onto pure
//! `CellGrid` + `AnsiWriter` we paint overlays directly into the cell
//! grid. The `tui` Cargo feature is now just a gate for the overlay
//! code (so `clitunesd` can opt out), not a ratatui wrapper.
//!
//! # Layout
//!
//! - [`persistence`] is **always compiled**: the daemon needs to read
//!   `state.toml` on boot to auto-resume the last station, even though
//!   it doesn't render anything itself.
//! - [`picker`] is **visualiser-gated**: it paints into `CellGrid`, so
//!   without the visualiser feature there is no paint surface.

#[cfg(feature = "visualiser")]
pub mod components;
#[cfg(feature = "visualiser")]
pub mod micro;
pub mod persistence;
#[cfg(feature = "visualiser")]
pub mod theme;
#[cfg(feature = "visualiser")]
pub mod transition;

#[cfg(feature = "visualiser")]
pub mod picker;
