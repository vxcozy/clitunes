#!/usr/bin/env bash
# Scenario 08: `clitunes auth` detects SSH/headless environments and
# prints port-forward instructions to stderr before launching the OAuth
# flow. Simulates a headless SSH session by setting $SSH_CONNECTION and
# clearing $DISPLAY / $WAYLAND_DISPLAY.
#
# We feed "y\n" to stdin so the librespot consent prompt proceeds, then
# kill the process a couple of seconds later (the real OAuth flow would
# block forever waiting for a browser callback we can't produce in CI).
# The assertion is on stderr: the headless banner must appear before we
# kill the process.

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

WORKDIR=$(mktemp -d)
cleanup() {
    if [[ -n "${AUTH_PID:-}" ]] && kill -0 "$AUTH_PID" 2>/dev/null; then
        kill "$AUTH_PID" 2>/dev/null || true
        sleep 0.2
        kill -KILL "$AUTH_PID" 2>/dev/null || true
    fi
    rm -rf "$WORKDIR"
}
trap cleanup EXIT

# Fake SSH environment, no display.
export SSH_CONNECTION="203.0.113.1 54321 198.51.100.1 22"
unset DISPLAY || true
unset WAYLAND_DISPLAY || true

# Isolate the credentials path so we force a fresh flow.
export HOME="$WORKDIR/home"
export XDG_CONFIG_HOME="$WORKDIR/xdg"
mkdir -p "$HOME" "$XDG_CONFIG_HOME"

export CLITUNES_LOG_FORMAT=text
export RUST_LOG=clitunes=info,clitunes_engine=info

e2e_log "scenario: headless auth prints port-forward instructions"
e2e_log "binary: $BIN"
e2e_log "workdir: $WORKDIR"

# Run `clitunes auth` with "y\n" on stdin (consent prompt) and let it
# run for ~3s. The OAuth listener binds to 127.0.0.1:8898 and blocks on
# a redirect we will never deliver — killing it is the expected path.
( printf 'y\n'; sleep 10 ) | "$BIN" auth \
    > "$WORKDIR/auth_stdout.log" \
    2> "$WORKDIR/auth_stderr.log" &
AUTH_PID=$!
e2e_log "auth started (pid $AUTH_PID)"

# Wait for the headless banner on stderr before killing. Polls up to 10s
# so slow CI runners don't race with the first log flush.
wait_for_log "$WORKDIR/auth_stderr.log" 'Headless mode detected' 10

# Kill the OAuth flow — we don't have a way to complete the redirect.
if kill -0 "$AUTH_PID" 2>/dev/null; then
    kill "$AUTH_PID" 2>/dev/null || true
    # Give it a moment to flush stderr.
    sleep 0.3
    kill -KILL "$AUTH_PID" 2>/dev/null || true
fi
wait "$AUTH_PID" 2>/dev/null || true

assert_file_nonempty "$WORKDIR/auth_stderr.log"
assert_log_contains "$WORKDIR/auth_stderr.log" \
    'Headless mode detected' \
    'headless banner printed'
assert_log_contains "$WORKDIR/auth_stderr.log" \
    'ssh -L 8898:127.0.0.1:8898' \
    'port-forward instruction printed'

echo
e2e_log "scenario passed"
