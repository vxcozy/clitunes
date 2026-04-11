//! Auralis — the maximalist instantaneous spectrum visualiser. Slice 1 ships
//! the bare-bones version: a 64-bar spectrum with a warm-to-cool gradient.
//! Future iterations add the bloom, particle overlay, and radial mode
//! variants that define its full identity.

use bytemuck::{Pod, Zeroable};

use crate::audio::FftSnapshot;
use crate::visualiser::{GpuContext, SurfaceKind, Visualiser, VisualiserId};

const NUM_BARS: usize = 64;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
    width: f32,
    height: f32,
    time: f32,
    num_bars: f32,
    bars: [f32; NUM_BARS],
}

pub struct Auralis {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    start: std::time::Instant,
    bar_smoothing: [f32; NUM_BARS],
}

impl Auralis {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("auralis-uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("auralis-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("auralis-bg"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("auralis-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("auralis.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("auralis-pl"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("auralis-pipe"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            pipeline,
            bind_group,
            uniform_buffer,
            start: std::time::Instant::now(),
            bar_smoothing: [0.0; NUM_BARS],
        }
    }

    fn bars_from_fft(&mut self, fft: &FftSnapshot) -> [f32; NUM_BARS] {
        // Log-scale bucket the positive-frequency bins into NUM_BARS groups.
        let bin_count = fft.magnitudes.len().max(1);
        let max_log = ((bin_count - 1) as f32).ln().max(1.0);
        let mut out = [0.0; NUM_BARS];
        for (bar, slot) in out.iter_mut().enumerate() {
            let lo_log = (bar as f32 / NUM_BARS as f32) * max_log;
            let hi_log = ((bar + 1) as f32 / NUM_BARS as f32) * max_log;
            let lo = (lo_log.exp().round() as usize).min(bin_count - 1);
            let hi = (hi_log.exp().round() as usize).clamp(lo + 1, bin_count);
            let slice = &fft.magnitudes[lo..hi];
            let max_mag = slice.iter().cloned().fold(0.0_f32, f32::max);
            // Log-compress + normalise. The /1000 denominator is a hack, but
            // this is Slice 1 — Unit 19 does proper agc.
            let compressed = (1.0 + max_mag / 1000.0).ln();
            *slot = compressed.min(1.0);
        }
        // Smoothing (attack-release) so bars don't flicker.
        for (i, slot) in self.bar_smoothing.iter_mut().enumerate() {
            if out[i] > *slot {
                *slot = 0.6 * *slot + 0.4 * out[i]; // attack
            } else {
                *slot = 0.85 * *slot + 0.15 * out[i]; // release
            }
            out[i] = *slot;
        }
        out
    }
}

impl Visualiser for Auralis {
    fn id(&self) -> VisualiserId {
        VisualiserId::Auralis
    }
    fn surface(&self) -> SurfaceKind {
        SurfaceKind::Gpu
    }
    fn render_gpu(&mut self, ctx: &mut GpuContext<'_>, fft: &FftSnapshot) {
        let bars = self.bars_from_fft(fft);
        let uniforms = Uniforms {
            width: ctx.width as f32,
            height: ctx.height as f32,
            time: self.start.elapsed().as_secs_f32(),
            num_bars: NUM_BARS as f32,
            bars,
        };
        ctx.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("auralis-enc"),
            });
        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("auralis-rp"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: ctx.target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.015,
                            g: 0.010,
                            b: 0.025,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(&self.pipeline);
            rp.set_bind_group(0, &self.bind_group, &[]);
            rp.draw(0..3, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}
