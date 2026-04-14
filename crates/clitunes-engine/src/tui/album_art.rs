//! Album-art rendering into the shared [`CellGrid`] (v1.2 Unit 8).
//!
//! The daemon emits `Event::NowPlayingChanged { art_url, .. }` with
//! a CDN URL (for Spotify, `https://i.scdn.co/image/…`). The client
//! fetches the bytes, decodes them with the `image` crate, and blits
//! the result into the same `CellGrid` the visualisers paint into.
//!
//! # Why halfblock
//!
//! Every modern terminal speaks 24-bit SGR, and the visualisers
//! already use the upper-half-block technique to double vertical
//! resolution: `fg` paints the top half, `bg` the bottom, char is
//! `▀`. That means one terminal cell covers two image pixels and
//! looks indistinguishable from a "real" image on a retina display.
//! The Kitty / Sixel fast paths in the plan can come later — they
//! buy higher fidelity but require terminal-capability detection
//! and escape-sequence plumbing through `AnsiWriter`, and halfblock
//! works everywhere today.
//!
//! # D15 invariant
//!
//! This module is gated behind the `album-art` feature, which in
//! turn `dep:image`s the `image` crate. The daemon never enables
//! `album-art`, so its binary never links `image`. This is the
//! same pattern Unit 1 established for `webapi` → `rspotify`.
//!
//! # Life cycle
//!
//! The caller owns an [`AlbumArtState`]. When a new `art_url`
//! arrives, call [`AlbumArtState::request`]: if the URL differs
//! from the currently-loaded art, the old decoded image is kept
//! painted while a fresh `tokio::spawn` fetches the new bytes.
//! Once the task finishes, [`AlbumArtState::poll_ready`] swaps the
//! decoded image in. Calling [`AlbumArtState::paint`] each frame is
//! cheap — it just blits the cached `RgbImage` into the grid.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use image::{imageops::FilterType, RgbImage};
use tokio::task::JoinHandle;

use crate::visualiser::{Cell, CellGrid, Rgb};

/// Glyph used for every album-art cell. Same one the visualisers use.
const HALF_BLOCK: char = '▀';
/// Hard cap on the fetched CDN response so a misconfigured host can't
/// OOM the client. Spotify covers top out around 500 KB; 4 MB is
/// comfortably above that without being ridiculous.
const MAX_BYTES: u64 = 4 * 1024 * 1024;
/// Request timeout. Album art isn't load-bearing for playback, so we'd
/// rather give up quickly than let a slow CDN stall the picker.
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);

/// A decoded-and-resized album cover, ready to blit into a `CellGrid`.
///
/// The stored image is always RGB (no alpha — Spotify covers are fully
/// opaque, and a black background is a sane fallback anyway).
#[derive(Clone, Debug)]
pub struct AlbumArt {
    /// Source URL this art was fetched from. Used to avoid refetching
    /// the same cover on idempotent `NowPlayingChanged` events.
    pub url: String,
    /// Pre-resized RGB image. Width/height in *pixels*; the render
    /// step treats two vertical pixels as one terminal cell.
    image: RgbImage,
}

impl AlbumArt {
    /// Build an `AlbumArt` from raw bytes (JPEG or PNG). The image is
    /// decoded but NOT resized — the render step resizes per-cell-area
    /// so terminal resizes don't require a re-fetch.
    pub fn decode(url: impl Into<String>, bytes: &[u8]) -> Result<Self> {
        let img = image::load_from_memory(bytes)
            .context("decode album art")?
            .to_rgb8();
        Ok(Self {
            url: url.into(),
            image: img,
        })
    }

    /// Paint this art into `grid` at the given cell-area.
    ///
    /// One terminal cell = two vertical pixels, so a `cells_h` of 10
    /// means a target image height of 20 pixels. `area_w_cells` and
    /// `area_h_cells` are both in cells; the image is centred inside
    /// that area, preserving aspect ratio.
    pub fn paint(&self, grid: &mut CellGrid, x0: u16, y0: u16, area_w: u16, area_h: u16) {
        if area_w == 0 || area_h == 0 {
            return;
        }

        // Target pixel extents: one cell tall == two pixels tall.
        let target_w = area_w as u32;
        let target_h = (area_h as u32) * 2;

        // Preserve aspect ratio: fit inside target_w × target_h.
        let src_w = self.image.width().max(1);
        let src_h = self.image.height().max(1);
        let scale = (target_w as f32 / src_w as f32).min(target_h as f32 / src_h as f32);
        let fit_w = ((src_w as f32) * scale).round().max(1.0) as u32;
        let fit_h = ((src_h as f32) * scale).round().max(1.0) as u32;
        // Snap height to an even number so halfblock pairs always complete.
        let fit_h = (fit_h & !1).max(2);
        let fit_w = fit_w.min(target_w);
        let fit_h = fit_h.min(target_h);

        // Triangle filter: good quality for downscaling, cheap enough
        // to run on every art change (we don't run this per frame).
        let resized = image::imageops::resize(&self.image, fit_w, fit_h, FilterType::Triangle);

        // Centre inside the cell area.
        let cell_w = fit_w as u16;
        let cell_h = (fit_h / 2) as u16;
        let off_x = x0 + (area_w.saturating_sub(cell_w)) / 2;
        let off_y = y0 + (area_h.saturating_sub(cell_h)) / 2;

        let grid_w = grid.width();
        let grid_h = grid.height();

        for cy in 0..cell_h {
            let py_top = (cy as u32) * 2;
            let py_bot = py_top + 1;
            let dest_y = off_y + cy;
            if dest_y >= grid_h {
                break;
            }
            for cx in 0..cell_w {
                let dest_x = off_x + cx;
                if dest_x >= grid_w {
                    break;
                }
                let top = resized.get_pixel(cx as u32, py_top);
                let bot = resized.get_pixel(cx as u32, py_bot);
                grid.set(
                    dest_x,
                    dest_y,
                    Cell {
                        ch: HALF_BLOCK,
                        fg: Rgb::new(top.0[0], top.0[1], top.0[2]),
                        bg: Rgb::new(bot.0[0], bot.0[1], bot.0[2]),
                    },
                );
            }
        }
    }
}

/// In-flight / completed album-art state.
///
/// Held by the client render loop. Poll `poll_ready()` each frame; if
/// the pending fetch has finished, it swaps the new art in and
/// returns `true` so the caller knows to repaint.
pub struct AlbumArtState {
    /// Currently-decoded art, if any. Painted every frame.
    current: Option<AlbumArt>,
    /// Background fetch in flight. `Arc<Mutex<Option<Result<AlbumArt>>>>`
    /// is the shared slot the task writes into on completion.
    pending: Option<Pending>,
}

struct Pending {
    url: String,
    slot: Arc<Mutex<Option<Result<AlbumArt>>>>,
    _handle: JoinHandle<()>,
}

impl Default for AlbumArtState {
    fn default() -> Self {
        Self::new()
    }
}

impl AlbumArtState {
    pub fn new() -> Self {
        Self {
            current: None,
            pending: None,
        }
    }

    /// Currently-loaded art, if any.
    pub fn current(&self) -> Option<&AlbumArt> {
        self.current.as_ref()
    }

    /// URL of the currently-loaded art, if any.
    pub fn current_url(&self) -> Option<&str> {
        self.current.as_ref().map(|a| a.url.as_str())
    }

    /// Request art for `url`. If `url` is already loaded or already in
    /// flight, this is a no-op. If different, any in-flight fetch is
    /// dropped and a new one is spawned.
    ///
    /// # Examples
    ///
    /// Same-URL requests are idempotent: repeatedly requesting the URL
    /// already in flight keeps the same pending fetch rather than
    /// cancelling and re-spawning it.
    ///
    /// ```
    /// # // The tokio::spawn inside request() needs a runtime in scope.
    /// # let rt = tokio::runtime::Builder::new_current_thread()
    /// #     .enable_all()
    /// #     .build()
    /// #     .unwrap();
    /// # let _guard = rt.enter();
    /// use clitunes_engine::tui::album_art::AlbumArtState;
    ///
    /// let mut state = AlbumArtState::new();
    /// assert!(state.current_url().is_none());
    ///
    /// state.request("https://i.scdn.co/image/ab67616d0000b2737f0c3b0e");
    /// // Repeated request for the same URL is a no-op — no panic, no
    /// // redundant fetch. The current art slot stays empty until the
    /// // spawned fetch finishes.
    /// state.request("https://i.scdn.co/image/ab67616d0000b2737f0c3b0e");
    /// assert!(state.current_url().is_none());
    /// ```
    pub fn request(&mut self, url: &str) {
        if self.current_url() == Some(url) {
            return;
        }
        if let Some(pending) = &self.pending {
            if pending.url == url {
                return;
            }
        }

        let slot: Arc<Mutex<Option<Result<AlbumArt>>>> = Arc::new(Mutex::new(None));
        let slot_clone = Arc::clone(&slot);
        let url_owned = url.to_owned();
        let url_for_task = url_owned.clone();

        let handle = tokio::spawn(async move {
            let result = fetch_and_decode(&url_for_task).await;
            // Last writer wins — if the slot was already dropped by a
            // newer request, the mutex still exists but nobody reads it.
            if let Ok(mut guard) = slot_clone.lock() {
                *guard = Some(result);
            }
        });

        self.pending = Some(Pending {
            url: url_owned,
            slot,
            _handle: handle,
        });
    }

    /// Clear any loaded art. Used when track ends / source changes and
    /// no new art is coming.
    pub fn clear(&mut self) {
        self.current = None;
        self.pending = None;
    }

    /// Check if the pending fetch has completed. Returns `true` if the
    /// art slot changed (caller should repaint).
    pub fn poll_ready(&mut self) -> bool {
        let Some(pending) = &self.pending else {
            return false;
        };
        let taken = pending.slot.lock().ok().and_then(|mut g| g.take());
        let Some(result) = taken else {
            return false;
        };
        let url = pending.url.clone();
        self.pending = None;
        match result {
            Ok(art) => {
                self.current = Some(art);
                true
            }
            Err(e) => {
                tracing::warn!(
                    target: "clitunes",
                    %url,
                    error = %e,
                    "album art fetch/decode failed — skipping",
                );
                false
            }
        }
    }

    /// Paint the current art into the grid, if any.
    pub fn paint(&self, grid: &mut CellGrid, x0: u16, y0: u16, area_w: u16, area_h: u16) {
        if let Some(art) = &self.current {
            art.paint(grid, x0, y0, area_w, area_h);
        }
    }
}

/// Fetch `url` and decode into an [`AlbumArt`]. Lives outside the
/// `AlbumArtState` impl so it's testable (no self) and so the spawn
/// closure can move just a String.
async fn fetch_and_decode(url: &str) -> Result<AlbumArt> {
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .context("build http client")?;
    let resp = client
        .get(url)
        .send()
        .await
        .context("album art GET")?
        .error_for_status()
        .context("album art http status")?;
    if let Some(len) = resp.content_length() {
        if len > MAX_BYTES {
            anyhow::bail!("album art too large: {} bytes (limit {})", len, MAX_BYTES);
        }
    }
    let bytes = resp.bytes().await.context("album art body")?;
    if bytes.len() as u64 > MAX_BYTES {
        anyhow::bail!(
            "album art too large: {} bytes (limit {})",
            bytes.len(),
            MAX_BYTES
        );
    }
    AlbumArt::decode(url, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb as ImgRgb};

    fn synthetic_cover(size: u32) -> Vec<u8> {
        // Top half red, bottom half blue — easy to assert against.
        let mut img: ImageBuffer<ImgRgb<u8>, Vec<u8>> = ImageBuffer::new(size, size);
        for (_, y, pixel) in img.enumerate_pixels_mut() {
            *pixel = if y < size / 2 {
                ImgRgb([255, 0, 0])
            } else {
                ImgRgb([0, 0, 255])
            };
        }
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("encode png");
        buf
    }

    #[test]
    fn decode_roundtrip() {
        let bytes = synthetic_cover(16);
        let art = AlbumArt::decode("https://x/y", &bytes).expect("decode");
        assert_eq!(art.url, "https://x/y");
        assert_eq!(art.image.width(), 16);
        assert_eq!(art.image.height(), 16);
    }

    #[test]
    fn paint_fills_requested_cells() {
        let bytes = synthetic_cover(32);
        let art = AlbumArt::decode("x", &bytes).expect("decode");
        let mut grid = CellGrid::new(20, 12);
        art.paint(&mut grid, 2, 1, 8, 4);

        // Somewhere in the 8×4 area the glyph should be the half-block.
        let mut found_block = false;
        for cy in 1..5 {
            for cx in 2..10 {
                let idx = (cy as usize) * 20 + cx as usize;
                if grid.cells()[idx].ch == HALF_BLOCK {
                    found_block = true;
                }
            }
        }
        assert!(found_block, "expected at least one halfblock cell painted");
    }

    #[test]
    fn paint_preserves_top_and_bottom_colors() {
        // Tall cover: top red, bottom blue. A halfblock cell straddling
        // the boundary should have both colors present somewhere in the
        // painted area.
        let bytes = synthetic_cover(64);
        let art = AlbumArt::decode("x", &bytes).expect("decode");
        let mut grid = CellGrid::new(40, 20);
        art.paint(&mut grid, 0, 0, 40, 20);

        let mut saw_red_fg = false;
        let mut saw_blue_bg = false;
        for c in grid.cells() {
            if c.ch != HALF_BLOCK {
                continue;
            }
            if c.fg.r > 200 && c.fg.g < 40 && c.fg.b < 40 {
                saw_red_fg = true;
            }
            if c.bg.b > 200 && c.bg.r < 40 && c.bg.g < 40 {
                saw_blue_bg = true;
            }
        }
        assert!(saw_red_fg, "expected at least one red fg somewhere");
        assert!(saw_blue_bg, "expected at least one blue bg somewhere");
    }

    #[test]
    fn paint_zero_area_is_noop() {
        let bytes = synthetic_cover(16);
        let art = AlbumArt::decode("x", &bytes).expect("decode");
        let mut grid = CellGrid::new(20, 12);
        art.paint(&mut grid, 2, 1, 0, 4);
        art.paint(&mut grid, 2, 1, 4, 0);
        // Grid untouched.
        assert!(grid.cells().iter().all(|c| c.ch == ' '));
    }

    #[test]
    fn request_same_url_twice_is_noop() {
        let mut st = AlbumArtState::new();
        // Seed a current art directly so we don't need the runtime.
        st.current = Some(AlbumArt {
            url: "https://x/y".into(),
            image: RgbImage::new(1, 1),
        });
        st.request("https://x/y");
        assert!(st.pending.is_none(), "same URL should not spawn a fetch");
    }

    #[test]
    fn clear_drops_current_and_pending() {
        let mut st = AlbumArtState::new();
        st.current = Some(AlbumArt {
            url: "https://x/y".into(),
            image: RgbImage::new(1, 1),
        });
        st.clear();
        assert!(st.current.is_none());
        assert!(st.pending.is_none());
    }

    #[test]
    fn decode_garbage_returns_err() {
        let result = AlbumArt::decode("x", b"not an image");
        assert!(result.is_err());
    }
}
