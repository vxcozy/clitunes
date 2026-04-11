//! clitunes Phase 0 spike: wgpu off-screen render → Kitty graphics protocol
//! throughput measurement.
//!
//! USAGE:
//!   cargo run -p clitunes-spike --release -- [--width 1024] [--height 512]
//!     [--frames 1800] [--target-fps 60] [--output stdout|null|tty]
//!
//! ENVIRONMENT:
//!   RUST_LOG=info  enables wgpu adapter logs
//!
//! OUTPUT:
//!   Histogram of per-frame end-to-end latency (render → readback → encode → write)
//!   printed to stderr. The decision rule lives in the spike doc, not in the
//!   binary — this just emits the raw measurements.

mod kitty_writer;
mod wgpu_pipeline;

use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

use hdrhistogram::Histogram;

#[derive(Debug, Clone)]
struct Args {
    width: u32,
    height: u32,
    frames: u32,
    target_fps: u32,
    output: OutputKind,
    no_pace: bool,
}

#[derive(Debug, Clone)]
enum OutputKind {
    Stdout,
    Null,
    Tty,
}

impl Args {
    fn parse() -> Self {
        let mut a = Args {
            width: 1024,
            height: 512,
            frames: 1800,
            target_fps: 60,
            output: OutputKind::Null,
            no_pace: false,
        };
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--width" => a.width = it.next().unwrap().parse().unwrap(),
                "--height" => a.height = it.next().unwrap().parse().unwrap(),
                "--frames" => a.frames = it.next().unwrap().parse().unwrap(),
                "--target-fps" => a.target_fps = it.next().unwrap().parse().unwrap(),
                "--output" => {
                    a.output = match it.next().unwrap().as_str() {
                        "stdout" => OutputKind::Stdout,
                        "null" => OutputKind::Null,
                        "tty" => OutputKind::Tty,
                        s => panic!("unknown --output {s}"),
                    }
                }
                "--no-pace" => a.no_pace = true,
                "-h" | "--help" => {
                    eprintln!("usage: clitunes-spike [--width N] [--height N] [--frames N] [--target-fps N] [--output stdout|null|tty] [--no-pace]");
                    std::process::exit(0);
                }
                other => panic!("unknown arg {other}"),
            }
        }
        a
    }
}

enum Sink {
    Buffered(BufWriter<Box<dyn Write + Send>>),
}

impl Sink {
    fn open(kind: &OutputKind) -> std::io::Result<Self> {
        let inner: Box<dyn Write + Send> = match kind {
            OutputKind::Stdout => Box::new(std::io::stdout()),
            OutputKind::Null => Box::new(OpenOptions::new().write(true).open("/dev/null")?),
            OutputKind::Tty => Box::new(OpenOptions::new().write(true).open("/dev/tty")?),
        };
        Ok(Sink::Buffered(BufWriter::with_capacity(
            8 * 1024 * 1024,
            inner,
        )))
    }
    fn writer(&mut self) -> &mut (dyn Write + Send) {
        match self {
            Sink::Buffered(w) => w,
        }
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    eprintln!(
        "clitunes-spike: {w}x{h} frames={f} target_fps={fps} output={out:?} no_pace={np}",
        w = args.width,
        h = args.height,
        f = args.frames,
        fps = args.target_fps,
        out = args.output,
        np = args.no_pace
    );

    let mut pipeline = pollster::block_on(wgpu_pipeline::Pipeline::new(args.width, args.height))?;
    eprintln!(
        "adapter: {} backend={} driver={}",
        pipeline.adapter_info.name, pipeline.adapter_info.backend, pipeline.adapter_info.driver
    );

    let mut sink = Sink::open(&args.output)?;
    let mut kitty = kitty_writer::KittyWriter::new(sink.writer());

    // Histogram of total per-frame latency in microseconds.
    let mut hist_total = Histogram::<u64>::new_with_bounds(10, 60_000_000, 3).unwrap();
    let mut hist_render = Histogram::<u64>::new_with_bounds(10, 60_000_000, 3).unwrap();
    let mut hist_readback = Histogram::<u64>::new_with_bounds(10, 60_000_000, 3).unwrap();
    let mut hist_encode = Histogram::<u64>::new_with_bounds(10, 60_000_000, 3).unwrap();

    let target_frame_dur = Duration::from_secs_f64(1.0 / args.target_fps as f64);
    let run_start = Instant::now();
    let mut last_warm = Instant::now();
    let mut total_bytes_written: u64 = 0;

    // Warm up: 30 frames before measurement starts (skip JIT compilation, GPU init, first-mmap).
    let warmup_frames: u32 = 30;
    for f in 0..warmup_frames {
        let (_r, _rb, pixels) = pipeline.render_and_readback(f);
        let _ = kitty.write_frame(args.width, args.height, &pixels);
    }
    let _ = kitty.flush();
    eprintln!("warmup done in {:?}", last_warm.elapsed());
    last_warm = Instant::now();

    for f in 0..args.frames {
        let frame_start = Instant::now();

        let (render_t, readback_t, pixels) = pipeline.render_and_readback(f + warmup_frames);

        let encode_start = Instant::now();
        let written = kitty.write_frame(args.width, args.height, &pixels)?;
        // Note: we deliberately do NOT flush every frame in --output null mode;
        // we DO flush in stdout/tty mode so the terminal can render. To compare
        // apples to apples we flush in both cases at frame boundary.
        kitty.flush()?;
        let encode_t = encode_start.elapsed();
        total_bytes_written += written as u64;

        let total = frame_start.elapsed();
        hist_total.record(clamp_us(total)).ok();
        hist_render.record(clamp_us(render_t)).ok();
        hist_readback.record(clamp_us(readback_t)).ok();
        hist_encode.record(clamp_us(encode_t)).ok();

        if !args.no_pace {
            if let Some(slack) = target_frame_dur.checked_sub(total) {
                std::thread::sleep(slack);
            }
        }
    }

    let elapsed = run_start.elapsed();
    eprintln!();
    eprintln!("--- results ---");
    eprintln!(
        "frames={} elapsed={:.2}s effective_fps={:.1}",
        args.frames,
        elapsed.as_secs_f64(),
        args.frames as f64 / elapsed.as_secs_f64()
    );
    eprintln!(
        "bytes_written={} ({:.1} MB)",
        total_bytes_written,
        total_bytes_written as f64 / (1024.0 * 1024.0)
    );
    eprintln!();
    print_hist("TOTAL", &hist_total);
    print_hist("render-submit", &hist_render);
    print_hist("readback (poll Wait + map)", &hist_readback);
    print_hist("encode+write", &hist_encode);

    // Decision rule, evaluated mechanically per the bead.
    let p99_total_ms = hist_total.value_at_quantile(0.99) as f64 / 1000.0;
    let p95_total_ms = hist_total.value_at_quantile(0.95) as f64 / 1000.0;
    eprintln!();
    eprintln!("decision (this platform only):");
    let bar_60 = p99_total_ms <= 16.0 && p95_total_ms <= 14.0;
    let bar_30 = p99_total_ms <= 33.0 && p95_total_ms <= 25.0;
    if bar_60 {
        eprintln!("  60fps bar: PASS (p99={p99_total_ms:.2}ms p95={p95_total_ms:.2}ms)");
    } else if bar_30 {
        eprintln!(
            "  60fps bar: fail; 30fps bar: PASS (p99={p99_total_ms:.2}ms p95={p95_total_ms:.2}ms)"
        );
    } else {
        eprintln!(
            "  60fps bar: fail; 30fps bar: fail (p99={p99_total_ms:.2}ms p95={p95_total_ms:.2}ms)"
        );
    }
    eprintln!("  (aggregate decision per bead requires ≥2 of 4 platforms — see docs/spikes/2026-04-11-wgpu-kitty-throughput-spike.md)");

    Ok(())
}

fn clamp_us(d: Duration) -> u64 {
    d.as_micros().min(60_000_000) as u64
}

fn print_hist(label: &str, h: &Histogram<u64>) {
    eprintln!(
        "  {label:32} n={n:>6} p50={p50:6.2}ms p95={p95:6.2}ms p99={p99:6.2}ms p99.9={p999:6.2}ms max={max:6.2}ms",
        label = label,
        n = h.len(),
        p50 = h.value_at_quantile(0.5) as f64 / 1000.0,
        p95 = h.value_at_quantile(0.95) as f64 / 1000.0,
        p99 = h.value_at_quantile(0.99) as f64 / 1000.0,
        p999 = h.value_at_quantile(0.999) as f64 / 1000.0,
        max = h.max() as f64 / 1000.0,
    );
}
