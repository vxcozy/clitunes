//! clitunes — slice-1 driver.
//!
//! Pipeline: `ToneSource` → `PcmRing` → `FftTap` → `Auralis` (wgpu) →
//! `KittyWriter` to stdout. The source thread produces PCM, the main thread
//! renders at ~60fps and writes Kitty frames in-place by reusing image id 1.
//!
//! This is the end-to-end proof that slice 1 works. Units 5-8 add radio and
//! the picker on top of this scaffold.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;

use clitunes_core::PcmFormat;
use clitunes_engine::audio::{FftTap, PcmRing};
use clitunes_engine::observability;
use clitunes_engine::sources::{tone_source::ToneSource, Source};
use clitunes_engine::visualiser::{
    auralis::Auralis, kitty_writer::KittyWriter, wgpu_runtime::WgpuRuntime, GpuContext, Visualiser,
};

const WIDTH: u32 = 1024;
const HEIGHT: u32 = 512;
const FFT_SIZE: usize = 2048;
const RING_FRAMES: usize = 48_000; // one second @ 48 kHz
const TONE_BLOCK: usize = 1024;
const TARGET_FRAME: Duration = Duration::from_millis(16); // ~60fps

fn main() -> Result<()> {
    observability::init_tracing("clitunes")?;
    tracing::info!(
        target: "clitunes",
        width = WIDTH,
        height = HEIGHT,
        "slice-1 boot: calibration tone → auralis → kitty"
    );

    let stop = Arc::new(AtomicBool::new(false));
    install_signal_handler(Arc::clone(&stop))?;

    let format = PcmFormat::STUDIO;
    let ring = PcmRing::new(format, RING_FRAMES);

    // Source thread: calibration tone writing into the ring forever.
    let source_stop = Arc::clone(&stop);
    let mut source_writer = ring.writer();
    let source_handle = thread::Builder::new()
        .name("clitunes-tone".into())
        .spawn(move || {
            let mut source = ToneSource::new(format, TONE_BLOCK);
            source.run(&mut source_writer, &source_stop);
        })?;

    // Render thread == main thread. Build wgpu runtime, Auralis pipeline, FFT
    // tap, and start the frame loop.
    let runtime = pollster::block_on(WgpuRuntime::new(WIDTH, HEIGHT))?;
    tracing::info!(
        target: "clitunes",
        adapter = %runtime.adapter_name,
        backend = %runtime.adapter_backend,
        "wgpu runtime ready"
    );

    let mut auralis = Auralis::new(&runtime.device, runtime.target_format);
    let mut fft = FftTap::new(FFT_SIZE);
    let reader = ring.reader();

    let stdout = io::stdout();
    let mut kitty = KittyWriter::new(stdout.lock());
    kitty.clear_screen()?;
    kitty.flush()?;

    let mut frame_idx: u64 = 0;
    let loop_start = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        let frame_start = Instant::now();

        let snapshot = fft.snapshot(&reader, format.sample_rate);

        {
            let mut ctx = GpuContext {
                device: &runtime.device,
                queue: &runtime.queue,
                target_view: &runtime.target_view,
                target_format: runtime.target_format,
                width: runtime.width,
                height: runtime.height,
            };
            auralis.render_gpu(&mut ctx, &snapshot);
        }

        let readout = runtime.readback();
        kitty.cursor_home()?;
        kitty.write_frame(runtime.width, runtime.height, &readout.pixels)?;
        kitty.flush()?;

        frame_idx += 1;
        if frame_idx.is_multiple_of(60) {
            tracing::debug!(
                target: "clitunes",
                frame_idx,
                render_ms = readout.render_time.as_secs_f32() * 1000.0,
                readback_ms = readout.readback_time.as_secs_f32() * 1000.0,
                "frame stats"
            );
        }

        let elapsed = frame_start.elapsed();
        if elapsed < TARGET_FRAME {
            thread::sleep(TARGET_FRAME - elapsed);
        }
    }

    tracing::info!(
        target: "clitunes",
        frames = frame_idx,
        uptime_secs = loop_start.elapsed().as_secs_f32(),
        "shutdown"
    );

    let _ = source_handle.join();
    Ok(())
}

fn install_signal_handler(stop: Arc<AtomicBool>) -> Result<()> {
    // Minimal SIGINT handler via libc. No signal-hook dep for slice 1.
    extern "C" fn handler(_sig: libc::c_int) {
        HANDLED.store(true, Ordering::SeqCst);
    }
    static HANDLED: AtomicBool = AtomicBool::new(false);

    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handler as *const () as usize;
        sa.sa_flags = 0;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
        libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
    }

    // Poll the static flag on a small watcher thread and flip the shared
    // AtomicBool so the main loop can exit.
    thread::Builder::new()
        .name("clitunes-signal".into())
        .spawn(move || loop {
            if HANDLED.load(Ordering::SeqCst) {
                stop.store(true, Ordering::SeqCst);
                return;
            }
            thread::sleep(Duration::from_millis(50));
        })?;
    Ok(())
}
