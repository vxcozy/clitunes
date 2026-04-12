//! clitunes — client library surface.
//!
//! Most of the clitunes binary still lives in `main.rs` (render loop,
//! visualisers, picker wiring). This lib target exists so Unit 9's
//! auto-spawn helper can be unit-tested from `tests/auto_spawn_tests.rs`
//! without dragging the entire TUI stack into a harness.

pub mod auto_spawn;
