#!/usr/bin/env bash
# Build the fotoko-api Lambda binaries for arm64 (provided.al2023) with cargo-lambda.
# Output: <target dir>/lambda/<bin>/bootstrap (the CDK default FOTOKO_LAMBDA_DIR).
# Bins that need a cargo feature (fotoko-local needs `local`) are not built.
# Usage: lambda-build.sh [--dry-run]
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

DRY_RUN=0
[ "${1:-}" != "--dry-run" ] || DRY_RUN=1

mapfile -t bins < <(python3 -I scripts/ci/cargo_meta.py lambda-bins)
if [ "${#bins[@]}" -eq 0 ] && [ "$DRY_RUN" -eq 1 ]; then
    bins=("<bin>")
elif [ "${#bins[@]}" -eq 0 ]; then
    echo "lambda-build: ERROR: no Lambda binaries found in fotoko-api" >&2
    exit 1
fi

args=(cargo lambda build --release --arm64 --locked -p fotoko-api --no-default-features)
for bin in "${bins[@]}"; do
    args+=(--bin "$bin")
done

if [ "$DRY_RUN" -eq 1 ]; then
    echo "+ ${args[*]}"
    exit 0
fi

command -v cargo-lambda >/dev/null 2>&1 || {
    echo "lambda-build: ERROR: cargo-lambda not found. Install it with: pip3 install cargo-lambda" >&2
    exit 1
}
"${args[@]}"
