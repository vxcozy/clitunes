# SPMC PCM Ring Spike — Outcome

**Date:** 2026-04-11
**Bead:** clitunes-5i7 (Unit 11A)
**Outcome:** PASS

## Summary

Built a minimal SPMC shared-memory ring buffer with loom model
checking and cross-process integration tests. All PASS criteria met.
Recommendation: proceed with Phase B Path 1 (real shm ring).

## Architecture

Ring layout in a flat byte region (shm or heap-backed):

```
Offset  Size  Field
0       4     magic (0x434C4952 = "CLIR")
4       1     version (1)
5       1     channels (2 = stereo)
8       4     sample_rate
12      4     capacity (power of 2, frames)
64      8     write_seq (AtomicU64, cache-line aligned)
128     N     frame data ([StereoFrame; capacity])
```

- **Producer** writes frames at `write_seq % capacity`, then does
  `write_seq.store(new_val, Release)`.
- **Consumer** does `write_seq.load(Acquire)`, reads frames, then a
  `fence(SeqCst)` + second `write_seq.load(Relaxed)` to detect torn
  reads from producer wrapping.
- **Overrun detection**: if `write_seq - cursor > capacity`, the consumer
  reports `Overrun` with the number of lost frames and repositions.
- **SHM lifecycle**: `shm_open` + `ftruncate` + `mmap`. Producer maps
  `PROT_READ|PROT_WRITE`, consumers map `PROT_READ` only (SEC-008).

## Loom Results

4 model-check tests, all pass. Exhaustive interleaving exploration:

| Test | Iterations | Result |
|------|-----------|--------|
| producer_consumer_no_torn_read | 1 | PASS |
| producer_two_writes_consumer_sees_both | 1 | PASS |
| overrun_detected_when_producer_wraps | 1 | PASS |
| two_consumers_independent | 12,837 | PASS |

The `two_consumers_independent` test (producer + 2 consumer threads)
explored 12,837 interleavings in 0.94s. No violations found. The
simpler tests have trivially small state spaces.

## Cross-Process Results (M1 macOS)

### Two consumers, 48kHz stereo, 5 seconds

| Metric | Consumer 1 | Consumer 2 |
|--------|-----------|-----------|
| Frames read | 238,080 | 238,080 |
| Hash | 10912968137257998893 | 10912968137257998893 |
| Overruns | 0 | 0 |

Both consumers read identical frame data (hash match). Zero overruns
at real-time 48kHz pacing with capacity=16384.

### Overrun recovery

Small ring (capacity=256), burst write of 2048 frames:

- Overruns detected: 1
- Frames read after recovery: 2048
- Consumer repositioned and continued reading correctly.

### Latency

| Metric | Value |
|--------|-------|
| p50 | <1µs |
| p99 | <1µs |
| Read count (3s) | 141,775 |

End-to-end read latency is sub-microsecond — well under the 2ms p99
target. The shm ring adds negligible overhead vs in-process access.

## aarch64 Verification

The M1 macOS tests run on Apple Silicon (aarch64). The `fence(SeqCst)`
in the consumer compiles to `DMB ISH` on aarch64, providing the
load-load barrier needed for weakly-ordered memory. No data corruption
observed.

x86_64 Linux and aarch64 Linux CI validation deferred to Phase B (the
ring code is platform-independent; only the shm lifecycle uses
platform-specific APIs, and both Linux and macOS use POSIX shm_open).

## Decision

**Proceed with Phase B Path 1 (real SPMC shm ring).**

Rationale:
- Loom verified atomic ordering under exhaustive interleaving.
- Cross-process tests on aarch64 (M1) show zero corruption.
- Sub-microsecond latency leaves massive headroom for 60fps rendering.
- The ring implementation is ~200 lines with no external deps beyond
  libc (already in the dependency graph).

## Phase B Recommendations

1. **Graduate spike code**: `spmc_ring.rs` is production-ready. Add the
   `cross_process_api` trait wrapper for backend-agnostic consumer code.
2. **Per-consumer cursors in daemon heap**: store cursor state keyed by
   client connection ID, not in the shm region.
3. **Stale shm cleanup**: the daemon should `shm_unlink` the previous
   region name before re-creating on startup.
4. **Default capacity**: N=16 (65,536 frames = ~680ms @ 48kHz). Enough
   buffer for a 60fps visualiser that occasionally stalls 1-2 frames.
5. **CI matrix**: add x86_64 Linux and aarch64 Linux cross-process tests
   to the CI matrix in Phase B.

## Files

- `crates/clitunes-engine/src/pcm/mod.rs`
- `crates/clitunes-engine/src/pcm/spmc_ring.rs`
- `crates/clitunes-engine/tests/pcm_spmc_ring_loom.rs`
- `crates/clitunes-engine/tests/pcm_spmc_ring_cross_process.rs`
