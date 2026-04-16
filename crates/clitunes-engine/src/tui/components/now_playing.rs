//! Now-playing metadata strip: artist + track/album info.
//!
//! Layout (2 lines):
//! ```text
//!   Artist Name                              [bold, foreground-bright]
//!   Track Title — Album Name                 [foreground, album in foreground-dim]
//! ```

use crate::tui::text::{truncate_str, write_str};
use crate::tui::theme::{Theme, Token};
use crate::visualiser::cell_grid::{Cell, CellGrid};

/// Current track metadata. Fed by NowPlayingChanged events.
#[derive(Clone, Debug, Default)]
pub struct NowPlayingState {
    pub artist: Option<String>,
    pub title: Option<String>,
    pub album: Option<String>,
    /// When set, a Spotify Connect client is controlling playback.
    /// Rendered as a dim indicator on the artist line.
    pub connect_device: Option<String>,
}

/// Render the 2-line now-playing strip starting at row `y`.
pub fn render_now_playing(
    grid: &mut CellGrid,
    y: u16,
    x0: u16,
    x1: u16,
    state: &NowPlayingState,
    theme: &Theme,
) {
    let w = x1.saturating_sub(x0) as usize;
    if w < 4 || y + 1 >= grid.height() {
        return;
    }

    let bg = theme.get(Token::Background);
    let bright = theme.get(Token::ForegroundBright);
    let fg = theme.get(Token::Foreground);
    let dim = theme.get(Token::ForegroundDim);

    // Clear both lines.
    for row in y..=(y + 1).min(grid.height().saturating_sub(1)) {
        for x in x0..x1.min(grid.width()) {
            grid.set(
                x,
                row,
                Cell {
                    ch: ' ',
                    fg: bg,
                    bg,
                },
            );
        }
    }

    let indent = x0 + 2;

    // Line 1: artist (bright), with optional Connect indicator right-aligned.
    let connect_tag = state.connect_device.as_ref().map(|name| {
        if name.is_empty() {
            "[Connect]".to_string()
        } else {
            format!("[Connect: {name}]")
        }
    });
    let tag_width = connect_tag.as_ref().map_or(0, |t| t.len() + 1);
    if let Some(ref artist) = state.artist {
        let text = truncate_str(artist, w.saturating_sub(4 + tag_width));
        write_str(grid, indent, y, &text, bright, bg);
    }
    if let Some(ref tag) = connect_tag {
        let tag_x = x1.saturating_sub(tag.len() as u16 + 2);
        write_str(grid, tag_x, y, tag, dim, bg);
    }

    // Line 2: title — album (dim).
    let title = state.title.as_deref().unwrap_or("—");
    if let Some(ref album) = state.album {
        let title_text = truncate_str(title, w.saturating_sub(4));
        let mut cursor = write_str(grid, indent, y + 1, &title_text, fg, bg);

        let album_prefix = " — ";
        let remaining = x1.saturating_sub(cursor) as usize;
        if remaining > album_prefix.len() + 2 {
            cursor = write_str(grid, cursor, y + 1, album_prefix, dim, bg);
            let album_text = truncate_str(album, remaining.saturating_sub(album_prefix.len() + 1));
            write_str(grid, cursor, y + 1, &album_text, dim, bg);
        }
    } else {
        let text = truncate_str(title, w.saturating_sub(4));
        write_str(grid, indent, y + 1, &text, fg, bg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::theme::Theme;

    #[test]
    fn now_playing_renders_artist_bright() {
        let mut grid = CellGrid::new(60, 4);
        let theme = Theme::default();
        let state = NowPlayingState {
            artist: Some("Boards of Canada".into()),
            title: Some("Roygbiv".into()),
            album: Some("Music Has the Right to Children".into()),
            connect_device: None,
        };
        render_now_playing(&mut grid, 0, 0, 60, &state, &theme);

        // Artist on line 0 at indent 2.  (row 0 × width 60 + col 2)
        let idx = 2;
        assert_eq!(grid.cells()[idx].ch, 'B');
        assert_eq!(grid.cells()[idx].fg, theme.get(Token::ForegroundBright));
    }

    #[test]
    fn now_playing_title_and_album() {
        let mut grid = CellGrid::new(60, 4);
        let theme = Theme::default();
        let state = NowPlayingState {
            artist: Some("Artist".into()),
            title: Some("Track".into()),
            album: Some("Album".into()),
            connect_device: None,
        };
        render_now_playing(&mut grid, 0, 0, 60, &state, &theme);

        // Line 1 (y=1) at indent 2: row_offset + indent = width * 1 + 2
        let w = grid.width() as usize;
        let line1_start = w + 2; // y=1, x=2
        assert_eq!(grid.cells()[line1_start].ch, 'T');
        assert_eq!(grid.cells()[line1_start].fg, theme.get(Token::Foreground));
    }

    #[test]
    fn now_playing_truncates_long_text() {
        let mut grid = CellGrid::new(30, 4);
        let theme = Theme::default();
        let state = NowPlayingState {
            artist: Some("A Very Long Artist Name That Should Be Truncated".into()),
            title: Some("Track".into()),
            album: None,
            connect_device: None,
        };
        render_now_playing(&mut grid, 0, 0, 30, &state, &theme);

        // Should contain ellipsis on the artist line.
        let has_ellipsis = (0..30).any(|x| grid.cells()[x].ch == '…');
        assert!(has_ellipsis, "long artist should be truncated");
    }

    #[test]
    fn now_playing_no_artist() {
        let mut grid = CellGrid::new(60, 4);
        let theme = Theme::default();
        let state = NowPlayingState {
            artist: None,
            title: Some("Track Title".into()),
            album: None,
            connect_device: None,
        };
        render_now_playing(&mut grid, 0, 0, 60, &state, &theme);

        // Line 0 should be blank (no artist), line 1 should have title.
        let w = grid.width() as usize;
        let line1_start = w + 2; // y=1, x=2
        assert_eq!(grid.cells()[line1_start].ch, 'T');
    }

    #[test]
    fn now_playing_connect_indicator() {
        let mut grid = CellGrid::new(60, 4);
        let theme = Theme::default();
        let state = NowPlayingState {
            artist: Some("Artist".into()),
            title: Some("Track".into()),
            album: None,
            connect_device: Some("iPhone".into()),
        };
        render_now_playing(&mut grid, 0, 0, 60, &state, &theme);

        let row0: String = (0..60).map(|x| grid.cells()[x].ch).collect();
        assert!(
            row0.contains("[Connect: iPhone]"),
            "connect indicator missing from row: {row0:?}"
        );
    }

    #[test]
    fn now_playing_connect_indicator_no_name() {
        let mut grid = CellGrid::new(60, 4);
        let theme = Theme::default();
        let state = NowPlayingState {
            artist: Some("Artist".into()),
            title: Some("Track".into()),
            album: None,
            connect_device: Some(String::new()),
        };
        render_now_playing(&mut grid, 0, 0, 60, &state, &theme);

        let row0: String = (0..60).map(|x| grid.cells()[x].ch).collect();
        assert!(
            row0.contains("[Connect]"),
            "connect indicator missing from row: {row0:?}"
        );
    }
}
