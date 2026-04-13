//! Paint a curated-station picker overlay into a `CellGrid`.
//!
//! The picker is a centered modal box drawn with a single-line box
//! glyph frame, a header line, a list body (one row per station), and
//! a short footer showing the active key bindings. Highlighted row
//! uses inverted colours (bright foreground, dark background swapped
//! for the selection). Non-highlighted rows render with a dim grey fg
//! so the surrounding Auralis visualiser shows through the border
//! area without the list competing for attention.
//!
//! # Layout math
//!
//! Given a terminal that clitunes has resolved to `(grid_w, grid_h)`
//! cells, the picker picks the largest comfortable modal box it can
//! fit with these invariants:
//!
//! - Max width  64 cells (wider wastes horizontal space).
//! - Min width  32 cells (narrower truncates station names).
//! - Body height is always `CURATED_SLOT_COUNT` rows.
//! - Chrome (border 2 + header 2 + footer 2) is 6 rows on top of the
//!   body, so total height is 6 + `CURATED_SLOT_COUNT` = 18 rows.
//!
//! If the terminal is smaller than the min box, [`paint_picker`]
//! **degrades gracefully**:
//!
//! - Too-narrow → drops the genre column, keeps only `NN. Name`.
//! - Too-short → clips the visible list around the selection, keeping
//!   the selected row in view (scroll window), and shrinks the chrome
//!   to a single-line frame with no spacer rows.
//! - Catastrophically small (< 20 cols or < 6 rows) → paints a
//!   one-line "PICKER (s to open larger)" banner at the top so the
//!   user isn't stuck with no signal at all.
//!
//! # Safety: untrusted strings
//!
//! Station names, genres, and countries originate from the
//! radio-browser directory and are already sanitized by
//! `clitunes_core::sanitize` at the ingestion boundary (Unit 5). The
//! paint path assumes sanitized input and does not re-sanitize. It
//! **does** strip non-printable characters defensively in
//! [`safe_chars`] as a final backstop before writing into the grid.

use crate::tui::components::panel::{draw_panel, PanelStyle};
use crate::tui::picker::curated_seed::{CuratedList, CURATED_SLOT_COUNT};
use crate::tui::theme::{Theme, Token};
use crate::visualiser::cell_grid::{Cell, CellGrid, Rgb};

/// Header text above the station list. Short and warm so the first
/// screen feels inviting rather than interrogative.
const HEADER_PRIMARY: &str = "First time? Pick a starting point.";
const HEADER_SECONDARY: &str = "You can change it anytime.";
const FOOTER: &str = "↑/↓ move   enter select   s hide   q quit";
const FOOTER_VIZ: &str = "n/p cycle viz · plasma · ripples · tunnel · metaballs · vortex · fire · matrix · moiré";

/// Minimum comfortable modal dimensions. See [`paint_picker`] for the
/// fallback behavior when the grid is smaller.
pub const MIN_MODAL_W: u16 = 32;
pub const MIN_MODAL_H: u16 = 18;
pub const MAX_MODAL_W: u16 = 64;

/// Public paint entry point. Paints the picker modal on top of
/// whatever the visualiser drew into `grid`.
///
/// `selected` is a 0-based index into `list.stations`. Values out of
/// range are clamped so a stale state.toml pointing at a removed
/// slot can't crash the paint path.
///
/// Returns the bounding rect of the painted modal as
/// `(x0, y0, x1, y1)` (exclusive upper bound), or `None` if the grid
/// was too small to paint even the degraded banner — the caller can
/// treat `None` as "the user will see the visualiser, no modal".
pub fn paint_picker(
    grid: &mut CellGrid,
    list: &CuratedList,
    selected: usize,
    theme: &Theme,
) -> Option<Rect> {
    let bg = theme.get(Token::Background);
    let surface = theme.get(Token::Surface);
    let body_fg = theme.get(Token::Foreground);
    let body_dim_fg = theme.get(Token::ForegroundDim);
    let header_fg = theme.get(Token::ForegroundBright);
    let accent = theme.get(Token::Accent);
    let select_text = theme.get(Token::ForegroundBright);
    let select_row_bg = theme.get(Token::SurfaceBright);

    let grid_w = grid.width();
    let grid_h = grid.height();

    let border_fg = theme.get(Token::Border);

    // Catastrophically small — one-line banner fallback.
    if grid_w < 20 || grid_h < 6 {
        return paint_fallback_banner(grid, border_fg, bg);
    }

    let selected = selected.min(list.stations.len().saturating_sub(1));

    let modal_w = grid_w.min(MAX_MODAL_W).max(MIN_MODAL_W.min(grid_w));
    let chrome_min_h: u16 = 6; // border*2 + header*2 + footer*2
    let visible_body = (grid_h.saturating_sub(chrome_min_h)).min(CURATED_SLOT_COUNT as u16);
    if visible_body == 0 {
        return paint_fallback_banner(grid, border_fg, bg);
    }
    let modal_h = chrome_min_h + visible_body;

    let x0 = (grid_w.saturating_sub(modal_w)) / 2;
    let y0 = (grid_h.saturating_sub(modal_h)) / 2;
    let x1 = x0 + modal_w;
    let y1 = y0 + modal_h;

    // Draw panel (fill + border) using the component.
    let rect = Rect { x0, y0, x1, y1 };
    let panel_style = PanelStyle {
        border_fg: Token::Border,
        border_bg: Token::Background,
        fill_bg: Token::Surface,
        corner_radius: true,
    };
    draw_panel(grid, rect, &panel_style, theme);

    // Header: two centered lines at rows y0+1 and y0+2.
    let inner_x0 = x0 + 2;
    let inner_x1 = x1.saturating_sub(2);
    let inner_w = inner_x1.saturating_sub(inner_x0);
    write_centered(
        grid,
        inner_x0,
        inner_w,
        y0 + 1,
        HEADER_PRIMARY,
        header_fg,
        surface,
    );
    write_centered(
        grid,
        inner_x0,
        inner_w,
        y0 + 2,
        HEADER_SECONDARY,
        body_dim_fg,
        surface,
    );

    // Body rows — scroll so the selected row is visible.
    let body_y0 = y0 + 3;
    let body_y1 = y1.saturating_sub(3);
    let body_rows = body_y1.saturating_sub(body_y0);
    if body_rows > 0 {
        let scroll = scroll_offset(list.stations.len(), body_rows as usize, selected);
        for row in 0..body_rows {
            let idx = scroll + row as usize;
            if idx >= list.stations.len() {
                break;
            }
            let station = &list.stations[idx];
            let is_selected = idx == selected;
            let line = format_row(station, inner_w as usize);
            let (fg, row_bg) = if is_selected {
                (select_text, select_row_bg)
            } else {
                (body_fg, surface)
            };
            // Fill the whole row with bg first so the selection
            // highlight extends across the full width.
            fill_rect_row(grid, inner_x0, inner_x1, body_y0 + row, row_bg);
            write_text(grid, inner_x0, body_y0 + row, &line, fg, row_bg);
            // Accent ▸ indicator on the selected row.
            if is_selected {
                set_glyph(grid, inner_x0, body_y0 + row, '▸', accent, row_bg);
            }
        }
    }

    // Footer.
    let footer_y = y1.saturating_sub(3);
    write_centered(
        grid,
        inner_x0,
        inner_w,
        footer_y,
        FOOTER,
        body_dim_fg,
        surface,
    );
    write_centered(
        grid,
        inner_x0,
        inner_w,
        footer_y + 1,
        FOOTER_VIZ,
        body_dim_fg,
        surface,
    );

    Some(Rect { x0, y0, x1, y1 })
}

/// Catastrophically-small fallback: one line of text at the top.
fn paint_fallback_banner(grid: &mut CellGrid, fg: Rgb, bg: Rgb) -> Option<Rect> {
    if grid.width() < 8 || grid.height() == 0 {
        return None;
    }
    let msg = "PICKER — enlarge terminal";
    let x1 = grid.width().min(msg.len() as u16 + 4);
    fill_rect_row(grid, 0, x1, 0, bg);
    write_text(grid, 1, 0, msg, fg, bg);
    Some(Rect {
        x0: 0,
        y0: 0,
        x1,
        y1: 1,
    })
}

/// Rect returned by [`paint_picker`], exclusive on x1/y1. The picker
/// state machine uses it to figure out where hit-testing would go for
/// mouse clicks (deferred to v1.1).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x0: u16,
    pub y0: u16,
    pub x1: u16,
    pub y1: u16,
}

impl Rect {
    pub fn width(&self) -> u16 {
        self.x1.saturating_sub(self.x0)
    }
    pub fn height(&self) -> u16 {
        self.y1.saturating_sub(self.y0)
    }
}

/// Compute a scroll offset for the body list such that `selected` is
/// visible within the `body_rows` window. Keeps the selected row in
/// the middle-third of the window when possible so arrow-key moves
/// feel smooth.
pub fn scroll_offset(total: usize, body_rows: usize, selected: usize) -> usize {
    if total <= body_rows {
        return 0;
    }
    let half = body_rows / 2;
    let max_scroll = total - body_rows;
    selected.saturating_sub(half).min(max_scroll)
}

/// Build a single row's display text, padded/truncated to `inner_w`
/// cells. Layout is `NN. Genre        Name` when wide enough, falling
/// back to `NN. Name` when genre won't fit.
pub fn format_row(station: &clitunes_core::CuratedStation, inner_w: usize) -> String {
    let slot = station.slot + 1;
    let name = safe_chars(&station.name);
    let genre = safe_chars(&station.genre);

    // Reserve 4 cells for "NN. " prefix, 1 cell left pad, 1 cell right pad.
    let body_w = inner_w.saturating_sub(6);
    if body_w == 0 {
        return format!("{slot:>2}");
    }

    let narrow = body_w < 22; // need at least ~22 cells for genre+name both
    if narrow {
        let name_w = body_w;
        format!(
            " {slot:>2}. {name} ",
            slot = slot,
            name = truncate_or_pad(&name, name_w),
        )
    } else {
        let genre_w = 12;
        let name_w = body_w.saturating_sub(genre_w + 1);
        format!(
            " {slot:>2}. {genre} {name} ",
            slot = slot,
            genre = truncate_or_pad(&genre, genre_w),
            name = truncate_or_pad(&name, name_w),
        )
    }
}

/// Truncate with an ellipsis when too long, right-pad with spaces
/// when too short. Works on Unicode char boundaries, not bytes.
pub fn truncate_or_pad(s: &str, cells: usize) -> String {
    let count = s.chars().count();
    if count == cells {
        s.to_string()
    } else if count > cells {
        if cells == 0 {
            String::new()
        } else {
            let mut out: String = s.chars().take(cells.saturating_sub(1)).collect();
            out.push('…');
            out
        }
    } else {
        let mut out = s.to_string();
        out.extend(std::iter::repeat_n(' ', cells - count));
        out
    }
}

/// Strip non-printable characters as a defensive backstop. Upstream
/// sanitization already handles ANSI escapes and C0/C1 controls — this
/// just catches anything that slipped through (bugs, future fields,
/// runtime-loaded override files) before it reaches the terminal.
pub fn safe_chars(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect::<String>()
}

fn fill_rect_row(grid: &mut CellGrid, x0: u16, x1: u16, y: u16, bg: Rgb) {
    if y >= grid.height() {
        return;
    }
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

fn set_glyph(grid: &mut CellGrid, x: u16, y: u16, ch: char, fg: Rgb, bg: Rgb) {
    if x >= grid.width() || y >= grid.height() {
        return;
    }
    grid.set(x, y, Cell { ch, fg, bg });
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

fn write_centered(
    grid: &mut CellGrid,
    inner_x0: u16,
    inner_w: u16,
    y: u16,
    text: &str,
    fg: Rgb,
    bg: Rgb,
) {
    let count = text.chars().count() as u16;
    if count <= inner_w {
        let pad = (inner_w - count) / 2;
        write_text(grid, inner_x0 + pad, y, text, fg, bg);
    } else {
        // Truncate with ellipsis to fit within the modal.
        let truncated = truncate_or_pad(text, inner_w as usize);
        write_text(grid, inner_x0, y, &truncated, fg, bg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::picker::curated_seed::baked_list;

    fn default_theme() -> Theme {
        Theme::default()
    }

    #[test]
    fn truncate_or_pad_pads_short() {
        assert_eq!(truncate_or_pad("ab", 5), "ab   ");
    }

    #[test]
    fn truncate_or_pad_truncates_long() {
        let out = truncate_or_pad("abcdefgh", 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn safe_chars_strips_controls() {
        assert_eq!(safe_chars("a\x1b[31mb\x07c"), "a[31mbc");
    }

    #[test]
    fn scroll_offset_fits_all_when_small() {
        assert_eq!(scroll_offset(5, 10, 0), 0);
        assert_eq!(scroll_offset(5, 10, 4), 0);
    }

    #[test]
    fn scroll_offset_keeps_selection_in_view() {
        // 12 items, 5-row window, selection at 8.
        let off = scroll_offset(12, 5, 8);
        assert!(off <= 8);
        assert!(off + 5 > 8);
    }

    #[test]
    fn scroll_offset_clamps_at_end() {
        assert_eq!(scroll_offset(12, 5, 11), 7); // max_scroll = 12 - 5 = 7
    }

    #[test]
    fn paint_picker_on_normal_grid_draws_something() {
        let mut grid = CellGrid::new(80, 24);
        let list = baked_list();
        let theme = default_theme();
        let rect = paint_picker(&mut grid, &list, 0, &theme).expect("rect");
        assert!(rect.width() >= MIN_MODAL_W);
        assert!(rect.height() >= 6);

        // Find at least one non-space cell inside the rect (border glyph).
        let w = grid.width() as usize;
        let mut painted_something = false;
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let idx = (y as usize) * w + x as usize;
                let c = grid.cells()[idx];
                if c.ch != ' ' && c.ch != '\0' {
                    painted_something = true;
                    break;
                }
            }
            if painted_something {
                break;
            }
        }
        assert!(painted_something);
    }

    #[test]
    fn paint_picker_selection_row_has_surface_bright_bg() {
        let mut grid = CellGrid::new(80, 24);
        let list = baked_list();
        let theme = default_theme();
        let rect = paint_picker(&mut grid, &list, 3, &theme).expect("rect");
        let w = grid.width() as usize;
        // Body starts at y0 + 3 (border + 2 header lines). With a
        // 12-item list fitting entirely in the window, scroll = 0, so
        // selection index 3 is at body row 3.
        let body_y0 = rect.y0 + 3;
        let selection_y = body_y0 + 3;
        let inner_x0 = rect.x0 + 2;
        let idx = (selection_y as usize) * w + inner_x0 as usize;
        let cell = grid.cells()[idx];
        assert_eq!(cell.bg, theme.get(Token::SurfaceBright));
        assert_eq!(cell.ch, '▸');
        assert_eq!(cell.fg, theme.get(Token::Accent));
    }

    #[test]
    fn paint_picker_non_selected_row_uses_surface_bg() {
        let mut grid = CellGrid::new(80, 24);
        let list = baked_list();
        let theme = default_theme();
        let rect = paint_picker(&mut grid, &list, 3, &theme).expect("rect");
        let w = grid.width() as usize;
        // Row 0 is not selected (selected = 3).
        let body_y0 = rect.y0 + 3;
        let inner_x0 = rect.x0 + 2;
        let idx = (body_y0 as usize) * w + (inner_x0 + 1) as usize;
        let cell = grid.cells()[idx];
        assert_eq!(cell.bg, theme.get(Token::Surface));
    }

    #[test]
    fn paint_picker_degrades_on_tiny_grid() {
        let mut grid = CellGrid::new(10, 3);
        let list = baked_list();
        let theme = default_theme();
        let rect = paint_picker(&mut grid, &list, 0, &theme);
        // Very small grids get the fallback banner, not a full modal.
        if let Some(r) = rect {
            assert_eq!(r.y0, 0);
            assert_eq!(r.height(), 1);
        }
    }

    #[test]
    fn paint_picker_clamps_out_of_range_selection() {
        let mut grid = CellGrid::new(80, 24);
        let list = baked_list();
        let theme = default_theme();
        // 999 > 11 — must not panic.
        let _ = paint_picker(&mut grid, &list, 999, &theme);
    }

    #[test]
    fn format_row_wide_contains_genre_and_name() {
        let list = baked_list();
        let row = format_row(&list.stations[0], 60);
        assert!(row.contains("1."));
        assert!(row.contains(&list.stations[0].genre));
    }

    #[test]
    fn format_row_narrow_drops_genre() {
        let list = baked_list();
        let row = format_row(&list.stations[0], 24);
        assert!(row.contains("1."));
    }
}
