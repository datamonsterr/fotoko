#!/usr/bin/env bash
# Fast changed-scope loop: checks what differs from origin/main.
# Clippy always covers the whole workspace; tests cover changed crates plus every workspace
# crate that depends on them (transitively). With no merge base, everything is checked.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

BASE="$(git merge-base origin/main HEAD 2>/dev/null)" || BASE=""

ALL=0
CHANGED=""
if [ -z "$BASE" ]; then
    echo "fast: WARNING: no merge base with origin/main, checking everything" >&2
    ALL=1
else
    CHANGED="$(
        {
            git diff --name-only "$BASE" HEAD
            git diff --name-only HEAD
            git ls-files --others --exclude-standard
        } | sort -u
    )"
    echo "fast: base $BASE, $(grep -c . <<<"$CHANGED" || true) changed files"
fi

# Whether any changed path matches an extended regex.
changed_matches() {
    [ -n "$CHANGED" ] && grep -Eq "$1" <<<"$CHANGED"
}

has_crates() { [ -n "$(find crates -maxdepth 2 -name Cargo.toml 2>/dev/null | head -1)" ]; }
has_js() { [ -n "$(find apps packages infra -maxdepth 2 -name package.json 2>/dev/null | head -1)" ]; }

# Crate names to test: changed crates + transitive workspace reverse dependencies.
selected_crates() {
    cargo metadata --format-version 1 --no-deps | python3 -c '
import json, os, sys
meta = json.load(sys.stdin)
changed = [line for line in sys.argv[1].splitlines() if line]
root = meta["workspace_root"]
pkgs = {p["name"]: p for p in meta["packages"]}
dirs = {n: os.path.relpath(os.path.dirname(p["manifest_path"]), root) for n, p in pkgs.items()}
selected = {n for n, d in dirs.items() if any(f == d or f.startswith(d + "/") for f in changed)}
rdeps = {n: set() for n in pkgs}
for n, p in pkgs.items():
    for dep in p["dependencies"]:
        if dep["name"] in pkgs:
            rdeps[dep["name"]].add(n)
queue = list(selected)
while queue:
    for r in rdeps[queue.pop()]:
        if r not in selected:
            selected.add(r)
            queue.append(r)
print("\n".join(sorted(selected)))
' "$CHANGED"
}

# --- Rust ---
RUST_ALL=$ALL
if [ "$RUST_ALL" -eq 0 ] && changed_matches '^(Cargo\.(toml|lock)|rust-toolchain\.toml|rustfmt\.toml|clippy\.toml|deny\.toml|\.config/|\.cargo/)'; then
    RUST_ALL=1
fi

if has_crates; then
    PKG_ARGS=(--workspace)
    RUN_TESTS=1
    if [ "$RUST_ALL" -eq 0 ]; then
        PKG_ARGS=()
        while IFS= read -r name; do
            [ -n "$name" ] && PKG_ARGS+=(-p "$name")
        done < <(selected_crates)
        [ "${#PKG_ARGS[@]}" -gt 0 ] || RUN_TESTS=0
    fi
    echo "fast: rust fmt + clippy (workspace), tests: ${PKG_ARGS[*]:-none}"
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    if [ "$RUN_TESTS" -eq 1 ]; then
        if cargo nextest --version >/dev/null 2>&1; then
            cargo nextest run "${PKG_ARGS[@]}" --no-tests=pass
        else
            cargo test "${PKG_ARGS[@]}" --lib --bins --tests
        fi
        cargo test "${PKG_ARGS[@]}" --doc
    fi
else
    echo "fast: no crates"
fi

# --- pnpm / turbo / Prettier ---
if [ ! -d node_modules ]; then
    echo "fast: ERROR: node_modules is missing, run 'pnpm install' first" >&2
    exit 1
fi

if has_js; then
    if [ "$ALL" -eq 1 ]; then
        FILTER=()
    else
        FILTER=("--filter=...[${BASE}]")
    fi
    echo "fast: turbo ${FILTER[*]:-all}"
    pnpm exec turbo run lint typecheck test "${FILTER[@]}"
else
    echo "fast: no pnpm packages"
fi

if [ "$ALL" -eq 1 ]; then
    pnpm exec prettier --check .
else
    FILES=()
    while IFS= read -r f; do
        [ -f "$f" ] && FILES+=("$f")
    done < <(grep -E '\.(js|jsx|cjs|mjs|ts|tsx|json|md|mdx|yml|yaml)$' <<<"$CHANGED" || true)
    if [ "${#FILES[@]}" -gt 0 ]; then
        pnpm exec prettier --check --ignore-unknown -- "${FILES[@]}"
    fi
fi

echo "fast: ok"
