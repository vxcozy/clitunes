#!/usr/bin/env bash
# Scenario 05: idle shutdown — boot daemon with --idle-timeout 2, connect then
# disconnect a client, assert the daemon exits within 5 seconds.

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

e2e_log "scenario: daemon idle-timeout auto-shutdown"
e2e_log "binary: $BIN"
e2e_log "daemon: $DAEMON_BIN"
e2e_log "workdir: $WORKDIR"

# Start daemon with a 2-second idle timeout.
"$DAEMON_BIN" --foreground --idle-timeout 2 > "$WORKDIR/daemon_stdout.log" 2> "$WORKDIR/daemon_stderr.log" &
DAEMON_PID=$!
e2e_log "daemon started with --idle-timeout 2 (pid $DAEMON_PID)"
sleep 0.5

# Connect a client briefly (status query) then let it disconnect.
"$BIN" status --json > "$WORKDIR/status.json" 2>/dev/null || true
e2e_log "client connected and disconnected"

# Wait up to 5 seconds for daemon to exit on its own.
ELAPSED=0
while kill -0 "$DAEMON_PID" 2>/dev/null; do
    sleep 0.5
    ELAPSED=$((ELAPSED + 1))
    if (( ELAPSED >= 10 )); then
        printf '%s daemon did not exit within 5s after idle timeout\n' "$E2E_FAIL"
        kill "$DAEMON_PID" 2>/dev/null || true
        exit 1
    fi
done

wait "$DAEMON_PID" 2>/dev/null
DAEMON_EXIT=$?
e2e_log "daemon exited with code $DAEMON_EXIT after ~$((ELAPSED / 2))s"

# Exit code 0 means clean idle shutdown.
if [[ "$DAEMON_EXIT" -eq 0 ]]; then
    printf '%s daemon exited cleanly via idle timeout\n' "$E2E_PASS"
else
    printf '%s daemon exit code %s (expected 0 for idle shutdown)\n' "$E2E_FAIL" "$DAEMON_EXIT"
    exit 1
fi

# Verify daemon logs mention the idle shutdown.
assert_log_contains "$WORKDIR/daemon_stderr.log" 'idle|timeout|shutdown' 'idle shutdown logged'

echo
e2e_log "scenario passed"
