//! Reusable TUI components: panels, status line, now-playing strip.

pub mod now_playing;
pub mod panel;
pub mod status_line;

pub use now_playing::{render_now_playing, NowPlayingState};
pub use panel::{draw_panel, draw_panel_with_header, PanelStyle};
pub use status_line::{render_status_line, PlayState, StatusLineState};
