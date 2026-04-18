/// Falling rain using box-drawing characters with per-column drops driven by audio energy.
use crate::audio::FftSnapshot;
use crate::visualiser::cell_grid::{Cell, CellGrid};
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

const MAX_DROPS_PER_COL: usize = 4;
const BASE_VELOCITY: f32 = 0.5;
const VELOCITY_RANGE: f32 = 1.0;

const HEAD_COLOR: Rgb = Rgb::new(180, 210, 255);
const BODY_COLOR: Rgb = Rgb::new(60, 100, 180);
const TAIL_COLOR: Rgb = Rgb::new(30, 50, 100);
const BG_COLOR: Rgb = Rgb::new(2, 2, 10);

struct Drop {
    y: f32,
    velocity: f32,
    length: u16,
}

pub struct Rain {
    energy: EnergyTracker,
    columns: Vec<Vec<Drop>>,
    last_w: u16,
    last_h: u16,
    rng: u32,
}

impl Rain {
    pub fn new() -> Self {
        Self {
            // Release tau ~115 ms: spawn-probability tracks the beat
            // rather than averaging the last quarter-second of audio.
            energy: EnergyTracker::new(0.5, 0.75, 500.0),
            columns: Vec::new(),
            last_w: 0,
            last_h: 0,
            rng: 0xDEAD_BEEF,
        }
    }

    /// xorshift32 — cheap PRNG.
    fn rand(&mut self) -> u32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x.max(1);
        x
    }

    fn ensure_columns(&mut self, w: u16, h: u16) {
        if w != self.last_w || h != self.last_h {
            self.columns.clear();
            self.columns.resize_with(w as usize, Vec::new);
            self.last_w = w;
            self.last_h = h;
        }
    }

    fn band_energy_for_col(&self, col: u16, w: u16, fft: &FftSnapshot) -> f32 {
        let n = fft.magnitudes.len();
        if n == 0 || w == 0 {
            return 0.0;
        }
        let start = (col as usize) * n / (w as usize);
        let end = ((col as usize) + 1) * n / (w as usize);
        let end = end.max(start + 1).min(n);
        let sum: f32 = fft.magnitudes[start..end].iter().sum();
        let avg = sum / (end - start) as f32;
        (avg / 500.0).min(1.0)
    }
}

impl Default for Rain {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Rain {
    fn id(&self) -> VisualiserId {
        VisualiserId::Rain
    }

    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Tui
    }

    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot) {
        self.energy.update(fft);

        let grid: &mut CellGrid = &mut *ctx.grid;
        let w = grid.width();
        let h = grid.height();
        if w == 0 || h == 0 {
            return;
        }

        let blank = Cell {
            ch: ' ',
            fg: Rgb::BLACK,
            bg: BG_COLOR,
        };
        grid.fill(blank);

        self.ensure_columns(w, h);

        // Spawn new drops.
        for x in 0..w as usize {
            let band = self.band_energy_for_col(x as u16, w, fft);
            let probability = band * 0.3;
            let threshold = (probability * 1000.0) as u32;
            let r = self.rand() % 1000;
            if r < threshold && self.columns[x].len() < MAX_DROPS_PER_COL {
                let vel = BASE_VELOCITY + (self.rand() % 1000) as f32 / 1000.0 * VELOCITY_RANGE;
                let length = 3 + (self.rand() % 6) as u16; // 3..8
                self.columns[x].push(Drop {
                    y: -(length as f32),
                    velocity: vel,
                    length,
                });
            }
        }

        // Update drops and remove off-screen ones.
        for col in &mut self.columns {
            for drop in col.iter_mut() {
                drop.y += drop.velocity;
            }
            col.retain(|d| d.y <= h as f32 + d.length as f32);
        }

        // Render drops.
        for x in 0..w as usize {
            for drop in &self.columns[x] {
                let head_y = drop.y as i32;
                let tail_start = head_y - drop.length as i32 + 1;

                for row in tail_start..=head_y {
                    if row < 0 || row >= h as i32 {
                        continue;
                    }
                    let (ch, fg) = if row == head_y {
                        ('\u{2503}', HEAD_COLOR) // ┃
                    } else if row == tail_start {
                        (':', TAIL_COLOR)
                    } else {
                        ('\u{2502}', BODY_COLOR) // │
                    };
                    grid.set(
                        x as u16,
                        row as u16,
                        Cell {
                            ch,
                            fg,
                            bg: BG_COLOR,
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
        FftSnapshot::new(vec![5000.0; 64], 48_000, 128)
    }

    #[test]
    fn render_paints_cells() {
        let mut rain = Rain::new();
        let fft = loud_fft();
        let mut grid = CellGrid::new(60, 20);
        // Run several frames so drops have time to spawn and advance.
        for _ in 0..30 {
            let mut ctx = TuiContext { grid: &mut grid };
            rain.render_tui(&mut ctx, &fft);
        }
        let painted = grid.cells().iter().filter(|c| c.ch != ' ').count();
        assert!(
            painted > 0,
            "expected some non-space cells after several loud frames"
        );
    }

    #[test]
    fn output_changes_between_frames() {
        let mut rain = Rain::new();
        let fft = loud_fft();

        let mut grid = CellGrid::new(60, 20);
        // Warm up so drops exist.
        for _ in 0..20 {
            let mut ctx = TuiContext { grid: &mut grid };
            rain.render_tui(&mut ctx, &fft);
        }
        let snap1 = grid.snapshot();

        {
            let mut ctx = TuiContext { grid: &mut grid };
            rain.render_tui(&mut ctx, &fft);
        }
        let snap2 = grid.snapshot();

        let differs = snap1
            .cells()
            .iter()
            .zip(snap2.cells().iter())
            .any(|(a, b)| a.ch != b.ch || a.fg != b.fg);
        assert!(differs, "two consecutive frames should differ");
    }

    #[test]
    fn resize_no_panic() {
        let mut rain = Rain::new();
        let fft = loud_fft();
        let mut grid = CellGrid::new(80, 24);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            rain.render_tui(&mut ctx, &fft);
        }
        grid.resize(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid };
            rain.render_tui(&mut ctx, &fft);
        }
    }
}
