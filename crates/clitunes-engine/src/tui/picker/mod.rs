//! Curated taste-neutral station picker (Unit 8).
//!
//! The picker is the first thing a new user sees: a modal overlay
//! painted over the Auralis calibration tone, listing 8–15 curated
//! stations that span genres without reflecting the engineer's taste.
//!
//! # Why "taste-neutral"
//!
//! Auto-memory rule `feedback_no_taste_imposition.md` is explicit:
//! **never hardcode defaults that reflect developer taste**. The
//! curated list is therefore documented per slot in
//! `docs/curation/2026-04-11-curated-stations.md` with a rationale
//! explaining why that slot exists (genre coverage, regional balance,
//! stream stability) — not "I like this station."
//!
//! # Override path
//!
//! Users who want a different seed can drop a file at
//! `~/.config/clitunes/curated_stations.toml` and [`load_curated`]
//! will prefer it over the baked list. The override is strict: if the
//! file exists but is empty or malformed, we fall back to the baked
//! list and log a warning — this matches the plan's edge case and
//! means "I messed up my override file" never leaves the user staring
//! at a blank picker.
//!
//! # Submodules
//!
//! - [`curated_seed`] — baked-in slot list + override loader
//! - `paint` / `state` — the picker UI itself (next task)

pub mod curated_seed;
pub mod paint;
pub mod state;

pub use curated_seed::{load_curated, CuratedList, CuratedLoadOutcome, CURATED_SLOT_COUNT};
pub use paint::{paint_picker, Rect};
pub use state::{key_from_bytes, PickerAction, PickerKey, PickerState};
