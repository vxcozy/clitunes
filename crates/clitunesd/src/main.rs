//! clitunesd — headless daemon stub. Phase 3 wires decoders, radio, control
//! bus, and the SPMC pcm ring. Slice 1 just proves the binary compiles and
//! observes the D15 boundary (cargo tree for this binary must not contain
//! wgpu, ratatui, or crossterm).

use clitunes_engine::observability;

fn main() -> anyhow::Result<()> {
    observability::init_tracing("clitunesd")?;
    tracing::info!(target: "clitunesd", "clitunesd v0.0.1 — slice-1 stub, nothing to play yet");
    Ok(())
}
