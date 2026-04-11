#!/usr/bin/env bash
# Slice 2 boot scenario: boot clitunes with no arguments, let it render for
# 3 seconds, verify the ANSI truecolor stream and tracing logs look healthy.
#
# Renamed from its slice-1 form (wgpu→Kitty) after the CPU CellGrid+AnsiWriter
# rewrite. Kept as 01 because it's still the "does the thing boot and render
# at all" smoketest — no station, no audio source, just the calibration tone
# behind the first-run picker.

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
trap 'rm -rf "$WORKDIR"' EXIT
export E2E_STDOUT="$WORKDIR/stdout.bin"
export E2E_STDERR="$WORKDIR/stderr.log"

export CLITUNES_LOG_FORMAT=json
export RUST_LOG=clitunes=info,clitunes_engine=info
# Force a fresh state dir so we hit the first-run picker path rather than
# resuming a pick from the developer's real ~/.config/clitunes/state.toml.
export XDG_CONFIG_HOME="$WORKDIR/xdg"
mkdir -p "$XDG_CONFIG_HOME"

e2e_log "scenario: first-run boot → calibration tone → ANSI CellGrid"
e2e_log "binary: $BIN"
e2e_log "workdir: $WORKDIR"

run_for 3 "$BIN"

# Exit code: graceful SIGINT shutdown should return 0. (130 is also acceptable
# if the process was still in its sleep when the signal arrived.)
if [[ "${E2E_EXIT_CODE:-1}" -ne 0 && "${E2E_EXIT_CODE:-1}" -ne 130 ]]; then
    printf '%s unexpected exit code: %s\n' "$E2E_FAIL" "${E2E_EXIT_CODE:-1}"
    exit 1
fi
printf '%s exit code %s accepted\n' "$E2E_PASS" "${E2E_EXIT_CODE:-0}"

assert_file_nonempty "$E2E_STDOUT"
assert_stdout_ansi_truecolor "$E2E_STDOUT"

assert_log_contains "$E2E_STDERR" 'tracing initialised' 'tracing subscriber online'
assert_log_contains "$E2E_STDERR" 'boot: source . visualiser carousel . ansi' 'boot event fired'
assert_log_contains "$E2E_STDERR" 'stdin is not a tty' 'non-interactive mode detected'
assert_log_contains "$E2E_STDERR" 'shutdown' 'clean shutdown'

echo
e2e_log "scenario passed"
