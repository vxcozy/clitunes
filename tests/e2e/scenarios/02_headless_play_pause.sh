#!/usr/bin/env bash
# Scenario 02: headless play/pause — boot daemon, issue play and pause verbs,
# verify state transitions via `clitunes status --json`.

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
export RUST_LOG=clitunes=info,clitunes_engine=info
export XDG_CONFIG_HOME="$WORKDIR/xdg"
export XDG_RUNTIME_DIR="$WORKDIR/runtime"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_RUNTIME_DIR"

e2e_log "scenario: headless play → pause state transitions"
e2e_log "binary: $BIN"
e2e_log "daemon: $DAEMON_BIN"
e2e_log "workdir: $WORKDIR"

# Start the daemon explicitly so we can issue headless commands against it.
"$DAEMON_BIN" --foreground > "$WORKDIR/daemon_stdout.log" 2> "$WORKDIR/daemon_stderr.log" &
DAEMON_PID=$!
e2e_log "daemon started (pid $DAEMON_PID)"
sleep 1

# Issue play command.
"$BIN" play > "$WORKDIR/play_stdout.log" 2> "$WORKDIR/play_stderr.log" || true
sleep 0.5

# Capture status after play.
"$BIN" status --json > "$WORKDIR/status_play.json" 2> "$WORKDIR/status_play_stderr.log" || true

assert_file_nonempty "$WORKDIR/status_play.json"
assert_json_status "$WORKDIR/status_play.json" ".state" "playing" "state is playing after play"

# Issue pause command.
"$BIN" pause > "$WORKDIR/pause_stdout.log" 2> "$WORKDIR/pause_stderr.log" || true
sleep 0.5

# Capture status after pause.
"$BIN" status --json > "$WORKDIR/status_pause.json" 2> "$WORKDIR/status_pause_stderr.log" || true

assert_file_nonempty "$WORKDIR/status_pause.json"
assert_json_status "$WORKDIR/status_pause.json" ".state" "paused" "state is paused after pause"

# Verify daemon logs show the transitions.
assert_log_contains "$WORKDIR/daemon_stderr.log" 'play|Playing' 'daemon received play command'
assert_log_contains "$WORKDIR/daemon_stderr.log" 'pause|Paused' 'daemon received pause command'

echo
e2e_log "scenario passed"
