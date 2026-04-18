/// 80s synthwave scene: striped sun, audio-reactive wave, perspective grid floor.
use crate::audio::FftSnapshot;
use crate::visualiser::braille::BrailleBuffer;
use crate::visualiser::cell_grid::CellGrid;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

pub struct Retro {
    energy: EnergyTracker,
    braille: BrailleBuffer,
    frame: u32,
    last_w: u16,
    last_h: u16,
}

impl Retro {
    pub fn new() -> Self {
        Self {
            // Release tau ~115 ms so the sun radius pulses with the music
            // rather than gliding smoothly like a second-long envelope.
            energy: EnergyTracker::new(0.5, 0.75, 500.0),
            braille: BrailleBuffer::new(1, 1),
            frame: 0,
            last_w: 0,
            last_h: 0,
        }
    }
}

impl Default for Retro {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Retro {
    fn id(&self) -> VisualiserId {
        VisualiserId::Retro
    }

    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Tui
    }

    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot) {
        let energy = self.energy.update(fft);
        self.frame = self.frame.wrapping_add(1);

        let grid: &mut CellGrid = ctx.grid;
        let w = grid.width();
        let h = grid.height();
        if w == 0 || h == 0 {
            return;
        }

        if w != self.last_w || h != self.last_h {
            self.braille.resize(w, h);
            self.last_w = w;
            self.last_h = h;
        }
        self.braille.clear();

        let bw = self.braille.width() as i32;
        let bh = self.braille.height() as i32;
        if bw == 0 || bh == 0 {
            return;
        }
        let min_dim = bw.min(bh) as f32;

        let horizon_y = (bh as f32 * 0.55) as i32;
        let center_x = bw / 2;

        // --- Layer 1: perspective grid floor (bottom 45%) ---
        {
            // Horizontal lines with perspective spacing, scrolling toward viewer.
            let floor_span = bh - horizon_y;
            if floor_span > 0 {
                let num_h_lines = 10u32;
                let scroll = (self.frame % 60) as f32 / 60.0;
                for i in 0..num_h_lines {
                    let t = (i as f32 + scroll) / num_h_lines as f32;
                    let y_off = (t * t * floor_span as f32) as i32;
                    let y = horizon_y + y_off;
                    if y >= 0 && y < bh {
                        self.braille.line(0, y, bw - 1, y);
                    }
                }

                // Vertical lines fanning from vanishing point.
                let num_v_lines = 12;
                for i in 0..num_v_lines {
                    let t = (i as f32 + 0.5) / num_v_lines as f32;
                    let spread = (t - 0.5) * 2.0;
                    let bottom_x = center_x as f32 + spread * bw as f32 * 0.8;
                    self.braille
                        .line(center_x, horizon_y, bottom_x as i32, bh - 1);
                }
            }
        }

        // --- Layer 2: striped sun ---
        {
            let sun_cx = bw as f32 / 2.0;
            let sun_cy = bh as f32 * 0.35;
            let radius = min_dim * 0.15 + energy * min_dim * 0.08;
            let r_sq = radius * radius;
            let stripe_period = (radius * 0.25).max(3.0) as i32;
            let stripe_gap = (stripe_period as f32 * 0.4).max(1.0) as i32;

            let y0 = ((sun_cy - radius) as i32).max(0);
            let y1 = ((sun_cy + radius) as i32).min(bh - 1);
            let x0 = ((sun_cx - radius) as i32).max(0);
            let x1 = ((sun_cx + radius) as i32).min(bw - 1);

            for y in y0..=y1 {
                let dy = y as f32 - sun_cy;
                // Stripe gaps: horizontal bands across the sun.
                let row_in_sun = (y as f32 - (sun_cy - radius)) as i32;
                if stripe_period > 0 && (row_in_sun % stripe_period) < stripe_gap {
                    continue;
                }
                for x in x0..=x1 {
                    let dx = x as f32 - sun_cx;
                    if dx * dx + dy * dy <= r_sq {
                        self.braille.set(x as u16, y as u16, true);
                    }
                }
            }
        }

        // --- Layer 3: audio wave at horizon ---
        {
            let mags = &fft.magnitudes;
            let num_bands = mags.len().max(1);
            let amplitude_scale = min_dim * 0.12;

            for x in 0..bw {
                let band_idx = ((x as usize) * num_bands / (bw as usize).max(1)).min(num_bands - 1);
                let mag = (1.0 + mags[band_idx] / 500.0).ln().min(1.0);
                let offset = (mag * amplitude_scale) as i32;
                let y = (horizon_y - offset).clamp(0, bh - 1);
                self.braille.set(x as u16, y as u16, true);
            }
        }

        // --- Compose colours ---
        let cell_h = h;
        let horizon_cell = (cell_h as f32 * 0.55) as u16;
        let sun_center_cell = (cell_h as f32 * 0.35) as u16;
        let sun_radius_cells = ((min_dim * 0.15 + energy * min_dim * 0.08) / 4.0) as u16;
        let sun_top = sun_center_cell.saturating_sub(sun_radius_cells);
        let sun_bottom = (sun_center_cell + sun_radius_cells).min(cell_h);

        let bg = Rgb::new(4, 0, 8);
        self.braille.compose(grid, |_cx, cy, dot_count| {
            let t = dot_count as f32 / 8.0;
            let fg = if cy >= horizon_cell {
                // Grid region: cyan/magenta
                let (r, g, b) = hsv_to_rgb(0.85, 0.7, lerp(0.15, 0.6, t));
                Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b))
            } else if cy >= sun_top && cy <= sun_bottom {
                // Sun region: orange/yellow
                let (r, g, b) = hsv_to_rgb(0.08, 0.9, lerp(0.5, 1.0, t));
                Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b))
            } else {
                // Sky region: dark purple-blue
                let (r, g, b) = hsv_to_rgb(0.75, 0.6, lerp(0.05, 0.15, t));
                Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b))
            };
            (fg, bg)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loud_fft() -> FftSnapshot {
        FftSnapshot::new(vec![5000.0; 128], 48_000, 256)
    }

    #[test]
    fn render_paints_cells() {
        let mut retro = Retro::new();
        let fft = loud_fft();
        let mut grid = CellGrid::new(40, 12);
        for _ in 0..5 {
            let mut ctx = TuiContext { grid: &mut grid };
            retro.render_tui(&mut ctx, &fft);
        }
        let braille_count = grid
            .cells()
            .iter()
            .filter(|c| c.ch != '\u{2800}' && c.ch != ' ')
            .count();
        assert!(
            braille_count > 0,
            "loud FFT should produce non-blank braille cells, got {braille_count}"
        );
    }

    #[test]
    fn output_changes_between_frames() {
        let mut retro = Retro::new();
        let fft = loud_fft();

        let mut grid_a = CellGrid::new(30, 10);
        {
            let mut ctx = TuiContext { grid: &mut grid_a };
            retro.render_tui(&mut ctx, &fft);
        }

        let mut grid_b = CellGrid::new(30, 10);
        {
            let mut ctx = TuiContext { grid: &mut grid_b };
            retro.render_tui(&mut ctx, &fft);
        }

        let diff = grid_a
            .cells()
            .iter()
            .zip(grid_b.cells().iter())
            .filter(|(a, b)| a.ch != b.ch)
            .count();
        assert!(
            diff > 0,
            "consecutive frames should differ due to grid scroll"
        );
    }

    #[test]
    fn resize_no_panic() {
        let mut retro = Retro::new();
        let fft = loud_fft();

        let mut grid = CellGrid::new(80, 24);
        let mut ctx = TuiContext { grid: &mut grid };
        retro.render_tui(&mut ctx, &fft);

        let mut grid = CellGrid::new(40, 12);
        let mut ctx = TuiContext { grid: &mut grid };
        retro.render_tui(&mut ctx, &fft);
    }
}
