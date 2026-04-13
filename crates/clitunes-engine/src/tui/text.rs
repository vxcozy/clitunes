//! Shared text helpers for TUI rendering into `CellGrid`.

use crate::visualiser::cell_grid::{Cell, CellGrid, Rgb};

/// Write `text` char-by-char into `grid` starting at `(x0, y)`.
/// Returns the cursor position after the last written character so
/// callers can chain writes on the same row.
pub fn write_str(grid: &mut CellGrid, x0: u16, y: u16, text: &str, fg: Rgb, bg: Rgb) -> u16 {
    if y >= grid.height() {
        return x0;
    }
    let mut x = x0;
    for ch in text.chars() {
        if x >= grid.width() {
            break;
        }
        grid.set(x, y, Cell { ch, fg, bg });
        x = x.saturating_add(1);
    }
    x
}

/// Set a single glyph at `(x, y)`, silently clamping at grid boundaries.
pub fn set_glyph(grid: &mut CellGrid, x: u16, y: u16, ch: char, fg: Rgb, bg: Rgb) {
    if x < grid.width() && y < grid.height() {
        grid.set(x, y, Cell { ch, fg, bg });
    }
}

/// Truncate `s` to at most `max` display characters, appending `…` if
/// truncated. Returns an empty string when `max < 2`.
pub fn truncate_str(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else if max > 1 {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        out
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_str_returns_cursor_position() {
        let mut grid = CellGrid::new(20, 1);
        let end = write_str(
            &mut grid,
            2,
            0,
            "hello",
            Rgb::new(255, 255, 255),
            Rgb::BLACK,
        );
        assert_eq!(end, 7);
        assert_eq!(grid.cells()[2].ch, 'h');
        assert_eq!(grid.cells()[6].ch, 'o');
    }

    #[test]
    fn write_str_clips_at_width() {
        let mut grid = CellGrid::new(5, 1);
        let end = write_str(
            &mut grid,
            3,
            0,
            "abcde",
            Rgb::new(255, 255, 255),
            Rgb::BLACK,
        );
        assert_eq!(end, 5);
        assert_eq!(grid.cells()[3].ch, 'a');
        assert_eq!(grid.cells()[4].ch, 'b');
    }

    #[test]
    fn write_str_noop_on_out_of_bounds_y() {
        let mut grid = CellGrid::new(10, 2);
        let end = write_str(&mut grid, 0, 5, "test", Rgb::new(255, 255, 255), Rgb::BLACK);
        assert_eq!(end, 0);
    }

    #[test]
    fn truncate_str_short_passthrough() {
        assert_eq!(truncate_str("ab", 5), "ab");
    }

    #[test]
    fn truncate_str_exact_fit() {
        assert_eq!(truncate_str("abcde", 5), "abcde");
    }

    #[test]
    fn truncate_str_adds_ellipsis() {
        let out = truncate_str("abcdefgh", 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.ends_with('…'));
        assert_eq!(out, "abcd…");
    }

    #[test]
    fn truncate_str_empty_on_tiny_budget() {
        assert_eq!(truncate_str("hello", 1), "");
        assert_eq!(truncate_str("hello", 0), "");
    }
}
