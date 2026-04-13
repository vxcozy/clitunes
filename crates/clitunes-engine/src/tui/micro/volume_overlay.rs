//! Volume feedback overlay: horizontal bar at bottom-right.
//!
//! Appears on volume change, auto-hides after 1.5 seconds with
//! a 6-frame fade-out.

use crate::tui::theme::{Theme, Token};
use crate::visualiser::cell_grid::{Cell, CellGrid, Rgb};

/// Display width of the volume bar in cells. 20 cells covers 5%
/// increments visually, readable without dominating the UI.
const BAR_WIDTH: u16 = 20;
/// Frames to hold at full opacity before fading (45 frames = 1.5 s
/// at 30 fps — long enough to read, short enough to not linger).
const DISPLAY_FRAMES: u16 = 45;
/// Fade-out duration in frames (6 frames = 200 ms — fast enough to
/// feel responsive, slow enough to not pop).
const FADE_FRAMES: u16 = 6;

/// Volume overlay state.
#[derive(Clone, Debug, Default)]
pub struct VolumeOverlay {
    level: u8,
    frames_remaining: u16,
    fade_frame: u16,
}

impl VolumeOverlay {
    /// Show the volume overlay at the given level (0–100).
    pub fn show(&mut self, level: u8) {
        self.level = level.min(100);
        self.frames_remaining = DISPLAY_FRAMES + FADE_FRAMES;
        self.fade_frame = 0;
    }

    /// Whether the overlay is visible.
    pub fn is_visible(&self) -> bool {
        self.frames_remaining > 0
    }

    /// Advance by one frame.
    pub fn tick(&mut self) {
        if self.frames_remaining == 0 {
            return;
        }
        self.frames_remaining -= 1;
        if self.frames_remaining < FADE_FRAMES {
            self.fade_frame += 1;
        }
    }

    /// Current opacity (1.0 = fully visible, fading to 0.0).
    pub fn opacity(&self) -> f32 {
        if self.frames_remaining >= FADE_FRAMES {
            1.0
        } else if self.frames_remaining == 0 {
            0.0
        } else {
            self.frames_remaining as f32 / FADE_FRAMES as f32
        }
    }

    /// Render the volume overlay into the grid at the bottom-right.
    pub fn render(&self, grid: &mut CellGrid, theme: &Theme) {
        if !self.is_visible() {
            return;
        }

        let w = grid.width();
        let h = grid.height();
        if w < BAR_WIDTH + 10 || h < 2 {
            return;
        }

        let opacity = self.opacity();
        let accent = theme.get(Token::Accent);
        let dim = theme.get(Token::Muted);
        let bg = theme.get(Token::Background);
        let fg = theme.get(Token::Foreground);

        // Position: 2 cells from right edge, 1 row from bottom.
        let bar_x0 = w - BAR_WIDTH - 8;
        let y = h - 2;

        // "◼ " prefix.
        let icon_fg = blend_with_opacity(fg, bg, opacity);
        set_cell(grid, bar_x0, y, '◼', icon_fg, bg);
        set_cell(grid, bar_x0 + 1, y, ' ', bg, bg);

        // Bar.
        let filled = (self.level as u16 * BAR_WIDTH / 100).min(BAR_WIDTH);
        for i in 0..BAR_WIDTH {
            let x = bar_x0 + 2 + i;
            let (ch_fg, ch) = if i < filled {
                (blend_with_opacity(accent, bg, opacity), '━')
            } else {
                (blend_with_opacity(dim, bg, opacity), '━')
            };
            set_cell(grid, x, y, ch, ch_fg, bg);
        }

        // " NN%" suffix.
        let pct = format!(" {}%", self.level);
        let pct_fg = blend_with_opacity(fg, bg, opacity);
        let mut x = bar_x0 + 2 + BAR_WIDTH;
        for ch in pct.chars() {
            if x < w {
                set_cell(grid, x, y, ch, pct_fg, bg);
                x += 1;
            }
        }
    }
}

impl super::Overlay for VolumeOverlay {
    fn tick(&mut self) {
        VolumeOverlay::tick(self);
    }

    fn is_active(&self) -> bool {
        self.is_visible()
    }

    fn apply(&mut self, grid: &mut CellGrid, theme: &Theme) {
        self.render(grid, theme);
    }
}

fn blend_with_opacity(fg: Rgb, bg: Rgb, opacity: f32) -> Rgb {
    Rgb::new(
        ((fg.r as f32 * opacity + bg.r as f32 * (1.0 - opacity)).round()) as u8,
        ((fg.g as f32 * opacity + bg.g as f32 * (1.0 - opacity)).round()) as u8,
        ((fg.b as f32 * opacity + bg.b as f32 * (1.0 - opacity)).round()) as u8,
    )
}

fn set_cell(grid: &mut CellGrid, x: u16, y: u16, ch: char, fg: Rgb, bg: Rgb) {
    if x < grid.width() && y < grid.height() {
        grid.set(x, y, Cell { ch, fg, bg });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_hide_lifecycle() {
        let mut v = VolumeOverlay::default();
        v.show(80);
        assert!(v.is_visible());
        for _ in 0..(DISPLAY_FRAMES + FADE_FRAMES) {
            v.tick();
        }
        assert!(!v.is_visible());
    }

    #[test]
    fn fade_starts_after_display_frames() {
        let mut v = VolumeOverlay::default();
        v.show(50);
        // After DISPLAY_FRAMES ticks, we've consumed the hold period
        // but haven't entered the fade — opacity should still be 1.0.
        for _ in 0..DISPLAY_FRAMES {
            v.tick();
        }
        assert!(
            (v.opacity() - 1.0).abs() < f32::EPSILON,
            "opacity should be exactly 1.0 before fade, got {}",
            v.opacity()
        );
        for _ in 0..FADE_FRAMES {
            v.tick();
        }
        assert_eq!(v.opacity(), 0.0, "should be fully faded after all frames");
    }

    #[test]
    fn volume_0_and_100() {
        let mut v = VolumeOverlay::default();
        v.show(0);
        assert_eq!(v.level, 0);
        v.show(100);
        assert_eq!(v.level, 100);
        // No panic.
    }

    #[test]
    fn volume_level_rendering() {
        let mut v = VolumeOverlay::default();
        v.show(50);
        let theme = Theme::default();
        let mut grid = CellGrid::new(60, 10);
        v.render(&mut grid, &theme);
        // Should have painted something in the bottom-right region.
        let y = 8; // h - 2
        let has_bar = (0..60).any(|x| {
            let idx = y * 60 + x;
            grid.cells()[idx].ch == '━'
        });
        assert!(has_bar, "volume bar should be rendered");
    }
}
