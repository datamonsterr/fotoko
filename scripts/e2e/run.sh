#!/usr/bin/env bash
# `pnpm --filter @fotoko/e2e-web test:e2e`: start the simulated stack, run Playwright, stop the
# stack whatever happens. Extra arguments go to `playwright test`. On failure the stack logs are
# copied into packages/e2e-web/playwright-report/sim-logs.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
sim="${FOTOKO_E2E_DIR:-$here/.sim}"
export FOTOKO_E2E_DIR="$sim"

# shellcheck disable=SC2329  # invoked by the trap
cleanup() {
    bash "$here/sim-down.sh" || true
}
trap cleanup EXIT
trap 'exit 130' INT TERM

bash "$here/sim-up.sh"
# shellcheck source=/dev/null
. "$sim/env.sh"

cd "$root/packages/e2e-web"
status=0
pnpm exec playwright test "$@" || status=$?
if [ "$status" -ne 0 ]; then
    mkdir -p playwright-report/sim-logs
    cp -R "$sim/logs/." playwright-report/sim-logs/ 2>/dev/null || true
    echo "run: failed, report in packages/e2e-web/playwright-report" >&2
fi
exit "$status"
