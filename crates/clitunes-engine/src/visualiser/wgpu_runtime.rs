//! Reusable wgpu off-screen device + texture + staging buffer for all GPU
//! visualisers. Slice 1 only uses Auralis; Unit 17 (Tideline) reuses this
//! runtime.

use std::sync::mpsc;
use std::time::{Duration, Instant};

const ALIGN: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

fn padded_bytes_per_row(width: u32) -> u32 {
    let unpadded = width * 4;
    let pad = (ALIGN - (unpadded % ALIGN)) % ALIGN;
    unpadded + pad
}

pub struct WgpuRuntime {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub target_texture: wgpu::Texture,
    pub target_view: wgpu::TextureView,
    pub target_format: wgpu::TextureFormat,
    pub staging_buffer: wgpu::Buffer,
    pub width: u32,
    pub height: u32,
    pub bytes_per_row_padded: u32,
    pub adapter_name: String,
    pub adapter_backend: String,
}

pub struct FrameReadout {
    pub render_time: Duration,
    pub readback_time: Duration,
    pub pixels: Vec<u8>,
}

impl WgpuRuntime {
    pub async fn new(width: u32, height: u32) -> anyhow::Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::default(),
            display: None,
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| anyhow::anyhow!("no adapter: {e:?}"))?;
        let info = adapter.get_info();
        let adapter_name = info.name.clone();
        let adapter_backend = format!("{:?}", info.backend);

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("clitunes-engine-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|e| anyhow::anyhow!("device: {e:?}"))?;

        let target_format = wgpu::TextureFormat::Rgba8Unorm;
        let target_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viz-target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: target_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let bytes_per_row_padded = padded_bytes_per_row(width);
        let staging_size = (bytes_per_row_padded as u64) * (height as u64);
        let staging_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viz-staging"),
            size: staging_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            device,
            queue,
            target_texture,
            target_view,
            target_format,
            staging_buffer,
            width,
            height,
            bytes_per_row_padded,
            adapter_name,
            adapter_backend,
        })
    }

    /// Read the contents of `target_texture` back into a tightly-packed
    /// `Vec<u8>`. Must be called after the visualiser has finished encoding
    /// its render commands and queue.submit has already been called.
    pub fn readback(&self) -> FrameReadout {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("viz-readback-enc"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.staging_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.bytes_per_row_padded),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );

        let render_start = Instant::now();
        self.queue.submit(Some(encoder.finish()));
        let render_time = render_start.elapsed();

        let readback_start = Instant::now();
        let (tx, rx) = mpsc::channel();
        self.staging_buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().unwrap().unwrap();

        let data = self.staging_buffer.slice(..).get_mapped_range();
        let unpadded_row = (self.width * 4) as usize;
        let padded_row = self.bytes_per_row_padded as usize;
        let mut pixels = Vec::with_capacity(unpadded_row * self.height as usize);
        for row in 0..self.height as usize {
            let start = row * padded_row;
            pixels.extend_from_slice(&data[start..start + unpadded_row]);
        }
        drop(data);
        self.staging_buffer.unmap();
        let readback_time = readback_start.elapsed();

        FrameReadout {
            render_time,
            readback_time,
            pixels,
        }
    }
}
