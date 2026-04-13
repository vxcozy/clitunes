//! Reusable panel drawing primitive.
//!
//! A panel is a bordered rectangle filled with a background colour,
//! optionally topped by a centered header line. Used by the picker
//! modal, future dialogs, and any component that needs a contained box.

use crate::tui::picker::paint::Rect;
use crate::tui::theme::{Theme, Token};
use crate::visualiser::cell_grid::{Cell, CellGrid, Rgb};

/// Style configuration for a panel, expressed as theme tokens.
#[derive(Copy, Clone, Debug)]
pub struct PanelStyle {
    /// Border glyph foreground.
    pub border_fg: Token,
    /// Border and outer background.
    pub border_bg: Token,
    /// Interior fill background.
    pub fill_bg: Token,
    /// Use rounded corners (╭╮╰╯) when true, sharp (┌┐└┘) when false.
    pub corner_radius: bool,
}

impl Default for PanelStyle {
    fn default() -> Self {
        Self {
            border_fg: Token::Border,
            border_bg: Token::Background,
            fill_bg: Token::Background,
            corner_radius: true,
        }
    }
}

/// Draw a bordered panel onto the grid at the given rect.
pub fn draw_panel(grid: &mut CellGrid, rect: Rect, style: &PanelStyle, theme: &Theme) {
    let fg = theme.get(style.border_fg);
    let bg = theme.get(style.border_bg);
    let fill = theme.get(style.fill_bg);

    // Fill interior.
    fill_rect(grid, rect.x0, rect.y0, rect.x1, rect.y1, fill);

    // Border.
    draw_border(grid, rect, fg, bg, style.corner_radius);
}

/// Draw a panel with a centered header line on the top border row.
pub fn draw_panel_with_header(
    grid: &mut CellGrid,
    rect: Rect,
    header: &str,
    style: &PanelStyle,
    theme: &Theme,
) {
    draw_panel(grid, rect, style, theme);

    // Write header centered on the top border row (y0).
    let fg = theme.get(Token::ForegroundBright);
    let bg = theme.get(style.border_bg);
    let inner_w = rect.width().saturating_sub(4); // 2 border + 1 space each side
    if inner_w > 0 {
        let count = header.chars().count() as u16;
        let pad = inner_w.saturating_sub(count) / 2;
        let x = rect.x0 + 2 + pad;
        write_text(grid, x, rect.y0, header, fg, bg);
    }
}

fn fill_rect(grid: &mut CellGrid, x0: u16, y0: u16, x1: u16, y1: u16, bg: Rgb) {
    for y in y0..y1.min(grid.height()) {
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
    }
}

fn draw_border(grid: &mut CellGrid, rect: Rect, fg: Rgb, bg: Rgb, rounded: bool) {
    let w = grid.width();
    let h = grid.height();
    if rect.x0 >= w || rect.y0 >= h || rect.x1 <= rect.x0 || rect.y1 <= rect.y0 {
        return;
    }
    let x1i = rect.x1.saturating_sub(1);
    let y1i = rect.y1.saturating_sub(1);

    for x in (rect.x0 + 1)..x1i {
        set_glyph(grid, x, rect.y0, '─', fg, bg);
        set_glyph(grid, x, y1i, '─', fg, bg);
    }
    for y in (rect.y0 + 1)..y1i {
        set_glyph(grid, rect.x0, y, '│', fg, bg);
        set_glyph(grid, x1i, y, '│', fg, bg);
    }

    let (tl, tr, bl, br) = if rounded {
        ('╭', '╮', '╰', '╯')
    } else {
        ('┌', '┐', '└', '┘')
    };
    set_glyph(grid, rect.x0, rect.y0, tl, fg, bg);
    set_glyph(grid, x1i, rect.y0, tr, fg, bg);
    set_glyph(grid, rect.x0, y1i, bl, fg, bg);
    set_glyph(grid, x1i, y1i, br, fg, bg);
}

fn set_glyph(grid: &mut CellGrid, x: u16, y: u16, ch: char, fg: Rgb, bg: Rgb) {
    if x < grid.width() && y < grid.height() {
        grid.set(x, y, Cell { ch, fg, bg });
    }
}

fn write_text(grid: &mut CellGrid, x0: u16, y: u16, text: &str, fg: Rgb, bg: Rgb) {
    if y >= grid.height() {
        return;
    }
    let mut x = x0;
    for ch in text.chars() {
        if x >= grid.width() {
            break;
        }
        grid.set(x, y, Cell { ch, fg, bg });
        x = x.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::theme::Theme;

    #[test]
    fn draw_panel_rounded_corners() {
        let mut grid = CellGrid::new(20, 10);
        let theme = Theme::default();
        let style = PanelStyle::default();
        let rect = Rect {
            x0: 2,
            y0: 1,
            x1: 18,
            y1: 9,
        };
        draw_panel(&mut grid, rect, &style, &theme);

        let idx = |x: u16, y: u16| (y as usize) * 20 + (x as usize);
        assert_eq!(grid.cells()[idx(2, 1)].ch, '╭');
        assert_eq!(grid.cells()[idx(17, 1)].ch, '╮');
        assert_eq!(grid.cells()[idx(2, 8)].ch, '╰');
        assert_eq!(grid.cells()[idx(17, 8)].ch, '╯');
        assert_eq!(grid.cells()[idx(2, 1)].fg, theme.get(Token::Border));
    }

    #[test]
    fn draw_panel_sharp_corners() {
        let mut grid = CellGrid::new(20, 10);
        let theme = Theme::default();
        let style = PanelStyle {
            corner_radius: false,
            ..PanelStyle::default()
        };
        let rect = Rect {
            x0: 2,
            y0: 1,
            x1: 18,
            y1: 9,
        };
        draw_panel(&mut grid, rect, &style, &theme);

        let idx = |x: u16, y: u16| (y as usize) * 20 + (x as usize);
        assert_eq!(grid.cells()[idx(2, 1)].ch, '┌');
        assert_eq!(grid.cells()[idx(17, 1)].ch, '┐');
    }

    #[test]
    fn draw_panel_fills_interior() {
        let mut grid = CellGrid::new(20, 10);
        let theme = Theme::default();
        let style = PanelStyle {
            fill_bg: Token::Surface,
            ..PanelStyle::default()
        };
        let rect = Rect {
            x0: 2,
            y0: 1,
            x1: 18,
            y1: 9,
        };
        draw_panel(&mut grid, rect, &style, &theme);

        let fill_bg = theme.get(Token::Surface);
        // Check an interior cell.
        let idx = 3 * 20 + 5; // y=3, x=5
        assert_eq!(grid.cells()[idx].bg, fill_bg);
    }

    #[test]
    fn draw_panel_with_header_centered() {
        let mut grid = CellGrid::new(30, 10);
        let theme = Theme::default();
        let style = PanelStyle::default();
        let rect = Rect {
            x0: 0,
            y0: 0,
            x1: 30,
            y1: 10,
        };
        draw_panel_with_header(&mut grid, rect, "Test", &style, &theme);

        // Header "Test" is 4 chars, inner_w = 30 - 4 = 26, pad = (26-4)/2 = 11.
        // x = 0 + 2 + 11 = 13.
        let idx = 13; // y=0, x=13
        assert_eq!(grid.cells()[idx].ch, 'T');
        assert_eq!(grid.cells()[idx + 1].ch, 'e');
    }

    #[test]
    fn panel_style_uses_theme_tokens() {
        let theme = Theme::default();
        let style = PanelStyle {
            border_fg: Token::BorderFocus,
            fill_bg: Token::SurfaceBright,
            ..PanelStyle::default()
        };

        let mut grid = CellGrid::new(20, 10);
        let rect = Rect {
            x0: 0,
            y0: 0,
            x1: 20,
            y1: 10,
        };
        draw_panel(&mut grid, rect, &style, &theme);

        assert_eq!(grid.cells()[0].fg, theme.get(Token::BorderFocus));
        // Interior cell bg.
        let interior = 2 * 20 + 5;
        assert_eq!(grid.cells()[interior].bg, theme.get(Token::SurfaceBright));
    }
}
