#!/usr/bin/env bash
# Everything CI runs, locally: the same commands as the CI jobs (scripts/ci/repro.sh), so a pass
# here predicts the `ci-ok` check. Jobs whose tool is missing are skipped with a notice.
#
#   bash scripts/ci/all.sh
#
# Opt-in extras: FOTOKO_IT=1 runs rust-integration (needs Docker), FOTOKO_WEB_E2E=1 runs web-e2e
# (needs Playwright browsers). pi-cross-build and lambda-build run when their toolchains exist.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

if [ ! -d node_modules ]; then
    echo "all: ERROR: node_modules is missing, run 'pnpm install' first" >&2
    exit 1
fi

repro() { bash scripts/ci/repro.sh "$@"; }
skipped=()
skip() {
    echo "all: SKIPPED $1 ($2)" >&2
    skipped+=("$1")
}
has() { command -v "$1" >/dev/null 2>&1; }

repro rust-lint
repro rust-test
repro rust-e2e # skips itself until crates/e2e exists

if cargo deny --version >/dev/null 2>&1; then
    repro deny
else
    skip deny "cargo-deny not installed: cargo install cargo-deny --locked"
fi

if [ "${FOTOKO_IT:-}" = "1" ] && has docker; then
    repro rust-integration
else
    skip rust-integration "set FOTOKO_IT=1 and install Docker"
fi

repro js
repro format
repro cdk-synth

if [ "${FOTOKO_WEB_E2E:-}" = "1" ]; then
    repro web-e2e
else
    skip web-e2e "set FOTOKO_WEB_E2E=1"
fi

if has aarch64-linux-gnu-gcc; then
    repro pi-cross-build
else
    skip pi-cross-build "aarch64-linux-gnu-gcc not installed"
fi

if has cargo-lambda; then
    repro lambda-build
else
    skip lambda-build "cargo-lambda not installed: pip3 install cargo-lambda"
fi

if has shellcheck; then
    repro shellcheck
else
    skip shellcheck "shellcheck not installed"
fi

if has actionlint; then
    repro actionlint
else
    skip actionlint "actionlint not installed"
fi

if [ "${#skipped[@]}" -gt 0 ]; then
    echo "all: ok (skipped: ${skipped[*]})"
else
    echo "all: ok"
fi
