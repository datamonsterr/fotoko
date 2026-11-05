#!/usr/bin/env bash
# Stop everything sim-up.sh started. Safe to run twice. Logs stay in $FOTOKO_E2E_DIR/logs.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sim="${FOTOKO_E2E_DIR:-$here/.sim}"

# Whether `pid` is still one of our processes (pids are reused); without /proc, trust the file.
ours() {
    local pid="$1"
    kill -0 "$pid" 2>/dev/null || return 1
    [ -r "/proc/$pid/cmdline" ] || return 0
    tr '\0' ' ' <"/proc/$pid/cmdline" | grep -Eq 'fotoko-|gateway\.mjs'
}

shopt -s nullglob
pidfiles=("$sim"/pids/*.pid)
for file in "${pidfiles[@]}"; do
    pid="$(cat "$file")"
    if ours "$pid"; then
        kill "$pid" 2>/dev/null || true
    fi
done
# Give them a moment to leave (the Pi agent announces `offline` first), then force.
for _ in $(seq 1 50); do
    alive=0
    for file in "${pidfiles[@]}"; do
        if ours "$(cat "$file")"; then alive=1; fi
    done
    [ "$alive" -eq 0 ] && break
    sleep 0.1
done
for file in "${pidfiles[@]}"; do
    pid="$(cat "$file")"
    if ours "$pid"; then
        kill -9 "$pid" 2>/dev/null || true
    fi
    rm -f "$file"
done
rm -f "$sim/env.sh"
echo "sim-down: stopped"
