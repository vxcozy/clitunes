use crate::audio::FftSnapshot;
use crate::visualiser::braille::BrailleBuffer;
use crate::visualiser::cell_grid::CellGrid;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

const MAX_PETALS: usize = 100;

struct Petal {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    size: u8,
    phase: f32,
}

pub struct Sakura {
    energy: EnergyTracker,
    braille: BrailleBuffer,
    petals: Vec<Petal>,
    frame: u32,
    rng: u32,
    last_w: u16,
    last_h: u16,
}

impl Sakura {
    pub fn new() -> Self {
        Self {
            energy: EnergyTracker::new(0.4, 0.88, 500.0),
            braille: BrailleBuffer::new(1, 1),
            petals: Vec::new(),
            frame: 0,
            rng: 1,
            last_w: 0,
            last_h: 0,
        }
    }

    fn rand(&mut self) -> u32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x.max(1);
        x
    }

    fn rand_f32(&mut self) -> f32 {
        (self.rand() % 10000) as f32 / 10000.0
    }

    fn spawn_petal(&mut self, bw: u16) {
        let x = (self.rand() % bw as u32) as f32;
        let vy = 0.3 + self.rand_f32() * 0.5;
        let vx = (self.rand_f32() - 0.5) * 0.3;
        let size = (self.rand() % 3) as u8;
        let phase = self.rand_f32() * std::f32::consts::TAU;
        self.petals.push(Petal {
            x,
            y: 0.0,
            vx,
            vy,
            size,
            phase,
        });
    }

    fn stamp_petal(braille: &mut BrailleBuffer, x: f32, y: f32, size: u8) {
        let px = x.round() as i32;
        let py = y.round() as i32;
        let bw = braille.width() as i32;
        let bh = braille.height() as i32;
        if px < 0 || py < 0 || px >= bw || py >= bh {
            return;
        }
        braille.set(px as u16, py as u16, true);
        match size {
            1 => {
                if px + 1 < bw {
                    braille.set((px + 1) as u16, py as u16, true);
                }
            }
            2 => {
                if px + 1 < bw {
                    braille.set((px + 1) as u16, py as u16, true);
                }
                if py + 1 < bh {
                    braille.set(px as u16, (py + 1) as u16, true);
                }
            }
            _ => {}
        }
    }
}

impl Default for Sakura {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Sakura {
    fn id(&self) -> VisualiserId {
        VisualiserId::Sakura
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

        let bw = self.braille.width();
        let bh = self.braille.height();

        // Spawn new petals based on audio energy.
        let spawn_count = (energy * 3.0) as usize;
        for _ in 0..spawn_count {
            if self.petals.len() >= MAX_PETALS {
                break;
            }
            self.spawn_petal(bw);
        }

        // Update petal positions.
        let frame_f = self.frame as f32;
        let bw_f = bw as f32;
        let bh_f = bh as f32;
        self.petals.retain_mut(|p| {
            p.y += p.vy;
            p.x += p.vx + (frame_f * 0.02 + p.phase).sin() * 0.5;
            p.y < bh_f && p.x >= -1.0 && p.x < bw_f + 1.0
        });

        // Render petals into braille buffer.
        for p in &self.petals {
            Self::stamp_petal(&mut self.braille, p.x, p.y, p.size);
        }

        // Compose to cell grid with pink/white palette.
        let bg = Rgb::new(6, 2, 6);
        self.braille.compose(grid, |_cx, _cy, dot_count| {
            let t = dot_count as f32 / 4.0;
            let hue = 0.94;
            let sat = lerp(0.3, 0.7, t);
            let val = lerp(0.5, 1.0, t);
            let (r, g, b) = hsv_to_rgb(hue, sat, val);
            let fg = Rgb::new(f32_to_u8(r), f32_to_u8(g), f32_to_u8(b));
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
        let mut sakura = Sakura::new();
        let fft = loud_fft();
        let mut grid = CellGrid::new(40, 12);
        for _ in 0..10 {
            let mut ctx = TuiContext { grid: &mut grid };
            sakura.render_tui(&mut ctx, &fft);
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
        let mut sakura = Sakura::new();
        let fft = loud_fft();

        let mut grid_a = CellGrid::new(30, 10);
        for _ in 0..5 {
            let mut ctx = TuiContext { grid: &mut grid_a };
            sakura.render_tui(&mut ctx, &fft);
        }

        let mut grid_b = CellGrid::new(30, 10);
        {
            let mut ctx = TuiContext { grid: &mut grid_b };
            sakura.render_tui(&mut ctx, &fft);
        }

        let diff = grid_a
            .cells()
            .iter()
            .zip(grid_b.cells().iter())
            .filter(|(a, b)| a.ch != b.ch)
            .count();
        assert!(
            diff > 0,
            "consecutive frames should differ due to petal movement"
        );
    }

    #[test]
    fn resize_no_panic() {
        let mut sakura = Sakura::new();
        let fft = loud_fft();

        let mut grid = CellGrid::new(80, 24);
        let mut ctx = TuiContext { grid: &mut grid };
        sakura.render_tui(&mut ctx, &fft);

        let mut grid = CellGrid::new(40, 12);
        let mut ctx = TuiContext { grid: &mut grid };
        sakura.render_tui(&mut ctx, &fft);
    }
}
