//! BarsOutline — minimal spectrum showing the top edge of frequency bands
//! as a flowing line using box-drawing characters.

use crate::audio::FftSnapshot;
use crate::visualiser::cell_grid::{Cell, CellGrid, Rgb};
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{SurfaceKind, TuiContext, Visualiser, VisualiserId};

pub struct BarsOutline {
    energy: EnergyTracker,
    bar_heights: Vec<f32>,
    last_w: u16,
}

impl BarsOutline {
    pub fn new() -> Self {
        Self {
            energy: EnergyTracker::new(0.5, 0.88, 500.0),
            bar_heights: Vec::new(),
            last_w: 0,
        }
    }

    fn ensure_bands(&mut self, w: u16) {
        if w != self.last_w {
            self.bar_heights.clear();
            self.bar_heights.resize(w as usize, 0.0);
            self.last_w = w;
        }
    }

    fn bins_from_fft(&mut self, fft: &FftSnapshot, num_bands: usize) {
        let bin_count = fft.magnitudes.len().max(1);
        let max_log = ((bin_count - 1) as f32).ln().max(1.0);

        for band in 0..num_bands {
            let lo_log = (band as f32 / num_bands as f32) * max_log;
            let hi_log = ((band + 1) as f32 / num_bands as f32) * max_log;
            let lo = (lo_log.exp().round() as usize).min(bin_count - 1);
            let hi = (hi_log.exp().round() as usize).clamp(lo + 1, bin_count);
            let slice = &fft.magnitudes[lo..hi];
            let max_mag = slice.iter().cloned().fold(0.0_f32, f32::max);
            let compressed = (1.0 + max_mag / 1000.0).ln().min(1.0);

            let old = self.bar_heights[band];
            if compressed > old {
                self.bar_heights[band] = 0.5 * old + 0.5 * compressed; // attack
            } else {
                self.bar_heights[band] = 0.85 * old + 0.15 * compressed; // release
            }
        }
    }
}

impl Default for BarsOutline {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for BarsOutline {
    fn id(&self) -> VisualiserId {
        VisualiserId::BarsOutline
    }

    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Tui
    }

    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot) {
        let energy = self.energy.update(fft);
        let grid: &mut CellGrid = ctx.grid;
        let w = grid.width();
        let h = grid.height();
        if w == 0 || h == 0 {
            return;
        }

        self.ensure_bands(w);
        self.bins_from_fft(fft, w as usize);

        let bg = Rgb::new(2, 2, 6);
        grid.fill(Cell {
            ch: ' ',
            fg: Rgb::BLACK,
            bg,
        });

        let h_f = (h - 1) as f32;
        let num_bands = w as usize;

        // Compute the outline row for each column.
        let outline_rows: Vec<u16> = (0..num_bands)
            .map(|x| ((1.0 - self.bar_heights[x]) * h_f).round() as u16)
            .collect();

        // Colour: cyan/teal, brightness scaled with energy.
        let hue = 0.5;
        let sat = 0.7;
        let val = lerp(0.4, 1.0, energy);
        let (cr, cg, cb) = hsv_to_rgb(hue, sat, val);
        let fg_colour = Rgb::new(f32_to_u8(cr), f32_to_u8(cg), f32_to_u8(cb));

        // First pass: draw horizontal segments at each column's outline row.
        for (x, &row) in outline_rows.iter().enumerate().take(num_bands) {
            if row < h {
                grid.set(
                    x as u16,
                    row,
                    Cell {
                        ch: '\u{2500}',
                        fg: fg_colour,
                        bg,
                    },
                );
            }
        }

        // Second pass: draw vertical connectors between adjacent columns
        // and place corner characters.
        for x in 0..num_bands.saturating_sub(1) {
            let r0 = outline_rows[x];
            let r1 = outline_rows[x + 1];
            if r0 == r1 {
                // Same height, both stay as '─'.
                continue;
            }
            let (top, bot) = if r0 < r1 { (r0, r1) } else { (r1, r0) };
            // Fill vertical segments between the two rows (exclusive of
            // endpoints which get corner chars).
            for ry in (top + 1)..bot {
                if ry < h {
                    grid.set(
                        x as u16,
                        ry,
                        Cell {
                            ch: '\u{2502}',
                            fg: fg_colour,
                            bg,
                        },
                    );
                }
            }
            if r0 < r1 {
                // Line goes down to the right.
                if r0 < h {
                    grid.set(
                        x as u16,
                        r0,
                        Cell {
                            ch: '\u{256E}',
                            fg: fg_colour,
                            bg,
                        },
                    );
                }
                if r1 < h {
                    grid.set(
                        (x + 1) as u16,
                        r1,
                        Cell {
                            ch: '\u{2570}',
                            fg: fg_colour,
                            bg,
                        },
                    );
                }
            } else {
                // Line goes up to the right.
                if r0 < h {
                    grid.set(
                        x as u16,
                        r0,
                        Cell {
                            ch: '\u{256F}',
                            fg: fg_colour,
                            bg,
                        },
                    );
                }
                if r1 < h {
                    grid.set(
                        (x + 1) as u16,
                        r1,
                        Cell {
                            ch: '\u{256D}',
                            fg: fg_colour,
                            bg,
                        },
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loud_fft() -> FftSnapshot {
        FftSnapshot::new(vec![5000.0; 128], 48_000, 256)
    }

    fn silent_fft() -> FftSnapshot {
        FftSnapshot::new(vec![0.0; 128], 48_000, 256)
    }

    #[test]
    fn render_paints_cells() {
        let mut vis = BarsOutline::new();
        let fft = loud_fft();
        let mut grid = CellGrid::new(40, 12);
        // Render several frames so smoothing converges.
        for _ in 0..10 {
            let mut ctx = TuiContext { grid: &mut grid };
            vis.render_tui(&mut ctx, &fft);
        }
        let non_space = grid.cells().iter().filter(|c| c.ch != ' ').count();
        assert!(non_space > 0, "loud FFT should produce non-space cells");
    }

    #[test]
    fn output_changes_between_frames() {
        let mut vis = BarsOutline::new();
        let loud = loud_fft();
        let silent = silent_fft();

        let mut grid_a = CellGrid::new(40, 12);
        for _ in 0..10 {
            let mut ctx = TuiContext { grid: &mut grid_a };
            vis.render_tui(&mut ctx, &loud);
        }
        let snap_a = grid_a.snapshot();

        // Switch to silence and render more frames.
        for _ in 0..20 {
            let mut ctx = TuiContext { grid: &mut grid_a };
            vis.render_tui(&mut ctx, &silent);
        }

        let diff = snap_a
            .cells()
            .iter()
            .zip(grid_a.cells().iter())
            .filter(|(a, b)| a.ch != b.ch || a.fg != b.fg)
            .count();
        assert!(diff > 0, "frames with different input should differ");
    }

    #[test]
    fn resize_no_panic() {
        let mut vis = BarsOutline::new();
        let fft = loud_fft();
        let mut grid = CellGrid::new(80, 24);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            vis.render_tui(&mut ctx, &fft);
        }
        grid.resize(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            vis.render_tui(&mut ctx, &fft);
        }
    }

    #[test]
    fn silent_input_draws_flat_line() {
        let mut vis = BarsOutline::new();
        let fft = silent_fft();
        let mut grid = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            vis.render_tui(&mut ctx, &fft);
        }
        // With silent input all bar_heights are 0, so outline_row =
        // ((1.0 - 0.0) * 11.0).round() = 11 for every column — bottom row.
        let bottom_row_start = 11 * 40;
        let bottom_cells = &grid.cells()[bottom_row_start..bottom_row_start + 40];
        let all_horizontal = bottom_cells.iter().all(|c| c.ch == '\u{2500}');
        assert!(
            all_horizontal,
            "silent input should draw all '─' on the bottom row"
        );
    }
}
