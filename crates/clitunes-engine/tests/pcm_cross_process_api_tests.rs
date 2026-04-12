#![cfg(unix)]

use std::sync::Mutex;

use clitunes_core::StereoFrame;
use clitunes_engine::pcm::cross_process_api::{
    PcmBridge, PcmConsumer, PcmProducer, DEFAULT_CAPACITY,
};
use clitunes_engine::pcm::spmc_ring::ShmRegion;
use clitunes_engine::proto::events::Event;

// PcmBridge::create uses a process-global canonical shm name, so tests
// calling it must be serialized.
static BRIDGE_LOCK: Mutex<()> = Mutex::new(());

fn test_frame(seq: u64) -> StereoFrame {
    StereoFrame {
        l: (seq as f32).sin(),
        r: (seq as f32).cos(),
    }
}

use std::sync::atomic::{AtomicU32, Ordering};
static COUNTER: AtomicU32 = AtomicU32::new(0);

fn unique_shm_name(tag: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("/ct-{}-{}-{n}", tag, std::process::id())
}

// --- PcmBridge trait tests (serialized: canonical name) ---

#[test]
fn bridge_create_uses_canonical_name_and_cleans_stale() {
    let _guard = BRIDGE_LOCK.lock().unwrap();

    let name = {
        let (region, _producer) = <ShmRegion as PcmBridge>::create(256, 48_000).unwrap();
        let n = region.shm_name().to_owned();
        assert!(
            n.starts_with("/clitunes-pcm-v1-"),
            "canonical name mismatch: {n}"
        );
        n
    };

    let (region2, _p2) = <ShmRegion as PcmBridge>::create(512, 48_000).unwrap();
    assert_eq!(region2.shm_name(), name, "canonical name must be stable");
}

#[test]
fn bridge_create_roundtrip_via_trait() {
    let _guard = BRIDGE_LOCK.lock().unwrap();

    let (region, mut producer) = <ShmRegion as PcmBridge>::create(1024, 48_000).unwrap();
    let shm_name = region.shm_name().to_owned();

    let frames: Vec<_> = (0..256).map(test_frame).collect();
    assert_eq!(PcmProducer::write_frames(&mut producer, &frames), 256);
    assert_eq!(PcmProducer::written(&producer), 256);

    let (_r, mut consumer) = <ShmRegion as PcmBridge>::open_consumer_from_start(&shm_name).unwrap();
    let mut buf = [StereoFrame::SILENCE; 512];
    let n = PcmConsumer::read_frames(&mut consumer, &mut buf).unwrap();
    assert_eq!(n, 256);
    assert_eq!(buf[0].l, frames[0].l);
    assert_eq!(buf[255].r, frames[255].r);
    assert_eq!(PcmConsumer::cursor(&consumer), 256);
}

// --- Trait-method tests (unique shm names, safe to run in parallel) ---

#[test]
fn trait_multiple_consumers_independent_cursors() {
    let name = unique_shm_name("multiconsumer");
    let (_region, mut producer) = ShmRegion::create(&name, 1024, 48_000).unwrap();

    let frames: Vec<_> = (0..100).map(test_frame).collect();
    PcmProducer::write_frames(&mut producer, &frames);

    let (_r1, mut c1) = ShmRegion::open_consumer_from_start(&name).unwrap();
    let (_r2, mut c2) = ShmRegion::open_consumer_from_start(&name).unwrap();

    let mut buf = [StereoFrame::SILENCE; 50];
    let n1 = PcmConsumer::read_frames(&mut c1, &mut buf).unwrap();
    assert_eq!(n1, 50);
    assert_eq!(PcmConsumer::cursor(&c1), 50);

    let mut buf2 = [StereoFrame::SILENCE; 100];
    let n2 = PcmConsumer::read_frames(&mut c2, &mut buf2).unwrap();
    assert_eq!(n2, 100);
    assert_eq!(PcmConsumer::cursor(&c2), 100);
}

#[test]
fn trait_overrun_detection_and_recovery() {
    let name = unique_shm_name("overrun");
    let (_region, mut producer) = ShmRegion::create(&name, 256, 48_000).unwrap();
    let (_r, mut consumer) = ShmRegion::open_consumer_from_start(&name).unwrap();
    assert_eq!(PcmConsumer::capacity(&consumer), 256);

    let burst: Vec<_> = (0..512).map(test_frame).collect();
    PcmProducer::write_frames(&mut producer, &burst);

    let mut buf = [StereoFrame::SILENCE; 64];
    let result = PcmConsumer::read_frames(&mut consumer, &mut buf);
    assert!(result.is_err());
    let overrun = result.unwrap_err();
    assert!(overrun.lost_frames > 0);

    let more: Vec<_> = (512..576).map(test_frame).collect();
    PcmProducer::write_frames(&mut producer, &more);

    let n = PcmConsumer::read_frames(&mut consumer, &mut buf).unwrap();
    assert!(n > 0, "consumer should recover after overrun");
}

// --- Constant and event tests ---

#[test]
fn default_capacity_is_power_of_two() {
    assert!(DEFAULT_CAPACITY.is_power_of_two());
    assert_eq!(DEFAULT_CAPACITY, 65_536);
}

#[test]
fn pcm_tap_event_roundtrip() {
    let event = Event::PcmTap {
        shm_name: "/clitunes-pcm-v1-501".into(),
        sample_rate: 48_000,
        channels: 2,
        capacity: 65_536,
    };
    let line = event.to_line();
    let parsed = Event::from_line(&line).unwrap();
    assert_eq!(parsed, event);
    assert_eq!(event.topic(), "pcm_meta");
}

#[test]
fn pcm_tap_event_contains_shm_name_in_json() {
    let event = Event::PcmTap {
        shm_name: "/clitunes-pcm-v1-501".into(),
        sample_rate: 48_000,
        channels: 2,
        capacity: 65_536,
    };
    let json = event.to_line();
    assert!(json.contains("pcm_tap"), "event tag should be pcm_tap");
    assert!(
        json.contains("/clitunes-pcm-v1-501"),
        "shm_name must appear in JSON"
    );
}
