//! Terminal writer that emits a `CellGrid` as 24-bit truecolor SGR + chars.
//!
//! The writer paints one full frame per call. For slice 1 we do no diffing
//! against the previous frame; we home the cursor, walk rows, and emit one
//! SGR prefix per colour change, then the glyph. A typical 200×60 grid at
//! 30 fps spills well under 2 MB/s through the pty — two orders of magnitude
//! less than the wgpu+Kitty path it replaces, and the terminal emulator
//! doesn't have to base64-decode anything.
//!
//! Colour coalescing: adjacent cells sharing the same `(fg, bg)` pair reuse
//! the prior SGR prefix. For bar visualisers this collapses each bar column
//! into a handful of SGR emissions instead of one per cell. The glyph is
//! written as plain UTF-8. At end of frame we emit SGR reset.

use std::io::{self, Write};

use crate::visualiser::cell_grid::{CellGrid, Rgb};

pub struct AnsiWriter<W: Write> {
    out: W,
}

impl<W: Write> AnsiWriter<W> {
    pub fn new(out: W) -> Self {
        Self { out }
    }

    pub fn hide_cursor(&mut self) -> io::Result<()> {
        self.out.write_all(b"\x1b[?25l")
    }

    pub fn show_cursor(&mut self) -> io::Result<()> {
        self.out.write_all(b"\x1b[?25h")
    }

    pub fn clear_screen(&mut self) -> io::Result<()> {
        self.out.write_all(b"\x1b[2J\x1b[H")
    }

    pub fn reset(&mut self) -> io::Result<()> {
        self.out.write_all(b"\x1b[0m")
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }

    pub fn write_frame(&mut self, grid: &CellGrid) -> io::Result<()> {
        let w = grid.width() as usize;
        let h = grid.height() as usize;
        let cells = grid.cells();
        let mut prev_colors: Option<(Rgb, Rgb)> = None;
        let mut char_buf = [0u8; 4];
        for y in 0..h {
            // CUP to column 1 of the next row. Terminal rows are 1-indexed.
            write!(self.out, "\x1b[{};1H", y + 1)?;
            for x in 0..w {
                let cell = cells[y * w + x];
                let next = (cell.fg, cell.bg);
                if prev_colors != Some(next) {
                    write!(
                        self.out,
                        "\x1b[38;2;{};{};{};48;2;{};{};{}m",
                        cell.fg.r, cell.fg.g, cell.fg.b, cell.bg.r, cell.bg.g, cell.bg.b,
                    )?;
                    prev_colors = Some(next);
                }
                let s = cell.ch.encode_utf8(&mut char_buf);
                self.out.write_all(s.as_bytes())?;
            }
        }
        self.out.write_all(b"\x1b[0m")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visualiser::cell_grid::Cell;

    #[test]
    fn write_frame_emits_sgr_and_glyph() {
        let mut grid = CellGrid::new(2, 1);
        grid.set(
            0,
            0,
            Cell {
                ch: '▀',
                fg: Rgb::new(255, 0, 0),
                bg: Rgb::new(0, 0, 255),
            },
        );
        grid.set(
            1,
            0,
            Cell {
                ch: '▀',
                fg: Rgb::new(255, 0, 0),
                bg: Rgb::new(0, 0, 255),
            },
        );
        let mut buf: Vec<u8> = Vec::new();
        let mut w = AnsiWriter::new(&mut buf);
        w.write_frame(&grid).unwrap();
        let s = String::from_utf8(buf).unwrap();
        // Home to row 1 col 1.
        assert!(s.contains("\x1b[1;1H"));
        // Single SGR shared between both cells (colour coalescing).
        let sgr_count = s.matches("\x1b[38;2;").count();
        assert_eq!(sgr_count, 1, "adjacent same-colour cells share one SGR");
        assert!(s.contains("\x1b[38;2;255;0;0;48;2;0;0;255m"));
        // Both glyphs present.
        assert_eq!(s.matches('▀').count(), 2);
        // Reset at end.
        assert!(s.ends_with("\x1b[0m"));
    }

    #[test]
    fn colour_change_emits_new_sgr() {
        let mut grid = CellGrid::new(2, 1);
        grid.set(
            0,
            0,
            Cell {
                ch: '▀',
                fg: Rgb::new(255, 0, 0),
                bg: Rgb::BLACK,
            },
        );
        grid.set(
            1,
            0,
            Cell {
                ch: '▀',
                fg: Rgb::new(0, 255, 0),
                bg: Rgb::BLACK,
            },
        );
        let mut buf: Vec<u8> = Vec::new();
        let mut w = AnsiWriter::new(&mut buf);
        w.write_frame(&grid).unwrap();
        let s = String::from_utf8(buf).unwrap();
        let sgr_count = s.matches("\x1b[38;2;").count();
        assert_eq!(sgr_count, 2, "distinct colours emit distinct SGR prefixes");
    }
}
