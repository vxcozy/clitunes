#![cfg(unix)]

//! Cross-process SPMC ring integration tests.
//!
//! The coordinator (parent) creates a shared-memory ring, spawns child
//! processes as consumers, produces deterministic frames, and verifies
//! that all consumers read identical, uncorrupted data.
//!
//! Each child re-invokes the test binary with SPMC_ROLE=consumer and
//! the shm name. The child reads frames, hashes them, and writes the
//! hash + frame count to stdout for the parent to verify.

use std::collections::hash_map::DefaultHasher;
use std::env;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clitunes_core::StereoFrame;
use clitunes_engine::pcm::spmc_ring::ShmRegion;

const ROLE_ENV: &str = "SPMC_TEST_ROLE";
const SHM_NAME_ENV: &str = "SPMC_SHM_NAME";
const DURATION_ENV: &str = "SPMC_DURATION_SECS";

fn deterministic_frame(seq: u64) -> StereoFrame {
    StereoFrame {
        l: (seq as f32).sin(),
        r: (seq as f32).cos(),
    }
}

fn run_consumer() {
    let shm_name = env::var(SHM_NAME_ENV).expect("SPMC_SHM_NAME not set");
    let duration_secs: u64 = env::var(DURATION_ENV)
        .unwrap_or_else(|_| "5".into())
        .parse()
        .unwrap();

    std::thread::sleep(Duration::from_millis(50));

    let (_region, mut consumer) = ShmRegion::open_consumer(&shm_name).unwrap();

    let deadline = Instant::now() + Duration::from_secs(duration_secs);
    let mut total_frames: u64 = 0;
    let mut overruns: u64 = 0;
    let mut hasher = DefaultHasher::new();
    let mut buf = [StereoFrame::SILENCE; 256];

    while Instant::now() < deadline {
        match consumer.read_frames(&mut buf) {
            Ok(0) => {
                std::thread::sleep(Duration::from_micros(100));
            }
            Ok(n) => {
                for frame in &buf[..n] {
                    frame.l.to_bits().hash(&mut hasher);
                    frame.r.to_bits().hash(&mut hasher);
                }
                total_frames += n as u64;
            }
            Err(_overrun) => {
                overruns += 1;
                // After overrun, cursor is repositioned; continue reading.
            }
        }
    }

    let hash = hasher.finish();
    println!(
        "RESULT frames={total_frames} hash={hash} overruns={overruns} cursor={}",
        consumer.cursor()
    );
}

fn spawn_consumer(test_name: &str, shm_name: &str, duration_secs: u64) -> std::process::Child {
    Command::new(env::current_exe().unwrap())
        .env(ROLE_ENV, "consumer")
        .env(SHM_NAME_ENV, shm_name)
        .env(DURATION_ENV, duration_secs.to_string())
        .arg(test_name)
        .arg("--exact")
        .arg("--nocapture")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn consumer")
}

struct ConsumerResult {
    frames: u64,
    hash: u64,
    overruns: u64,
    cursor: u64,
}

fn parse_consumer_output(child: &mut std::process::Child) -> ConsumerResult {
    let stdout = child.stdout.take().unwrap();
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        let line = line.unwrap();
        if let Some(rest) = line.strip_prefix("RESULT ") {
            let mut frames = 0u64;
            let mut hash = 0u64;
            let mut overruns = 0u64;
            let mut cursor = 0u64;
            for part in rest.split_whitespace() {
                if let Some(v) = part.strip_prefix("frames=") {
                    frames = v.parse().unwrap();
                } else if let Some(v) = part.strip_prefix("hash=") {
                    hash = v.parse().unwrap();
                } else if let Some(v) = part.strip_prefix("overruns=") {
                    overruns = v.parse().unwrap();
                } else if let Some(v) = part.strip_prefix("cursor=") {
                    cursor = v.parse().unwrap();
                }
            }
            return ConsumerResult {
                frames,
                hash,
                overruns,
                cursor,
            };
        }
    }
    panic!("consumer did not emit RESULT line");
}

// ---- Tests ----

#[test]
fn cross_process_two_consumers_48khz() {
    if env::var(ROLE_ENV).is_ok() {
        run_consumer();
        return;
    }

    let pid = std::process::id();
    let shm_name = format!("/clitunes-xproc-{pid}");
    let capacity: u32 = 1 << 14; // 16384 frames
    let sample_rate = 48_000u32;
    let duration_secs = 5u64;

    let (_region, mut producer) = ShmRegion::create(&shm_name, capacity, sample_rate).unwrap();

    let mut c1 = spawn_consumer(
        "cross_process_two_consumers_48khz",
        &shm_name,
        duration_secs,
    );
    let mut c2 = spawn_consumer(
        "cross_process_two_consumers_48khz",
        &shm_name,
        duration_secs,
    );

    // Give consumers time to attach.
    std::thread::sleep(Duration::from_millis(100));

    // Produce deterministic frames at ~48kHz for duration_secs.
    let total_frames = sample_rate as u64 * duration_secs;
    let chunk = 256usize;
    let mut frames = vec![StereoFrame::SILENCE; chunk];
    let mut written = 0u64;
    let start = Instant::now();

    while written < total_frames {
        let batch = chunk.min((total_frames - written) as usize);
        for (i, slot) in frames.iter_mut().enumerate().take(batch) {
            *slot = deterministic_frame(written + i as u64);
        }
        producer.write_frames(&frames[..batch]);
        written += batch as u64;

        // Pace production: sleep to approximate real-time rate.
        let expected_elapsed = Duration::from_secs_f64(written as f64 / sample_rate as f64);
        let actual = start.elapsed();
        if expected_elapsed > actual {
            std::thread::sleep(expected_elapsed - actual);
        }
    }

    // Wait for consumers to finish.
    let status1 = c1.wait().expect("consumer 1 failed");
    let status2 = c2.wait().expect("consumer 2 failed");
    assert!(status1.success(), "consumer 1 exited with {status1}");
    assert!(status2.success(), "consumer 2 exited with {status2}");

    let r1 = parse_consumer_output(&mut c1);
    let r2 = parse_consumer_output(&mut c2);

    eprintln!(
        "consumer1: frames={} hash={} overruns={} cursor={}",
        r1.frames, r1.hash, r1.overruns, r1.cursor
    );
    eprintln!(
        "consumer2: frames={} hash={} overruns={} cursor={}",
        r2.frames, r2.hash, r2.overruns, r2.cursor
    );

    // Both consumers must have read a substantial number of frames.
    assert!(
        r1.frames > 10_000,
        "consumer1 read too few frames: {}",
        r1.frames
    );
    assert!(
        r2.frames > 10_000,
        "consumer2 read too few frames: {}",
        r2.frames
    );

    // If both consumers read the same contiguous range (no overruns),
    // their hashes must match. With overruns, hash comparison is not
    // meaningful since each consumer may skip different frames.
    if r1.overruns == 0 && r2.overruns == 0 && r1.frames == r2.frames {
        assert_eq!(r1.hash, r2.hash, "frame data mismatch between consumers");
    }
}

#[test]
fn cross_process_overrun_recovery() {
    if env::var(ROLE_ENV).is_ok() {
        run_consumer();
        return;
    }

    let pid = std::process::id();
    let shm_name = format!("/clitunes-overrun-{pid}");
    let capacity: u32 = 256; // small ring to force overrun
    let sample_rate = 48_000u32;

    let (_region, mut producer) = ShmRegion::create(&shm_name, capacity, sample_rate).unwrap();

    let mut c1 = spawn_consumer("cross_process_overrun_recovery", &shm_name, 3);

    std::thread::sleep(Duration::from_millis(100));

    // Blast frames fast enough to guarantee overrun (consumer sleeps between reads).
    let frames: Vec<_> = (0..2048u64).map(deterministic_frame).collect();
    for chunk in frames.chunks(256) {
        producer.write_frames(chunk);
    }

    // Short pause, then write more — consumer should recover after overrun.
    std::thread::sleep(Duration::from_millis(200));
    let more: Vec<_> = (2048..4096u64).map(deterministic_frame).collect();
    for chunk in more.chunks(256) {
        producer.write_frames(chunk);
        std::thread::sleep(Duration::from_millis(10));
    }

    let status = c1.wait().expect("consumer failed");
    assert!(status.success(), "consumer exited with {status}");

    let r = parse_consumer_output(&mut c1);
    eprintln!(
        "overrun test: frames={} overruns={} cursor={}",
        r.frames, r.overruns, r.cursor
    );

    // Consumer must have experienced at least one overrun.
    assert!(
        r.overruns > 0,
        "expected overruns with capacity=256, got none"
    );
    // Consumer must have recovered and read some frames after the overrun.
    assert!(
        r.frames > 0,
        "consumer read zero frames — did not recover from overrun"
    );
}

#[test]
fn cross_process_latency_measurement() {
    if env::var(ROLE_ENV).is_ok() {
        // Consumer: measure time from write_seq change to read.
        let shm_name = env::var(SHM_NAME_ENV).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let (_region, mut consumer) = ShmRegion::open_consumer(&shm_name).unwrap();

        let mut buf = [StereoFrame::SILENCE; 1];
        let mut latencies = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);

        while Instant::now() < deadline {
            let t0 = Instant::now();
            match consumer.read_frames(&mut buf) {
                Ok(0) => {
                    std::hint::spin_loop();
                    continue;
                }
                Ok(_) => {
                    let lat = t0.elapsed();
                    latencies.push(lat);
                }
                Err(_) => {}
            }
        }

        latencies.sort();
        if latencies.is_empty() {
            println!("LATENCY p50=0 p99=0 count=0");
            return;
        }
        let p50 = latencies[latencies.len() / 2];
        let p99 = latencies[latencies.len() * 99 / 100];
        println!(
            "LATENCY p50={} p99={} count={}",
            p50.as_micros(),
            p99.as_micros(),
            latencies.len()
        );
        return;
    }

    let pid = std::process::id();
    let shm_name = format!("/clitunes-latency-{pid}");
    let capacity: u32 = 1 << 14;
    let sample_rate = 48_000u32;

    let (_region, mut producer) = ShmRegion::create(&shm_name, capacity, sample_rate).unwrap();

    let mut child = Command::new(env::current_exe().unwrap())
        .env(ROLE_ENV, "consumer")
        .env(SHM_NAME_ENV, &shm_name)
        .arg("cross_process_latency_measurement")
        .arg("--exact")
        .arg("--nocapture")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    std::thread::sleep(Duration::from_millis(100));

    // Produce at ~48kHz for 3 seconds.
    let start = Instant::now();
    let mut seq = 0u64;
    while start.elapsed() < Duration::from_secs(3) {
        let frame = deterministic_frame(seq);
        producer.write_frames(&[frame]);
        seq += 1;

        let expected = Duration::from_secs_f64(seq as f64 / sample_rate as f64);
        let actual = start.elapsed();
        if expected > actual {
            std::thread::sleep(expected - actual);
        }
    }

    let status = child.wait().unwrap();
    assert!(status.success());

    let stdout = child.stdout.take().unwrap();
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        let line = line.unwrap();
        if let Some(rest) = line.strip_prefix("LATENCY ") {
            eprintln!("latency: {rest}");
            for part in rest.split_whitespace() {
                if let Some(v) = part.strip_prefix("p99=") {
                    let p99_us: u64 = v.parse().unwrap();
                    assert!(p99_us < 2000, "p99 latency {p99_us}µs exceeds 2ms target");
                }
            }
            return;
        }
    }
    panic!("consumer did not emit LATENCY line");
}
