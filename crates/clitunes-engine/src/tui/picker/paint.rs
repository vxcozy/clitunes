//! Paint the tabbed picker overlay into a `CellGrid`.
//!
//! The picker is a centered modal box with four tabs — Radio, Search,
//! Library, Settings — drawn in a single-line rounded frame with a
//! tab bar, per-tab body, and footer. Only the active tab's body is
//! painted.
//!
//! # Layout math
//!
//! Given a terminal that clitunes has resolved to `(grid_w, grid_h)`,
//! the picker picks the largest comfortable modal box it can fit with
//! these invariants:
//!
//! - Max width  64 cells (wider wastes horizontal space).
//! - Min width  36 cells (narrower truncates tab labels).
//! - Chrome (border 2 + tab bar 2 + footer 2) is 6 rows; the body gets
//!   whatever's left (up to `CURATED_SLOT_COUNT` for Radio, up to a
//!   generous cap for Search/Library).
//!
//! If the terminal is smaller than the min box, [`paint_picker`]
//! degrades gracefully:
//!
//! - Too-narrow → drops the extra tab labels, keeps active tab only.
//! - Too-short → clips the visible list around the selection, keeping
//!   the selected row in view.
//! - Catastrophically small (< 20 cols or < 6 rows) → one-line banner
//!   so the user isn't stuck with no signal at all.
//!
//! # Safety: untrusted strings
//!
//! Station names, track titles, artist names, and playlist names all
//! originate from radio-browser or Spotify and are already sanitized by
//! `clitunes_core::sanitize` at the ingestion boundary. The paint path
//! assumes sanitized input and does not re-sanitize. It **does** strip
//! non-printable characters defensively in [`safe_chars`] as a final
//! backstop before writing into the grid.

use clitunes_core::{BrowseItem, LibraryCategory};

use crate::tui::components::panel::{draw_panel, PanelStyle};
use crate::tui::picker::curated_seed::{CuratedList, CURATED_SLOT_COUNT};
use crate::tui::picker::state::{
    LibraryView, PickerState, PickerTab, SettingsAuthStatus, SettingsSnapshot, LIBRARY_CATEGORIES,
};
use crate::tui::theme::{Theme, Token};
use crate::visualiser::cell_grid::{Cell, CellGrid, Rgb};

/// Header shown above the tab bar on all tabs. Short and warm.
const HEADER_PRIMARY: &str = "Pick a starting point.";

const TAB_RADIO_LABEL: &str = "Radio";
const TAB_SEARCH_LABEL: &str = "Search";
const TAB_LIBRARY_LABEL: &str = "Library";
const TAB_SETTINGS_LABEL: &str = "Settings";

const FOOTER_RADIO: &str = "1 radio  2 search  3 library  4 settings  esc close";
const FOOTER_SEARCH: &str = "type to search   ↑/↓ move   enter play   esc clear";
const FOOTER_LIBRARY: &str = "1 radio  2 search  3 library  4 settings  esc close";
const FOOTER_SETTINGS: &str = "a auth  1 radio  2 search  3 library  4 settings  esc close";

/// Minimum comfortable modal dimensions. See [`paint_picker`] for the
/// fallback behavior when the grid is smaller.
pub const MIN_MODAL_W: u16 = 36;
pub const MIN_MODAL_H: u16 = 14;
pub const MAX_MODAL_W: u16 = 64;

/// Rect returned by [`paint_picker`], exclusive on x1/y1.
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

/// Public paint entry point. Paints the picker modal on top of
/// whatever the visualiser drew into `grid`.
///
/// Returns the bounding rect of the painted modal, or `None` if the
/// grid was too small to paint even the degraded banner.
pub fn paint_picker(
    grid: &mut CellGrid,
    list: &CuratedList,
    state: &PickerState,
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
    let border_fg = theme.get(Token::Border);

    let grid_w = grid.width();
    let grid_h = grid.height();

    // Catastrophically small — one-line banner fallback.
    if grid_w < 20 || grid_h < 6 {
        return paint_fallback_banner(grid, border_fg, bg);
    }

    let modal_w = grid_w.min(MAX_MODAL_W).max(MIN_MODAL_W.min(grid_w));
    // Chrome: border(2) + header(1) + tabbar(1) + gap(1) + footer(1) = 6
    let chrome_h: u16 = 6;
    let max_body = grid_h.saturating_sub(chrome_h);
    // Cap body so the modal doesn't dominate enormous terminals.
    let body_rows = max_body.min(CURATED_SLOT_COUNT as u16 + 6);
    if body_rows == 0 {
        return paint_fallback_banner(grid, border_fg, bg);
    }
    let modal_h = chrome_h + body_rows;

    let x0 = (grid_w.saturating_sub(modal_w)) / 2;
    let y0 = (grid_h.saturating_sub(modal_h)) / 2;
    let x1 = x0 + modal_w;
    let y1 = y0 + modal_h;

    let rect = Rect { x0, y0, x1, y1 };
    let panel_style = PanelStyle {
        border_fg: Token::Border,
        border_bg: Token::Background,
        fill_bg: Token::Surface,
        corner_radius: true,
    };
    draw_panel(grid, rect, &panel_style, theme);

    let inner_x0 = x0 + 2;
    let inner_x1 = x1.saturating_sub(2);
    let inner_w = inner_x1.saturating_sub(inner_x0);

    // Header line (row y0+1): either the welcome primary, or the
    // banner message when one is set (e.g. "last station missing").
    let header_text = state.banner.as_deref().unwrap_or(HEADER_PRIMARY);
    write_centered(
        grid,
        inner_x0,
        inner_w,
        y0 + 1,
        header_text,
        header_fg,
        surface,
    );

    // Tab bar row (y0+2).
    paint_tab_bar(
        grid,
        inner_x0,
        inner_w,
        y0 + 2,
        state.active_tab,
        body_dim_fg,
        header_fg,
        accent,
        surface,
    );

    // Body rows start at y0+3, leave one row above footer.
    let body_y0 = y0 + 3;
    let footer_y = y1.saturating_sub(2);
    let body_y1 = footer_y.saturating_sub(1);
    let body_rows = body_y1.saturating_sub(body_y0);

    if body_rows > 0 {
        match state.active_tab {
            PickerTab::Radio => paint_radio_body(
                grid,
                list,
                state.radio_selected,
                inner_x0,
                inner_x1,
                inner_w,
                body_y0,
                body_rows,
                body_fg,
                body_dim_fg,
                accent,
                surface,
                select_text,
                select_row_bg,
            ),
            PickerTab::Search => paint_search_body(
                grid,
                state,
                inner_x0,
                inner_x1,
                inner_w,
                body_y0,
                body_rows,
                body_fg,
                body_dim_fg,
                accent,
                surface,
                select_text,
                select_row_bg,
            ),
            PickerTab::Library => paint_library_body(
                grid,
                state,
                inner_x0,
                inner_x1,
                inner_w,
                body_y0,
                body_rows,
                body_fg,
                body_dim_fg,
                accent,
                surface,
                select_text,
                select_row_bg,
            ),
            PickerTab::Settings => paint_settings_body(
                grid,
                state.settings.as_ref(),
                state.auth_in_progress,
                state.auth_error.as_deref(),
                inner_x0,
                inner_w,
                body_y0,
                body_rows,
                body_fg,
                body_dim_fg,
                accent,
                surface,
            ),
        }
    }

    // Footer (tab-specific hint).
    let footer = match state.active_tab {
        PickerTab::Radio => FOOTER_RADIO,
        PickerTab::Search => FOOTER_SEARCH,
        PickerTab::Library => FOOTER_LIBRARY,
        PickerTab::Settings => FOOTER_SETTINGS,
    };
    write_centered(
        grid,
        inner_x0,
        inner_w,
        footer_y,
        footer,
        body_dim_fg,
        surface,
    );

    Some(rect)
}

#[allow(clippy::too_many_arguments)]
fn paint_tab_bar(
    grid: &mut CellGrid,
    inner_x0: u16,
    inner_w: u16,
    y: u16,
    active: PickerTab,
    dim_fg: Rgb,
    bright_fg: Rgb,
    accent: Rgb,
    surface: Rgb,
) {
    // Compose " Radio   Search   Library " with the active tab marked
    // by a leading ▸ accent and a brighter fg.
    let labels = [
        (PickerTab::Radio, TAB_RADIO_LABEL),
        (PickerTab::Search, TAB_SEARCH_LABEL),
        (PickerTab::Library, TAB_LIBRARY_LABEL),
        (PickerTab::Settings, TAB_SETTINGS_LABEL),
    ];
    // Build per-segment strings, then center-pack them.
    let spacer = "  ";
    let mut total: usize = 0;
    for (_, label) in &labels {
        total += 1 + label.chars().count(); // 1 for marker ▸ or space
    }
    total += spacer.len() * (labels.len() - 1);
    if (total as u16) > inner_w {
        // Too narrow — paint active label only.
        let (_, label) = labels.iter().find(|(t, _)| *t == active).unwrap();
        let text = format!("▸ {label}");
        write_centered(grid, inner_x0, inner_w, y, &text, accent, surface);
        return;
    }

    let start_x = inner_x0 + (inner_w.saturating_sub(total as u16)) / 2;
    let mut x = start_x;
    for (i, (tab, label)) in labels.iter().enumerate() {
        if i > 0 {
            write_text(grid, x, y, spacer, dim_fg, surface);
            x = x.saturating_add(spacer.len() as u16);
        }
        let is_active = *tab == active;
        let (marker, fg) = if is_active {
            ("▸", accent)
        } else {
            (" ", dim_fg)
        };
        write_text(grid, x, y, marker, fg, surface);
        x = x.saturating_add(1);
        let label_fg = if is_active { bright_fg } else { dim_fg };
        write_text(grid, x, y, label, label_fg, surface);
        x = x.saturating_add(label.chars().count() as u16);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_radio_body(
    grid: &mut CellGrid,
    list: &CuratedList,
    selected: usize,
    inner_x0: u16,
    inner_x1: u16,
    inner_w: u16,
    body_y0: u16,
    body_rows: u16,
    body_fg: Rgb,
    _body_dim_fg: Rgb,
    accent: Rgb,
    surface: Rgb,
    select_text: Rgb,
    select_row_bg: Rgb,
) {
    let selected = selected.min(list.stations.len().saturating_sub(1));
    let scroll = scroll_offset(list.stations.len(), body_rows as usize, selected);
    for row in 0..body_rows {
        let idx = scroll + row as usize;
        if idx >= list.stations.len() {
            break;
        }
        let station = &list.stations[idx];
        let is_selected = idx == selected;
        let line = format_station_row(station, inner_w as usize);
        let (fg, row_bg) = if is_selected {
            (select_text, select_row_bg)
        } else {
            (body_fg, surface)
        };
        fill_rect_row(grid, inner_x0, inner_x1, body_y0 + row, row_bg);
        write_text(grid, inner_x0, body_y0 + row, &line, fg, row_bg);
        if is_selected {
            set_glyph(grid, inner_x0, body_y0 + row, '▸', accent, row_bg);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_search_body(
    grid: &mut CellGrid,
    state: &PickerState,
    inner_x0: u16,
    inner_x1: u16,
    inner_w: u16,
    body_y0: u16,
    body_rows: u16,
    body_fg: Rgb,
    body_dim_fg: Rgb,
    accent: Rgb,
    surface: Rgb,
    select_text: Rgb,
    select_row_bg: Rgb,
) {
    // Row 0: search input field.
    let input_y = body_y0;
    fill_rect_row(grid, inner_x0, inner_x1, input_y, surface);
    let prompt = "› ";
    write_text(grid, inner_x0, input_y, prompt, accent, surface);
    let q_x = inner_x0 + prompt.chars().count() as u16;
    let q_cells = inner_w.saturating_sub(prompt.chars().count() as u16);
    let query_line = truncate_or_pad(&safe_chars(&state.search_query), q_cells as usize);
    write_text(grid, q_x, input_y, &query_line, body_fg, surface);
    // Caret: underline at current cursor position when focused.
    let caret_x = q_x + state.search_query.chars().count().min(q_cells as usize) as u16;
    set_glyph(grid, caret_x, input_y, '▏', accent, surface);

    // Remaining rows: paginated results.
    if body_rows <= 1 {
        return;
    }
    let list_rows = body_rows - 1;
    let list_y0 = input_y + 1;

    if state.search_results.is_empty() {
        let hint = if state.search_query.is_empty() {
            "Type to search Spotify."
        } else {
            "No results yet — still typing?"
        };
        write_centered(
            grid,
            inner_x0,
            inner_w,
            list_y0 + list_rows / 2,
            hint,
            body_dim_fg,
            surface,
        );
        return;
    }

    let scroll = scroll_offset(
        state.search_results.len(),
        list_rows as usize,
        state.search_cursor,
    );
    for row in 0..list_rows {
        let idx = scroll + row as usize;
        if idx >= state.search_results.len() {
            break;
        }
        let item = &state.search_results[idx];
        let is_selected = idx == state.search_cursor;
        let line = format_track_row(item, inner_w as usize);
        let (fg, row_bg) = if is_selected {
            (select_text, select_row_bg)
        } else {
            (body_fg, surface)
        };
        fill_rect_row(grid, inner_x0, inner_x1, list_y0 + row, row_bg);
        write_text(grid, inner_x0, list_y0 + row, &line, fg, row_bg);
        if is_selected {
            set_glyph(grid, inner_x0, list_y0 + row, '▸', accent, row_bg);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_library_body(
    grid: &mut CellGrid,
    state: &PickerState,
    inner_x0: u16,
    inner_x1: u16,
    inner_w: u16,
    body_y0: u16,
    body_rows: u16,
    body_fg: Rgb,
    body_dim_fg: Rgb,
    accent: Rgb,
    surface: Rgb,
    select_text: Rgb,
    select_row_bg: Rgb,
) {
    match &state.library {
        LibraryView::Categories { cursor } => {
            for (i, category) in LIBRARY_CATEGORIES.iter().enumerate() {
                if i as u16 >= body_rows {
                    break;
                }
                let is_selected = i == *cursor;
                let label = category_label(*category);
                let line = format!(" {}", truncate_or_pad(label, inner_w as usize - 1));
                let (fg, row_bg) = if is_selected {
                    (select_text, select_row_bg)
                } else {
                    (body_fg, surface)
                };
                fill_rect_row(grid, inner_x0, inner_x1, body_y0 + i as u16, row_bg);
                write_text(grid, inner_x0, body_y0 + i as u16, &line, fg, row_bg);
                if is_selected {
                    set_glyph(grid, inner_x0, body_y0 + i as u16, '▸', accent, row_bg);
                }
            }
            // Sub-hint row below the categories.
            if (LIBRARY_CATEGORIES.len() as u16) < body_rows {
                write_centered(
                    grid,
                    inner_x0,
                    inner_w,
                    body_y0 + LIBRARY_CATEGORIES.len() as u16 + 1,
                    "Enter to open.",
                    body_dim_fg,
                    surface,
                );
            }
        }
        LibraryView::Items {
            category,
            items,
            cursor,
        } => {
            // Title row: "‹ Saved Tracks" so the user knows where they are.
            let title = format!("‹ {}", category_label(*category));
            let title_line = truncate_or_pad(&title, inner_w as usize);
            write_text(grid, inner_x0, body_y0, &title_line, body_dim_fg, surface);

            if body_rows <= 1 {
                return;
            }
            let list_rows = body_rows - 1;
            let list_y0 = body_y0 + 1;

            if items.is_empty() {
                write_centered(
                    grid,
                    inner_x0,
                    inner_w,
                    list_y0 + list_rows / 2,
                    "Nothing here yet.",
                    body_dim_fg,
                    surface,
                );
                return;
            }

            let scroll = scroll_offset(items.len(), list_rows as usize, *cursor);
            for row in 0..list_rows {
                let idx = scroll + row as usize;
                if idx >= items.len() {
                    break;
                }
                let item = &items[idx];
                let is_selected = idx == *cursor;
                let line = format_track_row(item, inner_w as usize);
                let (fg, row_bg) = if is_selected {
                    (select_text, select_row_bg)
                } else {
                    (body_fg, surface)
                };
                fill_rect_row(grid, inner_x0, inner_x1, list_y0 + row, row_bg);
                write_text(grid, inner_x0, list_y0 + row, &line, fg, row_bg);
                if is_selected {
                    set_glyph(grid, inner_x0, list_y0 + row, '▸', accent, row_bg);
                }
            }
        }
    }
}

/// Paint the read-only Settings tab. Shows:
///
/// - Spotify auth state (logged in / logged out / scopes / unreadable)
/// - Connect device name + whether the receiver is enabled
/// - Resolved daemon.toml path
/// - The one shell command the user needs to authenticate
///
/// The tab intentionally owns no input: the picker emits
/// `PickerAction::ReadConfig` when entering the tab and on Enter so
/// the daemon echoes a fresh snapshot; everything else is rendered
/// from that snapshot.
#[allow(clippy::too_many_arguments)]
fn paint_settings_body(
    grid: &mut CellGrid,
    snapshot: Option<&SettingsSnapshot>,
    auth_in_progress: bool,
    auth_error: Option<&str>,
    inner_x0: u16,
    inner_w: u16,
    body_y0: u16,
    body_rows: u16,
    body_fg: Rgb,
    body_dim_fg: Rgb,
    accent: Rgb,
    surface: Rgb,
) {
    if body_rows == 0 {
        return;
    }
    let Some(snap) = snapshot else {
        write_centered(
            grid,
            inner_x0,
            inner_w,
            body_y0 + body_rows / 2,
            "Loading daemon config…",
            body_dim_fg,
            surface,
        );
        return;
    };

    // Lay out as a stack of (label, value) rows. Truncation happens
    // per-row via `truncate_or_pad` — we never overflow the modal
    // horizontally because the outer `paint_picker` math already
    // reserved `inner_w` for us.
    let mut row: u16 = body_y0;
    let rows_end = body_y0 + body_rows;

    let write_kv = |grid: &mut CellGrid, row_y: u16, label: &str, value: &str, value_fg: Rgb| {
        if row_y >= rows_end {
            return;
        }
        // Reserve a 2-column left gutter so the rows don't kiss the
        // panel border.
        if inner_w < 6 {
            return;
        }
        let label_cells = (inner_w as usize).min(14);
        let label_out = truncate_or_pad(label, label_cells);
        write_text(grid, inner_x0 + 1, row_y, &label_out, body_dim_fg, surface);
        let value_x = inner_x0 + 1 + label_cells as u16;
        let value_w = (inner_w as usize).saturating_sub(1 + label_cells);
        if value_w == 0 {
            return;
        }
        let value_out = truncate_or_pad(&safe_chars(value), value_w);
        write_text(grid, value_x, row_y, &value_out, value_fg, surface);
    };

    // Row 1 — Spotify auth state. The user's most-asked question when
    // opening Settings is "am I logged in?", so it gets the top slot.
    let (status_text, status_fg) = match snap.auth_status {
        SettingsAuthStatus::LoggedIn => ("Logged in".to_string(), accent),
        SettingsAuthStatus::LoggedOut => ("Logged out".to_string(), body_fg),
        SettingsAuthStatus::ScopesInsufficient => {
            ("Needs re-auth (missing scopes)".to_string(), body_fg)
        }
        SettingsAuthStatus::Unreadable => {
            let detail = snap
                .auth_detail
                .as_deref()
                .map(|s| s.split(':').next().unwrap_or(s).trim())
                .unwrap_or("credential file unreadable");
            (format!("Error: {detail}"), body_fg)
        }
    };
    write_kv(grid, row, "Spotify:", &status_text, status_fg);
    row += 1;

    // Row 2 — device name + enabled flag.
    let device_line = if snap.connect_enabled {
        format!("{} (Connect enabled)", snap.device_name)
    } else {
        format!("{} (Connect disabled)", snap.device_name)
    };
    write_kv(grid, row, "Device:", &device_line, body_fg);
    row += 1;

    // Row 3 — config path. Paths are long; truncate with an ellipsis.
    let config_path = snap.config_path.as_deref().unwrap_or("(not resolved)");
    write_kv(grid, row, "Config:", config_path, body_dim_fg);
    row += 1;

    // Row 4 — credentials path (only meaningful when not logged out).
    if let Some(cred_path) = snap.credentials_path.as_deref() {
        write_kv(grid, row, "Creds:", cred_path, body_dim_fg);
        row += 1;
    }

    // Blank spacer, then the auth-instruction row.
    row = row.saturating_add(1);
    if row < rows_end {
        // Pending state dominates: while the OAuth flow is running,
        // re-running `clitunes auth` would race the daemon, so only
        // show the progress line.
        if auth_in_progress {
            // Two rows: primary status then a secondary hint pointing
            // SSH/headless users at the sibling CLI, since the daemon
            // can't surface the OAuth URL through the TUI today (see
            // TODO(librespot-oauth) in sources/spotify/auth.rs). The
            // sibling CLI runs in the same process as the user's
            // terminal so it can print the URL directly.
            write_centered(
                grid,
                inner_x0,
                inner_w,
                row,
                "Opening browser… waiting for Spotify to complete sign-in.",
                accent,
                surface,
            );
            let hint_row = row.saturating_add(1);
            if hint_row < rows_end {
                write_centered(
                    grid,
                    inner_x0,
                    inner_w,
                    hint_row,
                    "Headless? Cancel and run `clitunes auth` from a shell.",
                    body_dim_fg,
                    surface,
                );
                row = hint_row;
            }
        } else {
            let instruction = match snap.auth_status {
                SettingsAuthStatus::LoggedIn => "Press `a` to refresh scopes or switch accounts.",
                SettingsAuthStatus::LoggedOut => {
                    "Press `a` to sign in — opens Spotify in your browser."
                }
                SettingsAuthStatus::ScopesInsufficient => "Press `a` to grant the new scopes.",
                SettingsAuthStatus::Unreadable => {
                    "Remove the credential file and press `a` to retry."
                }
            };
            write_centered(grid, inner_x0, inner_w, row, instruction, body_fg, surface);
        }
        row = row.saturating_add(1);
    }

    // Error banner — only when a previous flow failed and the user
    // hasn't retried yet.
    if let Some(reason) = auth_error {
        if row < rows_end && !auth_in_progress {
            let msg = format!("Last attempt failed: {reason}");
            write_centered(grid, inner_x0, inner_w, row, &msg, body_fg, surface);
            row = row.saturating_add(1);
        }
    }

    // Edit-config hint — only meaningful when we know the path and we
    // still have screen real-estate left.
    if row < rows_end && snap.config_path.is_some() {
        write_centered(
            grid,
            inner_x0,
            inner_w,
            row,
            "Edit daemon.toml to rename the device or enable Connect.",
            body_dim_fg,
            surface,
        );
    }
}

fn category_label(category: LibraryCategory) -> &'static str {
    match category {
        LibraryCategory::SavedTracks => "Saved Tracks",
        LibraryCategory::SavedAlbums => "Saved Albums",
        LibraryCategory::Playlists => "Playlists",
        LibraryCategory::RecentlyPlayed => "Recently Played",
    }
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

/// Compute a scroll offset for a body list such that `selected` is
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

/// Build a curated-station row. Layout is `NN. Genre        Name` when
/// wide enough, falling back to `NN. Name` when genre won't fit.
pub fn format_station_row(station: &clitunes_core::CuratedStation, inner_w: usize) -> String {
    let slot = station.slot + 1;
    let name = safe_chars(&station.name);
    let genre = safe_chars(&station.genre);

    let body_w = inner_w.saturating_sub(6);
    if body_w == 0 {
        return format!("{slot:>2}");
    }

    let narrow = body_w < 22;
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

/// Build a Spotify track/album/playlist row. Layout is `Title — Artist`
/// when an artist is present, else `Title`. Both sanitized.
pub fn format_track_row(item: &BrowseItem, inner_w: usize) -> String {
    let title = safe_chars(&item.title);
    let artist = item.artist.as_deref().map(safe_chars);

    // Reserve 2 cells for leading "▸ " or "  " and 1 cell right pad.
    let body_w = inner_w.saturating_sub(3);
    if body_w == 0 {
        return String::new();
    }

    match artist {
        Some(a) if !a.is_empty() => {
            // Two-thirds title, one-third artist, with a soft dash between.
            let title_w = body_w * 2 / 3;
            let sep_w = 3; // " — "
            let artist_w = body_w.saturating_sub(title_w + sep_w);
            format!(
                "  {title} — {artist} ",
                title = truncate_or_pad(&title, title_w),
                artist = truncate_or_pad(&a, artist_w),
            )
        }
        _ => format!("  {title} ", title = truncate_or_pad(&title, body_w)),
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

/// Strip non-printable characters as a defensive backstop.
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

    fn browse_item(title: &str, artist: Option<&str>) -> BrowseItem {
        BrowseItem {
            title: title.into(),
            artist: artist.map(Into::into),
            album: None,
            uri: "spotify:track:x".into(),
            art_url: None,
            duration_ms: None,
        }
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
        let off = scroll_offset(12, 5, 8);
        assert!(off <= 8);
        assert!(off + 5 > 8);
    }

    #[test]
    fn scroll_offset_clamps_at_end() {
        assert_eq!(scroll_offset(12, 5, 11), 7);
    }

    #[test]
    fn paint_picker_radio_tab_draws_something() {
        let mut grid = CellGrid::new(80, 24);
        let list = baked_list();
        let theme = default_theme();
        let state = PickerState::new(&list, 0);
        let rect = paint_picker(&mut grid, &list, &state, &theme).expect("rect");
        assert!(rect.width() >= MIN_MODAL_W);

        // Find a painted cell inside the body area.
        let mut painted = false;
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let idx = (y as usize) * grid.width() as usize + x as usize;
                if grid.cells()[idx].ch != ' ' {
                    painted = true;
                    break;
                }
            }
            if painted {
                break;
            }
        }
        assert!(painted);
    }

    #[test]
    fn paint_picker_search_tab_shows_prompt() {
        let mut grid = CellGrid::new(80, 24);
        let list = baked_list();
        let theme = default_theme();
        let mut state = PickerState::new(&list, 0);
        state.active_tab = PickerTab::Search;
        state.search_query = "daft".into();
        let _rect = paint_picker(&mut grid, &list, &state, &theme).expect("rect");
        // The prompt "› " should appear somewhere in the grid.
        let cells = grid.cells();
        assert!(cells.iter().any(|c| c.ch == '›'));
        // "daft" should be visible.
        let row_has_query = (0..grid.height()).any(|y| {
            let row: String = (0..grid.width())
                .map(|x| cells[(y as usize) * grid.width() as usize + x as usize].ch)
                .collect();
            row.contains("daft")
        });
        assert!(row_has_query);
    }

    #[test]
    fn paint_picker_library_tab_shows_all_categories() {
        let mut grid = CellGrid::new(80, 24);
        let list = baked_list();
        let theme = default_theme();
        let mut state = PickerState::new(&list, 0);
        state.active_tab = PickerTab::Library;
        let _rect = paint_picker(&mut grid, &list, &state, &theme).expect("rect");
        let cells = grid.cells();
        let text: String = cells.iter().map(|c| c.ch).collect();
        for cat in LIBRARY_CATEGORIES {
            assert!(
                text.contains(category_label(*cat)),
                "missing label for {cat:?}"
            );
        }
    }

    #[test]
    fn paint_picker_tab_bar_marks_active_tab() {
        let mut grid = CellGrid::new(80, 24);
        let list = baked_list();
        let theme = default_theme();
        let mut state = PickerState::new(&list, 0);
        state.active_tab = PickerTab::Search;
        let _rect = paint_picker(&mut grid, &list, &state, &theme).expect("rect");
        // The tab bar row should contain '▸' before "Search".
        let w = grid.width() as usize;
        let tab_row: String = (0..grid.width())
            .map(|x| grid.cells()[(2 + 1) * w + x as usize])
            .map(|c| c.ch)
            .collect::<String>();
        // Not asserting exact layout — just presence of marker + label.
        let full: String = grid.cells().iter().map(|c| c.ch).collect();
        assert!(full.contains("Search"));
        // Marker appears somewhere in a row near the top.
        assert!(full.contains('▸'));
        let _ = tab_row;
    }

    #[test]
    fn paint_picker_degrades_on_tiny_grid() {
        let mut grid = CellGrid::new(10, 3);
        let list = baked_list();
        let theme = default_theme();
        let state = PickerState::new(&list, 0);
        let rect = paint_picker(&mut grid, &list, &state, &theme);
        if let Some(r) = rect {
            assert_eq!(r.y0, 0);
            assert_eq!(r.height(), 1);
        }
    }

    #[test]
    fn paint_picker_clamps_out_of_range_radio_selection() {
        let mut grid = CellGrid::new(80, 24);
        let list = baked_list();
        let theme = default_theme();
        let mut state = PickerState::new(&list, 0);
        state.radio_selected = 999; // bypass new() clamp
                                    // Must not panic.
        let _ = paint_picker(&mut grid, &list, &state, &theme);
    }

    #[test]
    fn format_station_row_wide_contains_genre_and_name() {
        let list = baked_list();
        let row = format_station_row(&list.stations[0], 60);
        assert!(row.contains("1."));
        assert!(row.contains(&list.stations[0].genre));
    }

    #[test]
    fn format_station_row_narrow_drops_genre() {
        let list = baked_list();
        let row = format_station_row(&list.stations[0], 24);
        assert!(row.contains("1."));
    }

    #[test]
    fn format_track_row_without_artist_shows_title_only() {
        let item = browse_item("Roygbiv", None);
        let row = format_track_row(&item, 40);
        assert!(row.contains("Roygbiv"));
        assert!(!row.contains(" — "));
    }

    fn sample_settings_snapshot(status: SettingsAuthStatus) -> SettingsSnapshot {
        SettingsSnapshot {
            device_name: "clitunes".into(),
            connect_enabled: false,
            config_path: Some("/home/u/.config/clitunes/daemon.toml".into()),
            credentials_path: Some("/home/u/.config/clitunes/spotify/credentials.json".into()),
            auth_status: status,
            auth_detail: None,
        }
    }

    #[test]
    fn paint_picker_settings_tab_shows_logged_out_state() {
        let mut grid = CellGrid::new(120, 40);
        let list = baked_list();
        let theme = default_theme();
        let mut state = PickerState::new(&list, 0);
        state.active_tab = PickerTab::Settings;
        state.set_settings(sample_settings_snapshot(SettingsAuthStatus::LoggedOut));
        let _rect = paint_picker(&mut grid, &list, &state, &theme).expect("rect");
        let text: String = grid.cells().iter().map(|c| c.ch).collect();
        assert!(text.contains("Logged out"), "missing auth status");
        assert!(text.contains("clitunes"), "missing device name");
        assert!(text.contains("daemon.toml"), "missing config path");
        // New in-TUI auth: the instruction should tell users to press `a`.
        assert!(
            text.contains("Press `a` to sign in"),
            "missing in-TUI auth-trigger instruction"
        );
        // Footer should advertise the new keybinds.
        assert!(text.contains("4 settings"), "footer missing 4 settings");
        assert!(text.contains("a auth"), "footer missing a auth keybind");
    }

    #[test]
    fn paint_picker_settings_tab_shows_pending_state() {
        let mut grid = CellGrid::new(120, 40);
        let list = baked_list();
        let theme = default_theme();
        let mut state = PickerState::new(&list, 0);
        state.active_tab = PickerTab::Settings;
        state.set_settings(sample_settings_snapshot(SettingsAuthStatus::LoggedOut));
        state.set_auth_started();
        let _rect = paint_picker(&mut grid, &list, &state, &theme).expect("rect");
        let text: String = grid.cells().iter().map(|c| c.ch).collect();
        assert!(
            text.contains("Opening browser"),
            "missing pending-state message"
        );
        assert!(
            !text.contains("Press `a` to sign in"),
            "pending state should replace the sign-in hint"
        );
    }

    #[test]
    fn paint_picker_settings_tab_pending_state_shows_headless_fallback_hint() {
        let mut grid = CellGrid::new(120, 40);
        let list = baked_list();
        let theme = default_theme();
        let mut state = PickerState::new(&list, 0);
        state.active_tab = PickerTab::Settings;
        state.set_settings(sample_settings_snapshot(SettingsAuthStatus::LoggedOut));
        state.set_auth_started();
        let _rect = paint_picker(&mut grid, &list, &state, &theme).expect("rect");
        let text: String = grid.cells().iter().map(|c| c.ch).collect();
        assert!(
            text.contains("clitunes auth"),
            "pending state should point headless users at the sibling CLI"
        );
    }

    #[test]
    fn paint_picker_settings_tab_shows_auth_error() {
        let mut grid = CellGrid::new(120, 40);
        let list = baked_list();
        let theme = default_theme();
        let mut state = PickerState::new(&list, 0);
        state.active_tab = PickerTab::Settings;
        state.set_settings(sample_settings_snapshot(SettingsAuthStatus::LoggedOut));
        state.set_auth_failed("timeout".into());
        let _rect = paint_picker(&mut grid, &list, &state, &theme).expect("rect");
        let text: String = grid.cells().iter().map(|c| c.ch).collect();
        assert!(text.contains("Last attempt failed"), "missing error banner");
        assert!(text.contains("timeout"), "missing error reason");
    }

    #[test]
    fn paint_picker_settings_tab_shows_logged_in_state() {
        let mut grid = CellGrid::new(120, 40);
        let list = baked_list();
        let theme = default_theme();
        let mut state = PickerState::new(&list, 0);
        state.active_tab = PickerTab::Settings;
        state.set_settings(sample_settings_snapshot(SettingsAuthStatus::LoggedIn));
        let _rect = paint_picker(&mut grid, &list, &state, &theme).expect("rect");
        let text: String = grid.cells().iter().map(|c| c.ch).collect();
        assert!(text.contains("Logged in"), "missing logged-in status");
    }

    #[test]
    fn paint_picker_settings_tab_without_snapshot_shows_loading() {
        let mut grid = CellGrid::new(120, 40);
        let list = baked_list();
        let theme = default_theme();
        let mut state = PickerState::new(&list, 0);
        state.active_tab = PickerTab::Settings;
        let _rect = paint_picker(&mut grid, &list, &state, &theme).expect("rect");
        let text: String = grid.cells().iter().map(|c| c.ch).collect();
        assert!(
            text.contains("Loading daemon config"),
            "expected loading placeholder"
        );
    }

    #[test]
    fn paint_picker_tab_bar_shows_all_four_labels() {
        let mut grid = CellGrid::new(120, 40);
        let list = baked_list();
        let theme = default_theme();
        let state = PickerState::new(&list, 0);
        let _rect = paint_picker(&mut grid, &list, &state, &theme).expect("rect");
        let text: String = grid.cells().iter().map(|c| c.ch).collect();
        for label in ["Radio", "Search", "Library", "Settings"] {
            assert!(text.contains(label), "tab bar missing {label}");
        }
    }

    #[test]
    fn format_track_row_with_artist_has_separator() {
        let item = browse_item("Get Lucky", Some("Daft Punk"));
        let row = format_track_row(&item, 60);
        assert!(row.contains("Get Lucky"));
        assert!(row.contains("Daft Punk"));
        assert!(row.contains(" — "));
    }
}
