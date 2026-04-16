//! Integration tests for the Cascade spectrogram waterfall visualiser.

use clitunes_engine::audio::FftSnapshot;
use clitunes_engine::visualiser::cascade::Cascade;
use clitunes_engine::visualiser::{Cell, CellGrid, Rgb, TuiContext, Visualiser};

#[test]
fn cascade_accumulates_history_and_renders() {
    let mut cascade = Cascade::new();
    let fft = FftSnapshot::new(vec![500.0; 128], 48_000, 256);
    let mut grid = CellGrid::new(40, 12);

    // Push enough frames to fill the grid (12 rows * 2 virtual pixels = 24 history rows).
    for _ in 0..30 {
        let mut ctx = TuiContext { grid: &mut grid };
        cascade.render_tui(&mut ctx, &fft);
    }

    // Every cell should have upper-block glyph.
    for c in grid.cells() {
        assert_eq!(c.ch, Cell::UPPER_BLOCK);
    }

    // With non-zero FFT data, cells should have non-black colours.
    let non_black = grid
        .cells()
        .iter()
        .filter(|c| c.fg != Rgb::BLACK || c.bg != Rgb::BLACK)
        .count();
    assert!(
        non_black > 0,
        "expected coloured cells from non-zero FFT data"
    );
}

#[test]
fn cascade_renders_single_frame_without_panic() {
    let mut cascade = Cascade::new();
    let fft = FftSnapshot::new(vec![100.0; 64], 44_100, 128);
    let mut grid = CellGrid::new(20, 8);
    let mut ctx = TuiContext { grid: &mut grid };
    cascade.render_tui(&mut ctx, &fft);

    // First frame: most of the grid is black (history too short), but
    // the bottom row should have at least one coloured virtual pixel.
    let bottom_row_start = (grid.height() as usize - 1) * grid.width() as usize;
    let bottom_cells = &grid.cells()[bottom_row_start..];
    let any_colour = bottom_cells
        .iter()
        .any(|c| c.fg != Rgb::BLACK || c.bg != Rgb::BLACK);
    assert!(any_colour, "bottom row should show first history frame");
}
