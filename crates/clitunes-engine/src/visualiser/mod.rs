//! Visualiser trait + implementations.
//!
//! The trait is **rendering-path-agnostic** (D8): both GPU+Kitty visualisers
//! (Auralis, Tideline) and the pure-CPU unicode-block Cascade implement it.
//! The dispatch happens on `Visualiser::surface()` returning either
//! `SurfaceKind::Gpu` or `SurfaceKind::Tui`, and the runner feeds the right
//! draw context to each.

use crate::audio::FftSnapshot;
pub use clitunes_core::{SurfaceKind, VisualiserId};

pub mod auralis;
pub mod kitty_writer;
pub mod wgpu_runtime;

pub use auralis::Auralis;

/// Context passed to a GPU visualiser each frame. Holds device references
/// and the current texture that the visualiser should render into.
pub struct GpuContext<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub target_view: &'a wgpu::TextureView,
    pub target_format: wgpu::TextureFormat,
    pub width: u32,
    pub height: u32,
}

/// Context passed to a TUI visualiser each frame. Held back from slice 1
/// (Cascade lands in Unit 18); the type exists so the trait shape is
/// stable now and doesn't churn when Cascade arrives.
pub struct TuiContext<'a> {
    pub _phantom: std::marker::PhantomData<&'a ()>,
}

pub trait Visualiser {
    fn id(&self) -> VisualiserId;
    fn surface(&self) -> SurfaceKind;

    fn render_gpu(&mut self, _ctx: &mut GpuContext<'_>, _fft: &FftSnapshot) {
        unreachable!("render_gpu called on a non-GPU visualiser");
    }

    fn render_tui(&mut self, _ctx: &mut TuiContext<'_>, _fft: &FftSnapshot) {
        unreachable!("render_tui called on a non-TUI visualiser");
    }
}
