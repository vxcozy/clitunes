//! Quit fade-to-black: 10-frame cinematic exit.
//!
//! When the user presses q, the screen fades to black before
//! terminal cleanup.

use crate::tui::transition::easing;
use crate::visualiser::cell_grid::{Cell, CellGrid, Rgb};

/// Quit fade state.
#[derive(Clone, Debug, Default)]
pub struct QuitFade {
    frame: u16,
    active: bool,
}

/// Duration of the quit fade in frames.
const QUIT_FRAMES: u16 = 10;

impl QuitFade {
    /// Start the quit fade.
    pub fn start(&mut self) {
        self.frame = 0;
        self.active = true;
    }

    /// Whether the fade is in progress.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Whether the fade has completed (screen is black).
    pub fn is_done(&self) -> bool {
        self.active && self.frame >= QUIT_FRAMES
    }

    /// Whether input should be blocked during the fade.
    pub fn is_input_blocked(&self) -> bool {
        self.active
    }

    /// Advance by one frame.
    pub fn tick(&mut self) {
        if self.active && self.frame < QUIT_FRAMES {
            self.frame += 1;
        }
    }

    /// Apply the fade to the grid, dimming toward black.
    pub fn apply(&self, grid: &mut CellGrid) {
        if !self.active {
            return;
        }
        let raw_t = self.frame as f32 / QUIT_FRAMES as f32;
        let t = easing::ease_in_cubic(raw_t.clamp(0.0, 1.0));
        let brightness = 1.0 - t;

        let w = grid.width();
        let h = grid.height();
        for y in 0..h {
            for x in 0..w {
                let idx = (y as usize) * (w as usize) + (x as usize);
                let cell = grid.cells()[idx];
                grid.set(
                    x,
                    y,
                    Cell {
                        ch: cell.ch,
                        fg: dim(cell.fg, brightness),
                        bg: dim(cell.bg, brightness),
                    },
                );
            }
        }
    }
}

fn dim(c: Rgb, factor: f32) -> Rgb {
    Rgb::new(
        (c.r as f32 * factor).round() as u8,
        (c.g as f32 * factor).round() as u8,
        (c.b as f32 * factor).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_fade_completes() {
        let mut q = QuitFade::default();
        q.start();
        assert!(q.is_active());
        assert!(!q.is_done());
        for _ in 0..QUIT_FRAMES {
            q.tick();
        }
        assert!(q.is_done());
    }

    #[test]
    fn quit_fade_blocks_input() {
        let mut q = QuitFade::default();
        q.start();
        assert!(q.is_input_blocked());
    }

    #[test]
    fn quit_fade_grid_goes_black() {
        let mut q = QuitFade::default();
        q.start();
        let mut grid = CellGrid::new(10, 5);
        grid.fill(Cell {
            ch: '*',
            fg: Rgb::new(200, 200, 200),
            bg: Rgb::new(50, 50, 50),
        });
        for _ in 0..QUIT_FRAMES {
            q.tick();
        }
        q.apply(&mut grid);
        // At t=1.0 (ease_in_cubic(1.0)=1.0), brightness=0.
        assert_eq!(grid.cells()[0].fg, Rgb::new(0, 0, 0));
        assert_eq!(grid.cells()[0].bg, Rgb::new(0, 0, 0));
    }
}
