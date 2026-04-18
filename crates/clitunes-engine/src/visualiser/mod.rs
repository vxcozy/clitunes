//! Visualiser trait + implementations.
//!
//! All visualisers render into a CPU [`CellGrid`] using truecolor ANSI.
//! Cells carry an fg, a bg, and a glyph; the ANSI writer walks the grid
//! and emits `\x1b[38;2;…;48;2;…m` + glyph per cell, coalescing SGR
//! prefixes across adjacent same-colour cells.
//!
//! Each visualiser chooses its own cell style. [`Plasma`] uses a density
//! ramp of ~70 glyphs so each cell also carries intensity weight — the
//! classic demoscene ASCII look. Other modes use upper-half blocks (`▀`)
//! for 2× vertical resolution, or Unicode braille sub-pixels (4× vertical
//! via U+2800). Future visualisers are free to mix these or add new ones.
//!
//! Shared infrastructure:
//! - [`cell_grid`] — the grid, cells, and Rgb primitive.
//! - [`palette`]   — HSV colour helpers.
//! - [`density_ramp`] — glyph ramps for ASCII-art style rendering.
//! - [`ansi_writer`]  — truecolor SGR emitter.

use crate::audio::FftSnapshot;
pub use clitunes_core::{SurfaceKind, VisualiserId};

pub mod ansi_writer;
pub mod bars_dot;
pub mod bars_outline;
pub mod binary;
pub mod braille;
pub mod butterfly;
pub mod cell_grid;
pub mod classic_peak;
pub mod density_ramp;
pub mod energy;
pub mod fire;
pub mod firework;
pub mod heartbeat;
pub mod matrix;
pub mod metaballs;
pub mod moire;
pub mod palette;
pub mod plasma;
pub mod pulse;
pub mod rain;
pub mod retro;
pub mod ripples;
pub mod sakura;
pub mod scatter;
pub mod scope;
pub mod terrain;
pub mod tunnel;
pub mod vortex;
pub mod wave;

pub use ansi_writer::AnsiWriter;
pub use bars_dot::BarsDot;
pub use bars_outline::BarsOutline;
pub use binary::Binary;
pub use butterfly::Butterfly;
pub use cell_grid::{Cell, CellGrid, Rgb};
pub use classic_peak::ClassicPeak;
pub use density_ramp::DensityRamp;
pub use fire::Fire;
pub use firework::Firework;
pub use heartbeat::Heartbeat;
pub use matrix::Matrix;
pub use metaballs::Metaballs;
pub use moire::Moire;
pub use plasma::Plasma;
pub use pulse::Pulse;
pub use rain::Rain;
pub use retro::Retro;
pub use ripples::Ripples;
pub use sakura::Sakura;
pub use scatter::Scatter;
pub use scope::Scope;
pub use terrain::Terrain;
pub use tunnel::Tunnel;
pub use vortex::Vortex;
pub use wave::Wave;

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
