#!/usr/bin/env bash
# Everything CI runs: Rust fmt/clippy/tests, turbo lint/typecheck/test, Prettier.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

if [ -n "$(find crates -maxdepth 2 -name Cargo.toml 2>/dev/null | head -1)" ]; then
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    if cargo nextest --version >/dev/null 2>&1; then
        cargo nextest run --workspace --profile ci --no-tests=pass
    else
        cargo test --workspace
    fi
fi

if [ -n "$(find apps packages infra -maxdepth 2 -name package.json 2>/dev/null | head -1)" ]; then
    pnpm exec turbo run lint typecheck test
fi

pnpm run format:check
echo "all: ok"
