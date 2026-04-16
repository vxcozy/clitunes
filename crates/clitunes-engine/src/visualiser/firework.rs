use crate::audio::FftSnapshot;
use crate::visualiser::braille::BrailleBuffer;
use crate::visualiser::cell_grid::CellGrid;
use crate::visualiser::energy::EnergyTracker;
use crate::visualiser::palette::{f32_to_u8, hsv_to_rgb, lerp};
use crate::visualiser::{Rgb, SurfaceKind, TuiContext, Visualiser, VisualiserId};

const MAX_FIREWORKS: usize = 5;
const PARTICLES_PER_BURST: usize = 40;
const GRAVITY: f32 = 0.15;
const MAX_AGE: u16 = 60;
const COOLDOWN_FRAMES: u16 = 8;
const TRANSIENT_THRESHOLD: f32 = 0.12;

struct Particle {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
}

enum Phase {
    Rising {
        x: f32,
        y: f32,
        vy: f32,
        apex_y: f32,
    },
    Exploding {
        particles: Vec<Particle>,
        age: u16,
    },
}

struct FireworkState {
    phase: Phase,
    hue: f32,
}

pub struct Firework {
    energy: EnergyTracker,
    braille: BrailleBuffer,
    fireworks: Vec<FireworkState>,
    prev_energy: f32,
    cooldown: u16,
    rng: u32,
    last_w: u16,
    last_h: u16,
}

impl Firework {
    pub fn new() -> Self {
        Self {
            energy: EnergyTracker::new(0.3, 0.85, 500.0),
            braille: BrailleBuffer::new(1, 1),
            fireworks: Vec::new(),
            prev_energy: 0.0,
            cooldown: 0,
            rng: 0xDEAD_BEEF,
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
        (self.rand() % 10_000) as f32 / 10_000.0
    }
}

impl Default for Firework {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualiser for Firework {
    fn id(&self) -> VisualiserId {
        VisualiserId::Firework
    }

    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Tui
    }

    fn render_tui(&mut self, ctx: &mut TuiContext<'_>, fft: &FftSnapshot) {
        let e = self.energy.update(fft);

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

        let bw = self.braille.width() as f32;
        let bh = self.braille.height() as f32;

        // Transient detection: launch firework on energy spike.
        let delta = e - self.prev_energy;
        if delta > TRANSIENT_THRESHOLD
            && self.cooldown == 0
            && self.fireworks.len() < MAX_FIREWORKS
        {
            let x = self.rand_f32() * bw;
            let apex_y = bh * (0.2 + self.rand_f32() * 0.4); // 20-60% from top
            let hue = self.rand_f32();
            self.fireworks.push(FireworkState {
                phase: Phase::Rising {
                    x,
                    y: bh - 1.0,
                    vy: -2.0,
                    apex_y,
                },
                hue,
            });
            self.cooldown = COOLDOWN_FRAMES;
        }

        if self.cooldown > 0 {
            self.cooldown -= 1;
        }
        self.prev_energy = e;

        // Update firework states.
        for fw in &mut self.fireworks {
            match &mut fw.phase {
                Phase::Rising { x, y, vy, apex_y } => {
                    *y += *vy;
                    if *y <= *apex_y {
                        let cx = *x;
                        let cy = *y;
                        let mut particles = Vec::with_capacity(PARTICLES_PER_BURST);
                        // We need a local rng copy to avoid borrow issues.
                        let mut rng = fw.hue.to_bits() ^ 0xBAAD_CAFE;
                        for _ in 0..PARTICLES_PER_BURST {
                            // Inline xorshift for particle generation.
                            rng ^= rng << 13;
                            rng ^= rng >> 17;
                            rng ^= rng << 5;
                            rng = rng.max(1);
                            let angle =
                                (rng % 10_000) as f32 / 10_000.0 * std::f32::consts::TAU;
                            rng ^= rng << 13;
                            rng ^= rng >> 17;
                            rng ^= rng << 5;
                            rng = rng.max(1);
                            let speed = 0.5 + (rng % 10_000) as f32 / 10_000.0 * 1.5;
                            particles.push(Particle {
                                x: cx,
                                y: cy,
                                vx: angle.cos() * speed,
                                vy: angle.sin() * speed,
                            });
                        }
                        fw.phase = Phase::Exploding { particles, age: 0 };
                    }
                }
                Phase::Exploding { particles, age } => {
                    for p in particles.iter_mut() {
                        p.x += p.vx;
                        p.y += p.vy;
                        p.vy += GRAVITY;
                    }
                    *age += 1;
                }
            }
        }

        // Remove expired fireworks.
        self.fireworks.retain(|fw| match &fw.phase {
            Phase::Rising { .. } => true,
            Phase::Exploding { age, .. } => *age <= MAX_AGE,
        });

        // Render into braille buffer.
        let buf_w = self.braille.width();
        let buf_h = self.braille.height();
        for fw in &self.fireworks {
            match &fw.phase {
                Phase::Rising { x, y, .. } => {
                    let px = *x as u16;
                    let py = *y as u16;
                    if px < buf_w && py < buf_h {
                        self.braille.set(px, py, true);
                    }
                }
                Phase::Exploding { particles, .. } => {
                    for p in particles {
                        if p.x < 0.0 || p.y < 0.0 {
                            continue;
                        }
                        let px = p.x as u16;
                        let py = p.y as u16;
                        if px < buf_w && py < buf_h {
                            self.braille.set(px, py, true);
                        }
                    }
                }
            }
        }

        // Compose into cell grid.
        let bg = Rgb::new(2, 2, 4);
        self.braille.compose(grid, |_cx, _cy, dot_count| {
            let val = lerp(0.4, 1.0, dot_count as f32 / 6.0);
            let (r, g, b) = hsv_to_rgb(0.08, 0.8, val);
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

    fn silent_fft() -> FftSnapshot {
        FftSnapshot::new(vec![0.0; 128], 48_000, 256)
    }

    #[test]
    fn render_paints_cells() {
        let mut fw = Firework::new();
        let fft = loud_fft();
        let mut grid = CellGrid::new(60, 20);
        for _ in 0..20 {
            let mut ctx = TuiContext { grid: &mut grid };
            fw.render_tui(&mut ctx, &fft);
        }
        let braille_count = grid
            .cells()
            .iter()
            .filter(|c| c.ch != '\u{2800}' && c.ch != ' ')
            .count();
        assert!(
            braille_count > 0,
            "loud FFT over many frames should produce non-blank braille cells"
        );
    }

    #[test]
    fn output_changes_between_frames() {
        let mut fw = Firework::new();
        let fft = loud_fft();

        // Warm up to get a firework launched.
        let mut grid = CellGrid::new(40, 12);
        for _ in 0..10 {
            let mut ctx = TuiContext { grid: &mut grid };
            fw.render_tui(&mut ctx, &fft);
        }

        let mut grid_a = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid_a };
            fw.render_tui(&mut ctx, &fft);
        }

        let mut grid_b = CellGrid::new(40, 12);
        {
            let mut ctx = TuiContext { grid: &mut grid_b };
            fw.render_tui(&mut ctx, &fft);
        }

        let diff = grid_a
            .cells()
            .iter()
            .zip(grid_b.cells().iter())
            .filter(|(a, b)| a.ch != b.ch)
            .count();
        assert!(
            diff > 0,
            "consecutive frames should differ as particles move"
        );
    }

    #[test]
    fn resize_no_panic() {
        let mut fw = Firework::new();
        let fft = loud_fft();

        let mut grid = CellGrid::new(80, 24);
        let mut ctx = TuiContext { grid: &mut grid };
        fw.render_tui(&mut ctx, &fft);

        let mut grid = CellGrid::new(40, 12);
        let mut ctx = TuiContext { grid: &mut grid };
        fw.render_tui(&mut ctx, &fft);
    }

    #[test]
    fn transient_spawns_firework() {
        let mut fw = Firework::new();
        let silent = silent_fft();
        let loud = loud_fft();

        // Feed silence so energy stays low.
        let mut grid = CellGrid::new(60, 20);
        for _ in 0..10 {
            let mut ctx = TuiContext { grid: &mut grid };
            fw.render_tui(&mut ctx, &silent);
        }
        assert!(fw.fireworks.is_empty(), "no fireworks during silence");

        // Hit with a loud frame to trigger transient.
        {
            let mut ctx = TuiContext { grid: &mut grid };
            fw.render_tui(&mut ctx, &loud);
        }
        assert!(
            !fw.fireworks.is_empty(),
            "loud transient should spawn a firework"
        );
    }
}
