//! Visualiser trait + implementations.
//!
//! All visualisers render into a CPU [`CellGrid`] using truecolor ANSI.
//! Cells carry an fg, a bg, and a glyph; the ANSI writer walks the grid
//! and emits `\x1b[38;2;…;48;2;…m` + glyph per cell, coalescing SGR
//! prefixes across adjacent same-colour cells.
//!
//! Each visualiser chooses its own cell style. [`Auralis`] uses upper-half
//! blocks (`▀`) for 2× vertical resolution on a bar spectrum. [`Plasma`]
//! uses a density ramp of ~70 glyphs so each cell also carries intensity
//! weight — the classic demoscene ASCII look. Future visualisers are free
//! to mix these strategies or add new ones.
//!
//! Shared infrastructure:
//! - [`cell_grid`] — the grid, cells, and Rgb primitive.
//! - [`palette`]   — HSV colour helpers.
//! - [`density_ramp`] — glyph ramps for ASCII-art style rendering.
//! - [`ansi_writer`]  — truecolor SGR emitter.

use crate::audio::FftSnapshot;
pub use clitunes_core::{SurfaceKind, VisualiserId};

pub mod ansi_writer;
pub mod auralis;
pub mod cell_grid;
pub mod density_ramp;
pub mod metaballs;
pub mod palette;
pub mod plasma;
pub mod ripples;
pub mod starfield;
pub mod tunnel;

pub use ansi_writer::AnsiWriter;
pub use auralis::Auralis;
pub use cell_grid::{Cell, CellGrid, Rgb};
pub use density_ramp::DensityRamp;
pub use metaballs::Metaballs;
pub use plasma::Plasma;
pub use ripples::Ripples;
pub use starfield::Starfield;
pub use tunnel::Tunnel;

/// Context passed to a visualiser each frame. The visualiser paints into
/// the mutable `grid`; the main loop's ANSI writer emits it afterwards.
pub struct TuiContext<'a> {
    pub grid: &'a mut CellGrid,
}

pub trait Visualiser {
    fn id(&self) -> VisualiserId;
    fn surface(&self) -> SurfaceKind;
    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot);
}
