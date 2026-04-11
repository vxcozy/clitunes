//! CPU cell grid for truecolor ANSI visualisers.
//!
//! Each cell is one character with a foreground and background truecolor
//! RGB. The default glyph is `▀` (upper half block): `fg` paints the top
//! half of the cell, `bg` paints the bottom half. That gives us 2× vertical
//! resolution on any terminal that speaks 24-bit colour SGR, which is
//! every modern emulator.
//!
//! The grid is row-major; `cells[y * width + x]`. Coordinates use `y=0` at
//! the top of the terminal (standard screen convention). The visualiser
//! flips Y when it wants "grow from the bottom" semantics.

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Self = Self { r: 0, g: 0, b: 0 };

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Cell {
    pub ch: char,
    pub fg: Rgb,
    pub bg: Rgb,
}

impl Cell {
    pub const UPPER_BLOCK: char = '▀';

    pub const fn empty() -> Self {
        Self {
            ch: ' ',
            fg: Rgb::BLACK,
            bg: Rgb::BLACK,
        }
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::empty()
    }
}

pub struct CellGrid {
    width: u16,
    height: u16,
    cells: Vec<Cell>,
}

impl CellGrid {
    pub fn new(width: u16, height: u16) -> Self {
        let cells = vec![Cell::empty(); (width as usize) * (height as usize)];
        Self {
            width,
            height,
            cells,
        }
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.cells.clear();
        self.cells
            .resize((width as usize) * (height as usize), Cell::empty());
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn set(&mut self, x: u16, y: u16, cell: Cell) {
        debug_assert!(x < self.width);
        debug_assert!(y < self.height);
        let idx = (y as usize) * (self.width as usize) + (x as usize);
        self.cells[idx] = cell;
    }

    pub fn fill(&mut self, cell: Cell) {
        for slot in &mut self.cells {
            *slot = cell;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_grid_is_black_spaces() {
        let grid = CellGrid::new(4, 3);
        assert_eq!(grid.width(), 4);
        assert_eq!(grid.height(), 3);
        assert_eq!(grid.cells().len(), 12);
        for c in grid.cells() {
            assert_eq!(c.ch, ' ');
            assert_eq!(c.fg, Rgb::BLACK);
            assert_eq!(c.bg, Rgb::BLACK);
        }
    }

    #[test]
    fn set_and_read_cell() {
        let mut grid = CellGrid::new(4, 3);
        let c = Cell {
            ch: '▀',
            fg: Rgb::new(10, 20, 30),
            bg: Rgb::new(40, 50, 60),
        };
        grid.set(2, 1, c);
        let idx = 4 + 2;
        assert_eq!(grid.cells()[idx].ch, '▀');
        assert_eq!(grid.cells()[idx].fg, Rgb::new(10, 20, 30));
        assert_eq!(grid.cells()[idx].bg, Rgb::new(40, 50, 60));
    }

    #[test]
    fn resize_clears_contents() {
        let mut grid = CellGrid::new(2, 2);
        grid.set(
            0,
            0,
            Cell {
                ch: '#',
                fg: Rgb::new(255, 0, 0),
                bg: Rgb::BLACK,
            },
        );
        grid.resize(3, 3);
        assert_eq!(grid.width(), 3);
        assert_eq!(grid.height(), 3);
        assert_eq!(grid.cells().len(), 9);
        assert_eq!(grid.cells()[0].ch, ' ');
    }
}
