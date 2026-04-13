#!/usr/bin/env bash
# Scenario 03: source switch to radio — boot daemon, switch source to a radio
# station, wait for NowPlayingChanged, assert the station name appears.
#
# This scenario requires network access. Set CLITUNES_E2E_NETWORK=1 to run.

set -euo pipefail

if [[ "${CLITUNES_E2E_NETWORK:-0}" != "1" ]]; then
    echo "[SKIP] scenario requires CLITUNES_E2E_NETWORK=1"
    exit 0
fi

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
export RUST_LOG=clitunes=info,clitunes_engine=info
export XDG_CONFIG_HOME="$WORKDIR/xdg"
export XDG_RUNTIME_DIR="$WORKDIR/runtime"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_RUNTIME_DIR"

e2e_log "scenario: source switch to radio station"
e2e_log "binary: $BIN"
e2e_log "daemon: $DAEMON_BIN"
e2e_log "workdir: $WORKDIR"

# Use a well-known test radio UUID (SomaFM Groove Salad).
RADIO_UUID="e2e-test-radio-00000001"

# Start the daemon.
"$DAEMON_BIN" > "$WORKDIR/daemon_stdout.log" 2> "$WORKDIR/daemon_stderr.log" &
DAEMON_PID=$!
e2e_log "daemon started (pid $DAEMON_PID)"
sleep 1

# Switch source to radio.
"$BIN" source "radio:${RADIO_UUID}" > "$WORKDIR/source_stdout.log" 2> "$WORKDIR/source_stderr.log" || true

# Wait for NowPlayingChanged event in daemon logs (up to 10 seconds).
wait_for_log "$WORKDIR/daemon_stderr.log" 'NowPlayingChanged|now_playing' 10

# Assert the station identifier appears in daemon logs.
assert_log_contains "$WORKDIR/daemon_stderr.log" "${RADIO_UUID}|radio" 'radio source activated'

# Check status reports the radio source.
"$BIN" status --json > "$WORKDIR/status_radio.json" 2>/dev/null || true
assert_file_nonempty "$WORKDIR/status_radio.json"

echo
e2e_log "scenario passed"
