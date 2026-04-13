//! CellGrid-rendered status / transport bar.
//!
//! Layout (left to right):
//! ```text
//! ▸ PLAYING  lofi hip hop radio  ━━━━━━━━━━━━━━━━━●━━━  3:42  ◼ vol 80%
//! ```

use crate::tui::text::{truncate_str, write_str};
use crate::tui::theme::{Theme, Token};
use crate::visualiser::cell_grid::{Cell, CellGrid};

/// Playback state for the transport bar.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum PlayState {
    Playing,
    Paused,
    #[default]
    Stopped,
}

/// Current status line data. Fed by daemon events.
#[derive(Clone, Debug, Default)]
pub struct StatusLineState {
    pub play_state: PlayState,
    pub station_or_source: String,
    pub progress: f32,
    pub position_secs: u32,
    pub volume_pct: u8,
}

/// Render the status line into a single row of the grid at `y`.
pub fn render_status_line(
    grid: &mut CellGrid,
    y: u16,
    x0: u16,
    x1: u16,
    state: &StatusLineState,
    theme: &Theme,
) {
    let w = x1.saturating_sub(x0) as usize;
    if w < 10 || y >= grid.height() {
        return;
    }

    let bg = theme.get(Token::Background);
    let accent = theme.get(Token::Accent);
    let fg = theme.get(Token::Foreground);
    let dim = theme.get(Token::ForegroundDim);
    let muted = theme.get(Token::Muted);

    // Clear the row.
    for x in x0..x1.min(grid.width()) {
        grid.set(
            x,
            y,
            Cell {
                ch: ' ',
                fg: bg,
                bg,
            },
        );
    }

    let mut cursor = x0;

    // Play state icon.
    let (icon, icon_color) = match state.play_state {
        PlayState::Playing => ("▸", accent),
        PlayState::Paused => ("❚❚", dim),
        PlayState::Stopped => ("◼", muted),
    };
    cursor = write_str(grid, cursor, y, icon, icon_color, bg);
    cursor = write_str(grid, cursor, y, " ", fg, bg);

    // Station / source name.
    let vol_str = format!(" ◼ {}%", state.volume_pct);
    let time_str = format_time(state.position_secs);
    // Reserve space for: progress bar (min 10) + time + volume
    let reserved = 10 + time_str.len() + vol_str.len() + 4;
    let name_budget = w.saturating_sub(cursor.saturating_sub(x0) as usize + reserved);
    let name = truncate_str(&state.station_or_source, name_budget);
    cursor = write_str(grid, cursor, y, &name, fg, bg);
    cursor = write_str(grid, cursor, y, "  ", fg, bg);

    // Progress bar.
    let bar_end = x1.saturating_sub((time_str.len() + vol_str.len() + 3) as u16);
    let bar_width = bar_end.saturating_sub(cursor) as usize;
    if bar_width >= 3 {
        let filled = ((state.progress * bar_width as f32).round() as usize).min(bar_width);
        let thumb = filled.min(bar_width.saturating_sub(1));

        for i in 0..bar_width {
            let x = cursor + i as u16;
            if x >= grid.width() {
                break;
            }
            let (ch, color) = if i == thumb {
                ('●', accent)
            } else if i < filled {
                ('━', accent)
            } else {
                ('━', muted)
            };
            grid.set(x, y, Cell { ch, fg: color, bg });
        }
        cursor += bar_width as u16;
    }

    // Time.
    cursor = write_str(grid, cursor, y, "  ", fg, bg);
    cursor = write_str(grid, cursor, y, &time_str, dim, bg);

    // Volume.
    let _ = write_str(grid, cursor, y, &vol_str, dim, bg);
}

fn format_time(secs: u32) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{m}:{s:02}")
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::theme::Theme;

    #[test]
    fn status_line_layout_at_80_cols() {
        let mut grid = CellGrid::new(80, 1);
        let theme = Theme::default();
        let state = StatusLineState {
            play_state: PlayState::Playing,
            station_or_source: "lofi hip hop radio".into(),
            progress: 0.5,
            position_secs: 222,
            volume_pct: 80,
        };
        render_status_line(&mut grid, 0, 0, 80, &state, &theme);

        // First char should be the play icon.
        assert_eq!(grid.cells()[0].ch, '▸');
        assert_eq!(grid.cells()[0].fg, theme.get(Token::Accent));
    }

    #[test]
    fn status_line_truncation() {
        let mut grid = CellGrid::new(40, 1);
        let theme = Theme::default();
        let state = StatusLineState {
            play_state: PlayState::Playing,
            station_or_source: "A Very Long Station Name That Exceeds Budget".into(),
            progress: 0.3,
            position_secs: 60,
            volume_pct: 50,
        };
        render_status_line(&mut grid, 0, 0, 40, &state, &theme);

        // Should not panic, and grid should be fully painted.
        let has_ellipsis = grid.cells().iter().any(|c| c.ch == '…');
        assert!(has_ellipsis, "long name should be truncated with ellipsis");
    }

    #[test]
    fn status_line_states() {
        let mut grid = CellGrid::new(80, 1);
        let theme = Theme::default();

        // Playing → accent icon.
        let state = StatusLineState {
            play_state: PlayState::Playing,
            ..Default::default()
        };
        render_status_line(&mut grid, 0, 0, 80, &state, &theme);
        assert_eq!(grid.cells()[0].fg, theme.get(Token::Accent));

        // Paused → dim icon.
        let state = StatusLineState {
            play_state: PlayState::Paused,
            ..Default::default()
        };
        render_status_line(&mut grid, 0, 0, 80, &state, &theme);
        assert_eq!(grid.cells()[0].fg, theme.get(Token::ForegroundDim));

        // Stopped → muted icon.
        let state = StatusLineState {
            play_state: PlayState::Stopped,
            ..Default::default()
        };
        render_status_line(&mut grid, 0, 0, 80, &state, &theme);
        assert_eq!(grid.cells()[0].fg, theme.get(Token::Muted));
    }

    #[test]
    fn progress_bar_math() {
        let mut grid = CellGrid::new(80, 1);
        let theme = Theme::default();
        let state = StatusLineState {
            play_state: PlayState::Playing,
            station_or_source: "test".into(),
            progress: 0.5,
            position_secs: 120,
            volume_pct: 75,
        };
        render_status_line(&mut grid, 0, 0, 80, &state, &theme);

        // Count accent-colored '━' cells (progress bar filled portion).
        let accent = theme.get(Token::Accent);
        let filled = grid
            .cells()
            .iter()
            .filter(|c| c.ch == '━' && c.fg == accent)
            .count();
        let muted_color = theme.get(Token::Muted);
        let unfilled = grid
            .cells()
            .iter()
            .filter(|c| c.ch == '━' && c.fg == muted_color)
            .count();

        // At 50%, filled and unfilled should be roughly equal.
        let total = filled + unfilled;
        if total > 0 {
            let ratio = filled as f32 / total as f32;
            assert!(
                (ratio - 0.5).abs() < 0.2,
                "at 50% progress, ratio should be ~0.5, got {ratio}"
            );
        }
    }

    #[test]
    fn format_time_works() {
        assert_eq!(format_time(0), "0:00");
        assert_eq!(format_time(65), "1:05");
        assert_eq!(format_time(3661), "61:01");
    }
}
