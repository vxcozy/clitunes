//! Per-cell blending logic for transition modes.
//!
//! Each function reads from `src` and `target` grids and writes the
//! blended result into `dst`. All three grids must have identical
//! dimensions — the caller ensures this.

use crate::visualiser::cell_grid::CellGrid;

/// Fade: per-cell linear blend between source and target.
pub fn fade(dst: &mut CellGrid, src: &CellGrid, target: &CellGrid, t: f32) {
    let cells_src = src.cells();
    let cells_tgt = target.cells();
    let w = dst.width();
    let h = dst.height();
    for y in 0..h {
        for x in 0..w {
            let idx = (y as usize) * (w as usize) + (x as usize);
            dst.set(x, y, cells_src[idx].lerp(cells_tgt[idx], t));
        }
    }
}

/// Slide: target slides in from `direction`, pushing source out.
pub fn slide(
    dst: &mut CellGrid,
    src: &CellGrid,
    target: &CellGrid,
    t: f32,
    direction: SlideDirection,
) {
    let w = dst.width() as usize;
    let h = dst.height() as usize;
    let cells_src = src.cells();
    let cells_tgt = target.cells();

    match direction {
        SlideDirection::Left => {
            let offset = (t * w as f32).round() as usize;
            for y in 0..h {
                for x in 0..w {
                    let idx = y * w + x;
                    let cell = if x + offset < w {
                        // Source region (shifted right by offset).
                        cells_src[y * w + x + offset]
                    } else {
                        // Target region (sliding in from right).
                        let tx = x + offset - w;
                        if tx < w {
                            cells_tgt[y * w + tx]
                        } else {
                            cells_tgt[idx]
                        }
                    };
                    dst.set(x as u16, y as u16, cell);
                }
            }
        }
        SlideDirection::Right => {
            let offset = (t * w as f32).round() as usize;
            for y in 0..h {
                for x in 0..w {
                    let idx = y * w + x;
                    let cell = if x >= offset {
                        cells_src[y * w + x - offset]
                    } else {
                        let tx = w - offset + x;
                        if tx < w {
                            cells_tgt[y * w + tx]
                        } else {
                            cells_tgt[idx]
                        }
                    };
                    dst.set(x as u16, y as u16, cell);
                }
            }
        }
        SlideDirection::Up => {
            let offset = (t * h as f32).round() as usize;
            for y in 0..h {
                for x in 0..w {
                    let idx = y * w + x;
                    let cell = if y + offset < h {
                        cells_src[(y + offset) * w + x]
                    } else {
                        let ty = y + offset - h;
                        if ty < h {
                            cells_tgt[ty * w + x]
                        } else {
                            cells_tgt[idx]
                        }
                    };
                    dst.set(x as u16, y as u16, cell);
                }
            }
        }
        SlideDirection::Down => {
            let offset = (t * h as f32).round() as usize;
            for y in 0..h {
                for x in 0..w {
                    let idx = y * w + x;
                    let cell = if y >= offset {
                        cells_src[(y - offset) * w + x]
                    } else {
                        let ty = h - offset + y;
                        if ty < h {
                            cells_tgt[ty * w + x]
                        } else {
                            cells_tgt[idx]
                        }
                    };
                    dst.set(x as u16, y as u16, cell);
                }
            }
        }
    }
}

/// Dissolve: random per-cell reveal using a pre-computed noise mask.
/// Each cell flips from source to target when `t >= threshold[cell]`.
pub fn dissolve(dst: &mut CellGrid, src: &CellGrid, target: &CellGrid, t: f32, thresholds: &[f32]) {
    let cells_src = src.cells();
    let cells_tgt = target.cells();
    let w = dst.width();
    let h = dst.height();
    for y in 0..h {
        for x in 0..w {
            let idx = (y as usize) * (w as usize) + (x as usize);
            let cell = if t >= thresholds[idx] {
                cells_tgt[idx]
            } else {
                cells_src[idx]
            };
            dst.set(x, y, cell);
        }
    }
}

/// Wipe: directional sweep with a soft edge.
pub fn wipe(
    dst: &mut CellGrid,
    src: &CellGrid,
    target: &CellGrid,
    t: f32,
    direction: WipeDirection,
) {
    let w = dst.width() as f32;
    let h = dst.height() as f32;
    let cells_src = src.cells();
    let cells_tgt = target.cells();
    let grid_w = dst.width();
    let grid_h = dst.height();
    // Soft edge width in normalised space.
    let edge = 0.05;

    for y in 0..grid_h {
        for x in 0..grid_w {
            let idx = (y as usize) * (grid_w as usize) + (x as usize);
            let pos = match direction {
                WipeDirection::LeftToRight => x as f32 / w,
                WipeDirection::RightToLeft => 1.0 - x as f32 / w,
                WipeDirection::TopToBottom => y as f32 / h,
                WipeDirection::BottomToTop => 1.0 - y as f32 / h,
            };
            // Map t to a sweep position with soft edge.
            let sweep = t * (1.0 + edge);
            let cell_t = ((sweep - pos) / edge).clamp(0.0, 1.0);
            let cell = cells_src[idx].lerp(cells_tgt[idx], cell_t);
            dst.set(x, y, cell);
        }
    }
}

/// Direction for slide transitions.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SlideDirection {
    Left,
    Right,
    Up,
    Down,
}

/// Direction for wipe transitions.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WipeDirection {
    LeftToRight,
    RightToLeft,
    TopToBottom,
    BottomToTop,
}
