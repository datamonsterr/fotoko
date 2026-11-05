#!/usr/bin/env bash
# Run one CI job's commands locally. CI calls this script for every job, so the commands here
# are exactly what runs on GitHub (job ids in .github/workflows/ci.yml match the names below).
#
#   bash scripts/ci/repro.sh <job> [arg]
#   bash scripts/ci/repro.sh list
#
# Jobs: rust-lint, rust-test [crate], rust-integration, rust-e2e, arch, deny, js [mobile|infra],
#       format, cdk-synth, web-e2e, pi-cross-build, lambda-build, shellcheck, actionlint
# Env: CI=1 switches to concise output (clippy --message-format short).
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

log() { echo "repro: $*" >&2; }
die() {
    echo "repro: ERROR: $*" >&2
    exit 1
}

skip() {
    if [ -n "${GITHUB_ACTIONS:-}" ]; then
        echo "::notice title=$JOB skipped::$*"
    fi
    log "SKIPPED $JOB: $*"
    exit 0
}

# need <binary> <install hint>
need() {
    command -v "$1" >/dev/null 2>&1 || die "$1 not found. $2"
}

have_crate() { [ -f "crates/$1/Cargo.toml" ]; }

nextest_ok() { cargo nextest --version >/dev/null 2>&1; }

rust_test_crate() {
    local crate="$1"
    if nextest_ok; then
        cargo nextest run -p "$crate" --all-features --profile ci --no-tests=pass
    else
        log "cargo-nextest not installed, using cargo test (cargo install cargo-nextest --locked)"
        cargo test -p "$crate" --all-features --lib --bins --tests
    fi
    if python3 -I scripts/ci/cargo_meta.py has-lib "$crate"; then
        cargo test -p "$crate" --all-features --doc
    fi
}

job_rust_lint() {
    local fmt=()
    [ -z "${CI:-}" ] || fmt=(--message-format short)
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features ${fmt[@]+"${fmt[@]}"} -- -D warnings
}

job_rust_test() {
    if [ $# -gt 0 ]; then
        rust_test_crate "$1"
        return
    fi
    local crate
    while IFS= read -r crate; do
        rust_test_crate "$crate"
    done < <(python3 -I scripts/ci/cargo_meta.py test-matrix | python3 -I -c '
import json, sys
for e in json.load(sys.stdin)["include"]:
    print(e["crate"])')
}

job_rust_integration() {
    have_crate api || skip "crates/api does not exist yet"
    [ -f docker-compose.test.yml ] || skip "docker-compose.test.yml does not exist yet"
    need docker "Install Docker with the compose plugin."
    # Global: the EXIT trap runs after this function has returned.
    compose=(docker compose -f docker-compose.test.yml)
    # shellcheck disable=SC2329 # invoked by the EXIT trap
    cleanup() {
        local code=$?
        if [ "$code" -ne 0 ] && [ -n "${CI:-}" ]; then
            "${compose[@]}" logs --no-color --tail 100 || true
        fi
        [ -n "${KEEP_SERVICES:-}" ] || "${compose[@]}" down -v --remove-orphans || true
    }
    trap cleanup EXIT
    "${compose[@]}" up -d --wait --wait-timeout 180
    # DynamoDB Local and MinIO accept any credentials; the SDK still needs some.
    export AWS_ACCESS_KEY_ID="${AWS_ACCESS_KEY_ID:-test}"
    export AWS_SECRET_ACCESS_KEY="${AWS_SECRET_ACCESS_KEY:-test}"
    export AWS_REGION="${AWS_REGION:-ap-southeast-1}"
    export FOTOKO_IT=1
    if nextest_ok; then
        cargo nextest run -p fotoko-api --all-features --profile ci --run-ignored all --no-tests=pass
    else
        log "cargo-nextest not installed, using cargo test (cargo install cargo-nextest --locked)"
        cargo test -p fotoko-api --all-features -- --include-ignored
    fi
}

job_rust_e2e() {
    have_crate e2e || skip "crates/e2e does not exist yet"
    if nextest_ok; then
        cargo nextest run -p fotoko-e2e --all-features --profile ci --no-tests=pass
    else
        log "cargo-nextest not installed, using cargo test (cargo install cargo-nextest --locked)"
        cargo test -p fotoko-e2e --all-features
    fi
}

job_arch() {
    bash scripts/ci/check-arch.sh
}

job_deny() {
    need cargo-deny "Install it with: cargo install cargo-deny --locked"
    cargo deny --all-features check
}

job_js() {
    local pkgs=("$@")
    [ ${#pkgs[@]} -gt 0 ] || pkgs=(mobile infra)
    local pkg
    for pkg in "${pkgs[@]}"; do
        pnpm exec turbo run lint typecheck test --filter="@fotoko/$pkg" --output-logs=errors-only
    done
}

job_format() {
    pnpm format:check
}

job_cdk_synth() {
    FOTOKO_LAMBDA_DIR=infra/cdk/test/fixtures/lambda pnpm --filter @fotoko/infra exec cdk synth --quiet
}

job_web_e2e() {
    [ -f packages/e2e-web/package.json ] || skip "packages/e2e-web does not exist yet"
    local deps=()
    [ -z "${CI:-}" ] || deps=(--with-deps)
    pnpm --filter ./packages/e2e-web exec playwright install ${deps[@]+"${deps[@]}"} chromium
    pnpm --filter ./packages/e2e-web run test:e2e
}

job_pi_cross_build() {
    local target=aarch64-unknown-linux-gnu
    need aarch64-linux-gnu-gcc "Install it with: sudo apt-get install gcc-aarch64-linux-gnu"
    export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
    export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
    export AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar
    cargo build --release --locked -p fotoko-pi-agent --target "$target"
    local dir bin
    dir="$(cargo metadata --format-version 1 --no-deps | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
    bin="$dir/$target/release/fotoko-pi-agent"
    [ -f "$bin" ] || die "expected binary $bin"
    if command -v file >/dev/null 2>&1; then file "$bin"; fi
}

job_lambda_build() {
    have_crate api || skip "crates/api does not exist yet"
    bash scripts/ci/lambda-build.sh
}

job_shellcheck() {
    need shellcheck "Install it from https://github.com/koalaman/shellcheck/releases or your package manager."
    git ls-files -z -- '*.sh' ':(exclude).otherskill' | xargs -0 -r shellcheck -x
}

job_actionlint() {
    if command -v actionlint >/dev/null 2>&1; then
        actionlint
    else
        need docker "Install actionlint (https://github.com/rhysd/actionlint/releases) or Docker."
        docker run --rm -v "$PWD:/repo:ro" -w /repo rhysd/actionlint:1.7.12
    fi
}

JOB="${1:-}"
[ -n "$JOB" ] || die "usage: repro.sh <job> [arg]  (repro.sh list)"
shift

case "$JOB" in
list)
    sed -n 's/^# Jobs: //p; s/^#       //p' "${BASH_SOURCE[0]}" | head -2
    ;;
rust-lint | rust-test | rust-integration | rust-e2e | arch | deny | js | format | cdk-synth | web-e2e | pi-cross-build | lambda-build | shellcheck | actionlint)
    log "$JOB ${*:-}"
    "job_${JOB//-/_}" "$@"
    log "$JOB ok"
    ;;
*)
    die "unknown job '$JOB' (repro.sh list)"
    ;;
esac
