#!/usr/bin/env bash
# Job summary for CI: summary.sh <title> <repro command> [detail...]
# Env: JOB_STATUS (${{ job.status }}), JOB_START (epoch seconds), JUNIT (nextest report path).
# Appends Markdown to $GITHUB_STEP_SUMMARY (stdout when unset). On failure it lists the
# failing tests and the exact local command that reproduces the job.
set -euo pipefail

title="${1:?title required}"
repro="${2:?repro command required}"
shift 2

status="${JOB_STATUS:-unknown}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

{
    echo "### ${title}: ${status}"
    echo
    if [ -n "${JOB_START:-}" ]; then
        echo "- duration: $(($(date +%s) - JOB_START))s"
    fi
    for detail in "$@"; do
        echo "- ${detail}"
    done
    if [ "$status" != "success" ]; then
        echo "- reproduce locally: \`${repro}\`"
        echo
        if [ -n "${JUNIT:-}" ] && [ -f "$JUNIT" ]; then
            python3 -I "$here/junit_summary.py" "$JUNIT"
        fi
    fi
    echo
} >>"${GITHUB_STEP_SUMMARY:-/dev/stdout}"
