#!/usr/bin/env bash
# Scenario 07: `clitunes search` without credentials returns a clear auth
# error. Boots a daemon with an empty $XDG_CONFIG_HOME so no Spotify
# credentials exist, issues `clitunes search "…"`, and asserts that:
# - The CLI exits non-zero.
# - stderr mentions "no cached Spotify credentials" (i.e. the dispatcher
#   surfaces the daemon-safe auth error verbatim).
#
# This exercises the no-auth path end-to-end: daemon dispatch → webapi
# build → load_credentials → CommandResult { ok: false } → headless.rs
# bail → CLI exit code. Crucially the daemon must NOT launch an OAuth
# flow here (load_credentials is the daemon-safe path).

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
    if [[ -n "${DAEMON_PID:-}" ]]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        sleep 0.2
    fi
    rm -rf "$WORKDIR"
}
trap cleanup EXIT

export CLITUNES_LOG_FORMAT=json
export RUST_LOG=clitunes=info,clitunes_engine=info,clitunesd=info
export XDG_CONFIG_HOME="$WORKDIR/xdg"
export XDG_RUNTIME_DIR="$WORKDIR/runtime"
# Pretend we have a home with no Spotify creds (dirs::config_dir falls
# back to $HOME/.config on Linux, $HOME/Library/… on macOS; either way
# the path will not contain a credentials.json).
export HOME="$WORKDIR/home"
# Pre-create platform cache/config dirs so clitunesd's tracing sink and
# dirs::config_dir() both resolve to writable paths under our fake HOME.
mkdir -p \
    "$XDG_CONFIG_HOME" "$XDG_RUNTIME_DIR" "$HOME" \
    "$HOME/.config/clitunes" \
    "$HOME/.cache/clitunes" \
    "$HOME/Library/Application Support/clitunes" \
    "$HOME/Library/Caches/clitunes"

e2e_log "scenario: search without credentials returns auth error"
e2e_log "binary: $BIN"
e2e_log "daemon: $DAEMON_BIN"
e2e_log "workdir: $WORKDIR"

# Start the daemon.
"$DAEMON_BIN" --foreground > "$WORKDIR/daemon_stdout.log" 2> "$WORKDIR/daemon_stderr.log" &
DAEMON_PID=$!
e2e_log "daemon started (pid $DAEMON_PID)"
sleep 1

# Issue search. Expect non-zero exit.
set +e
"$BIN" search "test query" > "$WORKDIR/search_stdout.log" 2> "$WORKDIR/search_stderr.log"
SEARCH_EXIT=$?
set -e
e2e_log "search exit code: $SEARCH_EXIT"

if (( SEARCH_EXIT == 0 )); then
    printf '%s search unexpectedly succeeded without credentials\n' "$E2E_FAIL"
    echo "stdout:"; cat "$WORKDIR/search_stdout.log" | sed 's/^/  /'
    echo "stderr:"; cat "$WORKDIR/search_stderr.log" | sed 's/^/  /'
    exit 1
fi
printf '%s search exited non-zero as expected (%d)\n' "$E2E_PASS" "$SEARCH_EXIT"

assert_log_contains "$WORKDIR/search_stderr.log" \
    'no cached Spotify credentials|spotify auth|authenticate' \
    'auth error surfaced to stderr'

# stdout should not contain a SearchResults JSON payload.
if [[ -s "$WORKDIR/search_stdout.log" ]] && grep -q '"type":"search_results"' "$WORKDIR/search_stdout.log"; then
    printf '%s search_results emitted despite missing credentials\n' "$E2E_FAIL"
    exit 1
fi
printf '%s no search_results event on failed auth\n' "$E2E_PASS"

echo
e2e_log "scenario passed"
