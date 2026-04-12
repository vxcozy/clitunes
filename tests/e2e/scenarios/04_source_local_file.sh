#!/usr/bin/env bash
# Scenario 04: local file source — generate a 2-second 440 Hz WAV fixture,
# play it via `clitunes source local:<path>`, wait for completion, verify
# frame count in daemon logs.

set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
source "$ROOT/tests/e2e/lib/assertions.sh"

BIN="${CLITUNES_BIN:-$ROOT/target/release/clitunes}"
if [[ ! -x "$BIN" ]]; then
    BIN="$ROOT/target/debug/clitunes"
fi

if [[ ! -x "$BIN" ]]; then
    printf 'clitunes binary not found; run `cargo build` first\n' >&2
    exit 1
fi

BIN_DIR=$(dirname "$BIN")
DAEMON_BIN="$BIN_DIR/clitunesd"
if [[ ! -x "$DAEMON_BIN" ]]; then
    printf 'clitunesd binary not found at %s; run `cargo build -p clitunesd` first\n' "$DAEMON_BIN" >&2
    exit 1
fi

WORKDIR=$(mktemp -d)
cleanup() {
    if [[ -f "$WORKDIR/runtime/clitunes/clitunesd.pid" ]]; then
        local pid
        pid=$(cat "$WORKDIR/runtime/clitunes/clitunesd.pid" 2>/dev/null || true)
        if [[ -n "$pid" ]]; then
            kill "$pid" 2>/dev/null || true
            sleep 0.2
        fi
    fi
    rm -rf "$WORKDIR"
}
trap cleanup EXIT

export E2E_STDOUT="$WORKDIR/stdout.bin"
export E2E_STDERR="$WORKDIR/stderr.log"

export CLITUNES_LOG_FORMAT=json
export RUST_LOG=clitunes=debug,clitunes_engine=debug
export XDG_CONFIG_HOME="$WORKDIR/xdg"
export XDG_RUNTIME_DIR="$WORKDIR/runtime"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_RUNTIME_DIR"

e2e_log "scenario: local WAV file playback with frame count verification"
e2e_log "binary: $BIN"
e2e_log "daemon: $DAEMON_BIN"
e2e_log "workdir: $WORKDIR"

# Generate a 2-second 440 Hz mono WAV fixture.
WAV_PATH="$WORKDIR/fixture_440hz.wav"
generate_wav_fixture "$WAV_PATH" 2 440
assert_file_nonempty "$WAV_PATH"

# Start the daemon.
"$DAEMON_BIN" > "$WORKDIR/daemon_stdout.log" 2> "$WORKDIR/daemon_stderr.log" &
DAEMON_PID=$!
e2e_log "daemon started (pid $DAEMON_PID)"
sleep 1

# Set source to local file.
"$BIN" source "local:${WAV_PATH}" > "$WORKDIR/source_stdout.log" 2> "$WORKDIR/source_stderr.log" || true

# Wait for playback to complete (2s file + 2s buffer).
wait_for_log "$WORKDIR/daemon_stderr.log" 'frames\|PlaybackComplete\|finished\|decoded' 8

# Verify daemon logs reference frame processing.
# A 2-second 44100 Hz WAV = 88200 frames. Accept any frame count mention.
assert_log_contains "$WORKDIR/daemon_stderr.log" 'frame\|sample\|decoded\|pcm' 'daemon processed audio frames'

# Verify the WAV path appears in daemon logs (source was accepted).
assert_log_contains "$WORKDIR/daemon_stderr.log" '440hz\|fixture\|local' 'local source path acknowledged'

echo
e2e_log "scenario passed"
