# clitunes logging convention

clitunes uses [`tracing`](https://docs.rs/tracing) for all structured logging.
Every binary calls `clitunes_engine::observability::init_tracing(component)`
exactly once on startup. Logs are written to stderr.

## Format

Default format is compact text. Set `CLITUNES_LOG_FORMAT=json` to switch to
newline-delimited JSON for the e2e harness and machine consumers.

```
CLITUNES_LOG_FORMAT=json clitunes | jq .
```

## Filter

Level is controlled by `RUST_LOG`. Default is
`clitunes=info,clitunes_engine=info,{component}=info,warn`.

```
RUST_LOG=clitunes_engine::audio=debug clitunes
```

## Rules

1. **Use structured fields, not interpolated strings.**
   - Good: `tracing::info!(station = %uuid, "station selected")`
   - Bad:  `tracing::info!("station selected: {uuid}")`
2. **Use spans for durations.** Wrap the block of work in
   `let _s = tracing::info_span!("viz.auralis.frame", frame = idx).entered();`.
3. **Never log PII.** No lyrics, no full local file paths (filename-only is
   fine), no user IP addresses. Scrubbing happens in the producer, not the
   subscriber.
4. **Use the named span constants in `observability::spans`.** Do not invent
   ad-hoc span/target strings. If you need a new one, add it to
   `crates/clitunes-engine/src/observability.rs`.
5. **Levels**:
   - `error` — user-visible failure or an invariant violation
   - `warn` — recoverable anomaly that a user should know about
   - `info` — lifecycle events (startup, shutdown, station selected)
   - `debug` — per-frame or per-chunk counters, guarded behind opt-in
   - `trace` — deep diagnostic, noisy by design

## Why JSON toggle

The e2e harness (`tests/e2e/run.sh`) scrapes log lines to verify pipeline
state (e.g. "frame_idx reached 60"). Parsing text is brittle; JSON lets the
harness use `jq` assertions.
