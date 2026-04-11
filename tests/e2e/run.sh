#!/usr/bin/env bash
# e2e harness entry point. Runs every scenario in tests/e2e/scenarios/ in
# lexical order. Each scenario is self-contained and uses assertions from
# tests/e2e/lib/assertions.sh.

set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)

SCENARIOS_DIR="$HERE/scenarios"
if [[ ! -d "$SCENARIOS_DIR" ]]; then
    printf 'no scenarios dir at %s\n' "$SCENARIOS_DIR" >&2
    exit 1
fi

SCENARIOS=()
while IFS= read -r line; do
    SCENARIOS+=("$line")
done < <(find "$SCENARIOS_DIR" -name '*.sh' -type f | sort)

if [[ ${#SCENARIOS[@]} -eq 0 ]]; then
    printf 'no scenarios found under %s\n' "$SCENARIOS_DIR" >&2
    exit 1
fi

printf '[e2e] harness — %d scenario(s)\n' "${#SCENARIOS[@]}"
PASSED=0
FAILED=()

for scenario in "${SCENARIOS[@]}"; do
    name=$(basename "$scenario")
    printf '\n========== %s ==========\n' "$name"
    if bash "$scenario"; then
        PASSED=$((PASSED + 1))
    else
        FAILED+=("$name")
    fi
done

printf '\n[e2e] summary: %d passed, %d failed\n' "$PASSED" "${#FAILED[@]}"
if [[ ${#FAILED[@]} -gt 0 ]]; then
    printf '[e2e] failing scenarios:\n'
    for f in "${FAILED[@]}"; do
        printf '  - %s\n' "$f"
    done
    exit 1
fi
