use super::cell_grid::{Cell, CellGrid, Rgb};

const BRAILLE_BIT: [[u8; 2]; 4] = [
    [0x01, 0x08],
    [0x02, 0x10],
    [0x04, 0x20],
    [0x40, 0x80],
];

pub struct BrailleBuffer {
    dots: Vec<bool>,
    width: u16,
    height: u16,
    cell_w: u16,
    cell_h: u16,
}

impl BrailleBuffer {
    pub fn new(cell_w: u16, cell_h: u16) -> Self {
        let width = cell_w * 2;
        let height = cell_h * 4;
        Self {
            dots: vec![false; width as usize * height as usize],
            width,
            height,
            cell_w,
            cell_h,
        }
    }

    pub fn clear(&mut self) {
        self.dots.fill(false);
    }

    pub fn resize(&mut self, cell_w: u16, cell_h: u16) {
        self.cell_w = cell_w;
        self.cell_h = cell_h;
        self.width = cell_w * 2;
        self.height = cell_h * 4;
        self.dots.clear();
        self.dots
            .resize(self.width as usize * self.height as usize, false);
    }

    pub fn set(&mut self, x: u16, y: u16, on: bool) {
        if x < self.width && y < self.height {
            self.dots[y as usize * self.width as usize + x as usize] = on;
        }
    }

    pub fn get(&self, x: u16, y: u16) -> bool {
        if x < self.width && y < self.height {
            self.dots[y as usize * self.width as usize + x as usize]
        } else {
            false
        }
    }

    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx: i32 = if x0 < x1 { 1 } else { -1 };
        let sy: i32 = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let mut cx = x0;
        let mut cy = y0;
        loop {
            if cx >= 0 && cy >= 0 {
                self.set(cx as u16, cy as u16, true);
            }
            if cx == x1 && cy == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                cx += sx;
            }
            if e2 <= dx {
                err += dx;
                cy += sy;
            }
        }
    }

    pub fn compose<F>(&self, grid: &mut CellGrid, mut fg_fn: F)
    where
        F: FnMut(u16, u16, u8) -> (Rgb, Rgb),
    {
        for cy in 0..self.cell_h {
            for cx in 0..self.cell_w {
                if cx >= grid.width() || cy >= grid.height() {
                    continue;
                }
                let mut mask: u8 = 0;
                let mut dot_count: u8 = 0;
                for row in 0..4u16 {
                    for col in 0..2u16 {
                        let sx = cx * 2 + col;
                        let sy = cy * 4 + row;
                        if self.get(sx, sy) {
                            mask |= BRAILLE_BIT[row as usize][col as usize];
                            dot_count += 1;
                        }
                    }
                }
                let ch = char::from_u32(0x2800 + mask as u32).unwrap_or(' ');
                let (fg, bg) = fg_fn(cx, cy, dot_count);
                grid.set(cx, cy, Cell { ch, fg, bg });
            }
        }
    }

    pub fn cell_w(&self) -> u16 {
        self.cell_w
    }

    pub fn cell_h(&self) -> u16 {
        self.cell_h
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_buffer_composes_to_blank_braille() {
        let buf = BrailleBuffer::new(3, 2);
        let mut grid = CellGrid::new(3, 2);
        buf.compose(&mut grid, |_, _, _| (Rgb::BLACK, Rgb::BLACK));
        for cell in grid.cells() {
            assert_eq!(cell.ch, '\u{2800}');
        }
    }

    #[test]
    fn all_dots_set_composes_to_u28ff() {
        let mut buf = BrailleBuffer::new(1, 1);
        for y in 0..4u16 {
            for x in 0..2u16 {
                buf.set(x, y, true);
            }
        }
        let mut grid = CellGrid::new(1, 1);
        buf.compose(&mut grid, |_, _, _| (Rgb::BLACK, Rgb::BLACK));
        assert_eq!(grid.cells()[0].ch, '\u{28FF}');
    }

    #[test]
    fn known_dot_pattern_produces_correct_codepoint() {
        let mut buf = BrailleBuffer::new(1, 1);
        buf.set(0, 0, true); // bit 0x01
        buf.set(1, 0, true); // bit 0x08
        // Expected: 0x2800 + 0x01 + 0x08 = 0x2809
        let mut grid = CellGrid::new(1, 1);
        buf.compose(&mut grid, |_, _, _| (Rgb::BLACK, Rgb::BLACK));
        assert_eq!(grid.cells()[0].ch, '\u{2809}');
    }

    #[test]
    fn boundary_dots_no_panic() {
        let mut buf = BrailleBuffer::new(2, 2);
        buf.set(3, 7, true); // max valid coords for 2x2 cell grid
        assert!(buf.get(3, 7));
    }

    #[test]
    fn out_of_bounds_set_is_noop() {
        let mut buf = BrailleBuffer::new(2, 2);
        buf.set(4, 8, true); // out of bounds
        buf.set(100, 100, true);
        // Should not panic, dots remain false
        assert!(!buf.get(4, 8));
    }

    #[test]
    fn resize_clears_and_updates_dimensions() {
        let mut buf = BrailleBuffer::new(2, 2);
        buf.set(0, 0, true);
        buf.resize(3, 3);
        assert_eq!(buf.width(), 6);
        assert_eq!(buf.height(), 12);
        assert!(!buf.get(0, 0));
    }

    #[test]
    fn compose_callback_receives_correct_dot_count() {
        let mut buf = BrailleBuffer::new(2, 1);
        // Cell (0,0): set 3 dots
        buf.set(0, 0, true);
        buf.set(1, 0, true);
        buf.set(0, 1, true);
        // Cell (1,0): set 0 dots
        let mut counts = vec![];
        let mut grid = CellGrid::new(2, 1);
        buf.compose(&mut grid, |cx, _cy, dot_count| {
            counts.push((cx, dot_count));
            (Rgb::BLACK, Rgb::BLACK)
        });
        assert_eq!(counts, vec![(0, 3), (1, 0)]);
    }

    #[test]
    fn line_draws_connected_dots() {
        let mut buf = BrailleBuffer::new(5, 1);
        buf.line(0, 0, 9, 0); // horizontal line across full width
        for x in 0..10u16 {
            assert!(buf.get(x, 0), "dot at x={x} should be set");
        }
    }

    #[test]
    fn line_draws_diagonal() {
        let mut buf = BrailleBuffer::new(3, 3);
        buf.line(0, 0, 5, 11);
        assert!(buf.get(0, 0));
        assert!(buf.get(5, 11));
        // At least the endpoints and some intermediate dots should be set
        let set_count: usize = buf.dots.iter().filter(|&&d| d).count();
        assert!(set_count >= 10, "diagonal should set many dots, got {set_count}");
    }

    #[test]
    fn line_with_negative_coords_no_panic() {
        let mut buf = BrailleBuffer::new(3, 3);
        buf.line(-5, -5, 2, 2); // starts out of bounds, enters buffer
        assert!(buf.get(0, 0));
    }
}
