//! Rolling buffer of FFT magnitude rows for the cascade visualiser.
//!
//! Each row is a rebinned, log-compressed snapshot of one frame's
//! spectrum. The history is stored oldest-first and capped at
//! [`MAX_ROWS`] (~30 seconds at 30 fps) to bound memory.

use std::collections::VecDeque;

/// Maximum number of history rows retained.
const MAX_ROWS: usize = 900;

pub struct History {
    rows: VecDeque<Vec<f32>>,
}

impl History {
    pub fn new() -> Self {
        Self {
            rows: VecDeque::new(),
        }
    }

    /// Append a new magnitude row (newest). Evicts the oldest row if the
    /// buffer is full.
    pub fn push_row(&mut self, row: Vec<f32>) {
        self.rows.push_back(row);
        while self.rows.len() > MAX_ROWS {
            self.rows.pop_front();
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Get a row by index. Index 0 is oldest, `len() - 1` is newest.
    pub fn get(&self, idx: usize) -> Option<&[f32]> {
        self.rows.get(idx).map(|v| v.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviction_caps_at_max_rows() {
        let mut h = History::new();
        for i in 0..(MAX_ROWS + 100) {
            h.push_row(vec![i as f32]);
        }
        assert_eq!(h.len(), MAX_ROWS);
    }

    #[test]
    fn ordering_oldest_to_newest() {
        let mut h = History::new();
        h.push_row(vec![1.0]);
        h.push_row(vec![2.0]);
        h.push_row(vec![3.0]);
        assert_eq!(h.get(0), Some([1.0].as_slice()));
        assert_eq!(h.get(1), Some([2.0].as_slice()));
        assert_eq!(h.get(2), Some([3.0].as_slice()));
    }

    #[test]
    fn eviction_preserves_newest() {
        let mut h = History::new();
        for i in 0..(MAX_ROWS + 50) {
            h.push_row(vec![i as f32]);
        }
        // Oldest surviving row should be index 50.
        assert_eq!(h.get(0), Some([50.0].as_slice()));
        // Newest row should be MAX_ROWS + 49.
        let last = (MAX_ROWS + 49) as f32;
        assert_eq!(h.get(h.len() - 1), Some([last].as_slice()));
    }
}
