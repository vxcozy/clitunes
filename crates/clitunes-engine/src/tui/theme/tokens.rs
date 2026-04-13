//! Semantic colour tokens for the clitunes TUI.
//!
//! Every UI colour in the application is expressed as a [`Token`] rather
//! than a raw RGB value. The theme engine resolves tokens to concrete
//! [`Rgb`](crate::visualiser::cell_grid::Rgb) values at paint time.

/// A semantic colour token.
///
/// Variants are grouped by purpose: surfaces (backgrounds), borders,
/// text (foregrounds), and semantic meanings (accent, danger, …).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Token {
    // ── Surfaces ──────────────────────────────────────────────
    /// Deepest background layer.
    Background,
    /// Elevated panel / modal body.
    Surface,
    /// Highest-elevation surface (tooltips, focused panels).
    SurfaceBright,

    // ── Borders ───────────────────────────────────────────────
    /// Default border colour.
    Border,
    /// Focused / active panel border.
    BorderFocus,

    // ── Text ──────────────────────────────────────────────────
    /// Primary body text.
    Foreground,
    /// Secondary / hint text.
    ForegroundDim,
    /// Emphasised text (headings, highlights).
    ForegroundBright,
    /// Disabled / inactive text.
    Muted,

    // ── Semantic ──────────────────────────────────────────────
    /// Primary action / brand colour.
    Accent,
    /// Hovered accent state.
    AccentHover,
    /// Error / destructive.
    Danger,
    /// Caution.
    Warning,
    /// Confirmation / success.
    Success,
}
