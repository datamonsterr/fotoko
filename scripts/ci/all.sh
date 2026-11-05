#!/usr/bin/env bash
# Everything CI runs: Rust fmt/clippy/tests/doctests/deny, turbo lint/typecheck/test, Prettier.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

if [ ! -d node_modules ]; then
    echo "all: ERROR: node_modules is missing, run 'pnpm install' first" >&2
    exit 1
fi

if [ -n "$(find crates -maxdepth 2 -name Cargo.toml 2>/dev/null | head -1)" ]; then
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    if cargo nextest --version >/dev/null 2>&1; then
        cargo nextest run --workspace --profile ci --no-tests=pass
    else
        cargo test --workspace --lib --bins --tests
    fi
    cargo test --workspace --doc
    if cargo deny --version >/dev/null 2>&1; then
        cargo deny check
    else
        echo "all: SKIPPED cargo deny check (cargo-deny not installed: cargo install cargo-deny)" >&2
    fi
fi

if [ -n "$(find apps packages infra -maxdepth 2 -name package.json 2>/dev/null | head -1)" ]; then
    pnpm exec turbo run lint typecheck test
fi

pnpm run format:check
echo "all: ok"
