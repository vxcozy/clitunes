#!/usr/bin/env bash
# Slice 3 boot scenario: boot clitunes with no arguments (daemon-client
# architecture), let it render for 5 seconds, verify the ANSI truecolor
# stream and tracing logs look healthy.
#
# The client auto-spawns clitunesd (the daemon) which handles audio and
# the SPMC PCM ring. The client connects over a Unix socket, receives
# the PcmTap event, opens the shared-memory consumer, and renders
# visualisers at 30 fps.

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

# Verify daemon binary is alongside the client (auto-spawn looks there first).
BIN_DIR=$(dirname "$BIN")
DAEMON_BIN="$BIN_DIR/clitunesd"
if [[ ! -x "$DAEMON_BIN" ]]; then
    printf 'clitunesd binary not found at %s; run `cargo build -p clitunesd` first\n' "$DAEMON_BIN" >&2
    exit 1
fi

WORKDIR=$(mktemp -d)
cleanup() {
    # Kill any daemon we spawned (it double-forks, so find by pidfile or pkill).
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
# Isolate config and runtime dirs so we don't touch the developer's real state.
export XDG_CONFIG_HOME="$WORKDIR/xdg"
export XDG_RUNTIME_DIR="$WORKDIR/runtime"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_RUNTIME_DIR"

e2e_log "scenario: first-run boot → daemon auto-spawn → ANSI CellGrid"
e2e_log "binary: $BIN"
e2e_log "daemon: $DAEMON_BIN"
e2e_log "workdir: $WORKDIR"

# Give 5s so the daemon has time to start and the client can connect + render.
run_for 5 "$BIN"

# Exit code: graceful SIGINT shutdown should return 0. (130 is also acceptable
# if the process was still in its sleep when the signal arrived.)
if [[ "${E2E_EXIT_CODE:-1}" -ne 0 && "${E2E_EXIT_CODE:-1}" -ne 130 ]]; then
    printf '%s unexpected exit code: %s\n' "$E2E_FAIL" "${E2E_EXIT_CODE:-1}"
    # Dump stderr for debugging
    printf '  stderr:\n'
    cat "$E2E_STDERR" | sed 's/^/    /' || true
    exit 1
fi
printf '%s exit code %s accepted\n' "$E2E_PASS" "${E2E_EXIT_CODE:-0}"

assert_file_nonempty "$E2E_STDOUT"
assert_stdout_ansi_truecolor "$E2E_STDOUT"

assert_log_contains "$E2E_STDERR" 'tracing initialised' 'tracing subscriber online'
assert_log_contains "$E2E_STDERR" 'boot: daemon client . visualiser carousel . ansi' 'boot event fired'
assert_log_contains "$E2E_STDERR" 'stdin is not a tty' 'non-interactive mode detected'
assert_log_contains "$E2E_STDERR" 'shutdown' 'clean shutdown'

echo
e2e_log "scenario passed"
