#!/usr/bin/env bash
# e2e test assertion helpers. Sourced by scenario scripts in ../scenarios/.
# Each helper prints a clear PASS/FAIL line to stdout and returns non-zero on
# failure so `set -e` callers can abort the scenario.

set -u

E2E_PASS=$'\033[32m✓ PASS\033[0m'
E2E_FAIL=$'\033[31m✗ FAIL\033[0m'

e2e_log() {
    printf '[e2e] %s\n' "$*"
}

# assert_file_exists <path>
assert_file_exists() {
    local path="$1"
    if [[ -e "$path" ]]; then
        printf '%s file exists: %s\n' "$E2E_PASS" "$path"
    else
        printf '%s file missing: %s\n' "$E2E_FAIL" "$path"
        return 1
    fi
}

# assert_file_nonempty <path>
assert_file_nonempty() {
    local path="$1"
    if [[ -s "$path" ]]; then
        local bytes
        bytes=$(wc -c < "$path" | tr -d ' ')
        printf '%s file non-empty (%s bytes): %s\n' "$E2E_PASS" "$bytes" "$path"
    else
        printf '%s file empty or missing: %s\n' "$E2E_FAIL" "$path"
        return 1
    fi
}

# assert_log_contains <logfile> <regex> <description>
assert_log_contains() {
    local logfile="$1"
    local regex="$2"
    local desc="$3"
    if grep -qE "$regex" "$logfile"; then
        printf '%s log matches /%s/ — %s\n' "$E2E_PASS" "$regex" "$desc"
    else
        printf '%s log missing /%s/ — %s\n' "$E2E_FAIL" "$regex" "$desc"
        printf '  last 20 lines of %s:\n' "$logfile"
        tail -n 20 "$logfile" | sed 's/^/    /'
        return 1
    fi
}

# assert_json_log_field <logfile> <jq_expr> <expected> <description>
# Requires CLITUNES_LOG_FORMAT=json.
assert_json_log_field() {
    local logfile="$1"
    local jq_expr="$2"
    local expected="$3"
    local desc="$4"
    local actual
    actual=$(jq -rs "map(select(.${jq_expr} != null)) | last | .${jq_expr}" < "$logfile" 2>/dev/null || true)
    if [[ "$actual" == "$expected" ]]; then
        printf '%s json field %s == %s — %s\n' "$E2E_PASS" "$jq_expr" "$expected" "$desc"
    else
        printf '%s json field %s: expected %s, got %s — %s\n' "$E2E_FAIL" "$jq_expr" "$expected" "$actual" "$desc"
        return 1
    fi
}

# assert_stdout_ansi_truecolor <stdout_file>
# Verifies the stdout stream contains ANSI CSI SGR truecolor sequences
# (ESC [ 38;2;R;G;B m or ESC [ 48;2;R;G;B m). This is the post-slice-2
# CellGrid+AnsiWriter output signature; the slice-1 Kitty APC path is gone.
assert_stdout_ansi_truecolor() {
    local path="$1"
    # ESC is 0x1b. The AnsiWriter emits combined fg+bg SGR sequences
    # (ESC [ 38;2;R;G;B;48;2;R;G;B m), so we can't require a trailing `m` —
    # just match the truecolor introducer ESC [ 38;2;<n> (or 48;2).
    if head -c 65536 "$path" | LC_ALL=C grep -aqE $'\x1b\\[[34]8;2;[0-9]+'; then
        printf '%s stdout contains ANSI truecolor SGR sequences\n' "$E2E_PASS"
    else
        printf '%s stdout has no ANSI truecolor SGR sequences\n' "$E2E_FAIL"
        return 1
    fi
}

# assert_json_status <jsonfile> <jq_expr> <expected> <description>
# Reads a JSON file and asserts the value at the given jq path equals expected.
assert_json_status() {
    local jsonfile="$1"
    local jq_expr="$2"
    local expected="$3"
    local desc="$4"
    local actual
    actual=$(jq -r "$jq_expr" < "$jsonfile" 2>/dev/null || echo "__jq_error__")
    if [[ "$actual" == "$expected" ]]; then
        printf '%s json %s == %s — %s\n' "$E2E_PASS" "$jq_expr" "$expected" "$desc"
    else
        printf '%s json %s: expected %s, got %s — %s\n' "$E2E_FAIL" "$jq_expr" "$expected" "$actual" "$desc"
        return 1
    fi
}

# wait_for_log <logfile> <regex> <timeout_seconds>
# Polls the logfile until the regex matches or the timeout expires.
wait_for_log() {
    local logfile="$1"
    local regex="$2"
    local timeout="${3:-10}"
    local elapsed=0
    e2e_log "waiting up to ${timeout}s for /${regex}/ in $(basename "$logfile")"
    while (( elapsed < timeout )); do
        if [[ -f "$logfile" ]] && grep -qE "$regex" "$logfile" 2>/dev/null; then
            e2e_log "found /${regex}/ after ~${elapsed}s"
            return 0
        fi
        sleep 0.5
        elapsed=$((elapsed + 1))
    done
    printf '%s timed out waiting for /%s/ in %s after %ds\n' "$E2E_FAIL" "$regex" "$logfile" "$timeout"
    return 1
}

# generate_wav_fixture <output_path> <duration_secs> <freq_hz>
# Generates a PCM WAV file using python3 (stdlib only) or sox as fallback.
generate_wav_fixture() {
    local output="$1"
    local duration="${2:-2}"
    local freq="${3:-440}"
    e2e_log "generating ${duration}s ${freq}Hz WAV fixture at $output"

    if command -v python3 &>/dev/null; then
        python3 -c "
import struct, math, wave
sr = 44100
n = int(sr * $duration)
with wave.open('$output', 'w') as w:
    w.setnchannels(1)
    w.setsampwidth(2)
    w.setframerate(sr)
    for i in range(n):
        s = int(32767 * math.sin(2 * math.pi * $freq * i / sr))
        w.writeframes(struct.pack('<h', s))
"
    elif command -v sox &>/dev/null; then
        sox -n -r 44100 -c 1 -b 16 "$output" synth "$duration" sine "$freq"
    else
        printf '%s neither python3 nor sox available to generate WAV fixture\n' "$E2E_FAIL"
        return 1
    fi

    if [[ -s "$output" ]]; then
        local bytes
        bytes=$(wc -c < "$output" | tr -d ' ')
        e2e_log "WAV fixture generated ($bytes bytes)"
    else
        printf '%s WAV fixture generation failed\n' "$E2E_FAIL"
        return 1
    fi
}

# run_for <seconds> <cmd...>
# Launches the command in the background, waits <seconds>, sends SIGINT,
# waits for graceful exit. Captures stdout to $E2E_STDOUT and stderr to
# $E2E_STDERR (set by the caller before invocation). Stores the exit code in
# $E2E_EXIT_CODE.
run_for() {
    local seconds="$1"
    shift
    : "${E2E_STDOUT:?E2E_STDOUT must be set}"
    : "${E2E_STDERR:?E2E_STDERR must be set}"
    e2e_log "running for ${seconds}s: $*"
    "$@" > "$E2E_STDOUT" 2> "$E2E_STDERR" &
    local pid=$!
    sleep "$seconds"
    kill -INT "$pid" 2>/dev/null || true
    local waited=0
    while kill -0 "$pid" 2>/dev/null; do
        sleep 0.1
        waited=$((waited + 1))
        if (( waited > 30 )); then
            e2e_log "process did not exit after SIGINT, sending SIGKILL"
            kill -KILL "$pid" 2>/dev/null || true
            break
        fi
    done
    wait "$pid" 2>/dev/null
    E2E_EXIT_CODE=$?
    e2e_log "process exited with code ${E2E_EXIT_CODE}"
}
