/// Pulsating filled circle with shockwave rings on beat transients.
use crate::audio::FftSnapshot;
use crate::visualiser::braille::BrailleBuffer;
use crate::visualiser::cell_grid::CellGrid;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

const RING_EXPAND_SPEED: f32 = 1.5;
const RING_FADE: f32 = 0.03;
const TRANSIENT_THRESHOLD: f32 = 0.15;

struct Ring {
    radius: f32,
    brightness: f32,
}

pub struct Pulse {
    energy: EnergyTracker,
    braille: BrailleBuffer,
    prev_energy: f32,
    rings: Vec<Ring>,
    last_w: u16,
    last_h: u16,
}

impl Pulse {
    pub fn new() -> Self {
        Self {
            energy: EnergyTracker::new(0.3, 0.85, 500.0),
            braille: BrailleBuffer::new(1, 1),
            prev_energy: 0.0,
            rings: Vec::new(),
            last_w: 0,
            last_h: 0,
        }
    }

    fn ensure_buf(&mut self, w: u16, h: u16) {
        if self.last_w != w || self.last_h != h {
            self.braille.resize(w, h);
            self.last_w = w;
            self.last_h = h;
        }
    }
}

impl Default for Pulse {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Pulse {
    fn id(&self) -> VisualiserId {
        VisualiserId::Pulse
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

        self.ensure_buf(w, h);
        self.braille.clear();

        let bw = self.braille.width();
        let bh = self.braille.height();
        let cx = bw / 2;
        let cy = bh / 2;

        // Compute main circle radius based on energy.
        let min_dim = cx.min(cy) as f32;
        let base_radius = min_dim * 0.3 + energy * min_dim * 0.5;

        // Detect transient and spawn shockwave ring.
        if energy - self.prev_energy > TRANSIENT_THRESHOLD {
            self.rings.push(Ring {
                radius: base_radius,
                brightness: 1.0,
            });
        }
        self.prev_energy = energy;

        // Update rings: expand and fade.
        for ring in &mut self.rings {
            ring.radius += RING_EXPAND_SPEED;
            ring.brightness -= RING_FADE;
        }
        self.rings.retain(|r| r.brightness >= 0.05);

        // Draw filled circle and ring outlines in braille sub-pixel space.
        let cx_f = cx as f32;
        let cy_f = cy as f32;
        for y in 0..bh {
            let dy = y as f32 - cy_f;
            for x in 0..bw {
                let dx = x as f32 - cx_f;
                let dist = (dx * dx + dy * dy).sqrt();

                // Filled circle.
                if dist <= base_radius {
                    self.braille.set(x, y, true);
                    continue;
                }

                // Shockwave ring outlines.
                for ring in &self.rings {
                    if (dist - ring.radius).abs() < 1.5 {
                        self.braille.set(x, y, true);
                        break;
                    }
                }
            }
        }

        // Compose into grid with radial colour gradient.
        let cell_w = w as f32;
        let cell_h = h as f32;

        self.braille.compose(grid, |cell_x, cell_y, dot_count| {
            let cx_cell = cell_x as f32 / cell_w - 0.5;
            let cy_cell = cell_y as f32 / cell_h - 0.5;
            let cell_dist = (cx_cell * cx_cell + cy_cell * cy_cell * 4.0).sqrt();

            let hue = lerp(0.33, 0.0, cell_dist.min(1.0));
            let sat = 0.8;
            let val = lerp(0.8, 0.4, cell_dist.min(1.0)) * (dot_count as f32 / 8.0).max(0.1);

            let (r, g, b) = hsv_to_rgb(hue, sat, val);
            let fg = Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b));
            let bg = Rgb::new(2, 4, 2);
            (fg, bg)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loud_fft() -> FftSnapshot {
        FftSnapshot::new(vec![5000.0; 64], 48_000, 128)
    }

    fn silent_fft() -> FftSnapshot {
        FftSnapshot::new(vec![0.0; 64], 48_000, 128)
    }

    #[test]
    fn render_paints_cells() {
        let mut pulse = Pulse::new();
        let fft = loud_fft();
        // Feed a few frames so energy ramps up.
        let mut grid = CellGrid::new(40, 12);
        for _ in 0..10 {
            let mut ctx = TuiContext { grid: &mut grid };
            pulse.render_tui(&mut ctx, &fft);
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
        let mut pulse = Pulse::new();
        let silent = silent_fft();
        let loud = loud_fft();

        let mut grid_a = CellGrid::new(30, 10);
        for _ in 0..5 {
            let mut ctx = TuiContext { grid: &mut grid_a };
            pulse.render_tui(&mut ctx, &silent);
        }
        // Snapshot after silence.
        let mut grid_a = CellGrid::new(30, 10);
        {
            let mut ctx = TuiContext { grid: &mut grid_a };
            pulse.render_tui(&mut ctx, &silent);
        }

        // Feed loud frames.
        for _ in 0..10 {
            let mut grid = CellGrid::new(30, 10);
            let mut ctx = TuiContext { grid: &mut grid };
            pulse.render_tui(&mut ctx, &loud);
        }
        let mut grid_b = CellGrid::new(30, 10);
        {
            let mut ctx = TuiContext { grid: &mut grid_b };
            pulse.render_tui(&mut ctx, &loud);
        }

        let diff = grid_a
            .cells()
            .iter()
            .zip(grid_b.cells().iter())
            .filter(|(a, b)| a.ch != b.ch)
            .count();
        assert!(diff > 0, "different inputs should produce different output");
    }

    #[test]
    fn resize_no_panic() {
        let mut pulse = Pulse::new();
        let fft = loud_fft();

        let mut grid = CellGrid::new(80, 24);
        let mut ctx = TuiContext { grid: &mut grid };
        pulse.render_tui(&mut ctx, &fft);

        let mut grid = CellGrid::new(40, 12);
        let mut ctx = TuiContext { grid: &mut grid };
        pulse.render_tui(&mut ctx, &fft);
    }

    #[test]
    fn shockwave_spawns_on_transient() {
        let mut pulse = Pulse::new();
        let silent = silent_fft();
        let loud = loud_fft();

        // Feed silence to establish low baseline.
        let mut grid = CellGrid::new(40, 12);
        for _ in 0..10 {
            let mut ctx = TuiContext { grid: &mut grid };
            pulse.render_tui(&mut ctx, &silent);
        }

        // Sudden loud input should trigger a transient.
        let mut ctx = TuiContext { grid: &mut grid };
        pulse.render_tui(&mut ctx, &loud);

        assert!(
            !pulse.rings.is_empty(),
            "sudden loud input after silence should spawn at least one ring"
        );
    }
}
