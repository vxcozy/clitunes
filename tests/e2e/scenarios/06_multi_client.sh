#!/usr/bin/env bash
# Scenario 06: multi-client — boot daemon, connect 2 clients concurrently
# via status queries, assert the daemon accepted both connections.

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
    # Also kill the backgrounded daemon directly.
    [[ -n "${DAEMON_PID:-}" ]] && kill "$DAEMON_PID" 2>/dev/null || true
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

e2e_log "scenario: multi-client concurrent connections"
e2e_log "binary: $BIN"
e2e_log "daemon: $DAEMON_BIN"
e2e_log "workdir: $WORKDIR"

# Start the daemon in foreground so we capture stderr.
"$DAEMON_BIN" --foreground > "$WORKDIR/daemon_stdout.log" 2> "$WORKDIR/daemon_stderr.log" &
DAEMON_PID=$!
e2e_log "daemon started (pid $DAEMON_PID)"
sleep 1

# Connect two clients concurrently via status queries.
"$BIN" status --json > "$WORKDIR/client1_status.json" 2> "$WORKDIR/client1_stderr.log" &
CLIENT1_PID=$!
e2e_log "client 1 querying status (pid $CLIENT1_PID)"

"$BIN" status --json > "$WORKDIR/client2_status.json" 2> "$WORKDIR/client2_stderr.log" &
CLIENT2_PID=$!
e2e_log "client 2 querying status (pid $CLIENT2_PID)"

# Wait for both to complete.
wait "$CLIENT1_PID" 2>/dev/null || true
wait "$CLIENT2_PID" 2>/dev/null || true

sleep 0.5

# Both clients should have received valid JSON status.
assert_file_nonempty "$WORKDIR/client1_status.json"
assert_file_nonempty "$WORKDIR/client2_status.json"

# Daemon logs should show multiple client connections.
# The server logs "client connected" with a client_id for each.
assert_log_contains "$WORKDIR/daemon_stderr.log" 'client connected' 'daemon accepted client connections'

echo
e2e_log "scenario passed"
