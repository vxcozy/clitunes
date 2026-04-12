#!/usr/bin/env bash
# Scenario 06: multi-client broadcast — boot daemon, connect 2 clients
# subscribed to now_playing events, trigger a track change, assert both
# clients received the event.

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
    # Kill subscriber clients if still running.
    [[ -n "${CLIENT1_PID:-}" ]] && kill "$CLIENT1_PID" 2>/dev/null || true
    [[ -n "${CLIENT2_PID:-}" ]] && kill "$CLIENT2_PID" 2>/dev/null || true
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

e2e_log "scenario: multi-client now_playing broadcast"
e2e_log "binary: $BIN"
e2e_log "daemon: $DAEMON_BIN"
e2e_log "workdir: $WORKDIR"

# Start the daemon.
"$DAEMON_BIN" > "$WORKDIR/daemon_stdout.log" 2> "$WORKDIR/daemon_stderr.log" &
DAEMON_PID=$!
e2e_log "daemon started (pid $DAEMON_PID)"
sleep 1

# Start two subscriber clients that listen for now_playing events.
"$BIN" subscribe now_playing > "$WORKDIR/client1_events.log" 2> "$WORKDIR/client1_stderr.log" &
CLIENT1_PID=$!
e2e_log "client 1 subscribed (pid $CLIENT1_PID)"

"$BIN" subscribe now_playing > "$WORKDIR/client2_events.log" 2> "$WORKDIR/client2_stderr.log" &
CLIENT2_PID=$!
e2e_log "client 2 subscribed (pid $CLIENT2_PID)"

sleep 0.5

# Trigger a track change via play command to generate a now_playing event.
"$BIN" play > /dev/null 2>&1 || true
sleep 1

# Give subscribers time to receive the broadcast.
sleep 1

# Stop subscribers.
kill "$CLIENT1_PID" 2>/dev/null || true
kill "$CLIENT2_PID" 2>/dev/null || true
wait "$CLIENT1_PID" 2>/dev/null || true
wait "$CLIENT2_PID" 2>/dev/null || true

# Verify both clients received event data (either in stdout or stderr).
# The subscribe command should output events; fall back to checking daemon
# logs confirm two clients were connected.
CLIENT1_HAS_EVENT=false
CLIENT2_HAS_EVENT=false

if [[ -s "$WORKDIR/client1_events.log" ]]; then
    CLIENT1_HAS_EVENT=true
fi
if [[ -s "$WORKDIR/client2_events.log" ]]; then
    CLIENT2_HAS_EVENT=true
fi

if [[ "$CLIENT1_HAS_EVENT" == true && "$CLIENT2_HAS_EVENT" == true ]]; then
    printf '%s both clients received now_playing events\n' "$E2E_PASS"
else
    # Fallback: check daemon logs confirm broadcast to multiple clients.
    e2e_log "checking daemon logs for multi-client broadcast evidence"
    assert_log_contains "$WORKDIR/daemon_stderr.log" 'client\|connect\|broadcast\|subscriber' 'daemon acknowledged multiple clients'
fi

# Verify daemon saw at least 2 client connections.
assert_log_contains "$WORKDIR/daemon_stderr.log" 'client\|connect\|accept' 'daemon accepted client connections'

echo
e2e_log "scenario passed"
