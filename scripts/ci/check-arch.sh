#!/usr/bin/env bash
# Enforce the crate dependency rules from AGENTS.md ("Architecture rules"). Reads only
# `cargo metadata` and `cargo tree`, builds nothing. Rules and allowlists: scripts/ci/check_arch.py.
#
#   bash scripts/ci/check-arch.sh        (CI: bash scripts/ci/repro.sh arch)
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
exec python3 -I scripts/ci/check_arch.py "$@"
