#!/usr/bin/env bash
# Fast changed-scope loop: checks only what differs from origin/main.
# Falls back to HEAD~1, then to everything, when no base is available.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

BASE=""
if git rev-parse --verify --quiet origin/main >/dev/null; then
    BASE="$(git merge-base origin/main HEAD)"
elif git rev-parse --verify --quiet HEAD~1 >/dev/null; then
    BASE="HEAD~1"
fi

ALL=0
CHANGED=""
if [ -z "$BASE" ]; then
    echo "fast: no base commit, checking everything"
    ALL=1
else
    CHANGED="$(
        {
            git diff --name-only "$BASE" HEAD
            git diff --name-only HEAD
            git ls-files --others --exclude-standard
        } | sort -u
    )"
    echo "fast: base $BASE, $(printf '%s' "$CHANGED" | grep -c . || true) changed files"
fi

# Whether any changed path matches an extended regex.
changed_matches() {
    [ -n "$CHANGED" ] && printf '%s\n' "$CHANGED" | grep -Eq "$1"
}

# --- Rust ---
RUST_CRATES=()
RUST_ALL=$ALL
if [ "$ALL" -eq 0 ] && changed_matches '^(Cargo\.(toml|lock)|rust-toolchain\.toml|rustfmt\.toml|clippy\.toml|deny\.toml|\.config/nextest\.toml)$'; then
    RUST_ALL=1
fi
if [ "$RUST_ALL" -eq 0 ] && [ -n "$CHANGED" ]; then
    while IFS= read -r dir; do
        [ -n "$dir" ] && [ -f "$dir/Cargo.toml" ] || continue
        name="$(sed -n 's/^name *= *"\(.*\)"/\1/p' "$dir/Cargo.toml" | head -1)"
        [ -n "$name" ] && RUST_CRATES+=("$name")
    done < <(printf '%s\n' "$CHANGED" | sed -n 's#^\(crates/[^/]*\)/.*#\1#p' | sort -u)
fi

has_crates() { [ -n "$(find crates -maxdepth 2 -name Cargo.toml 2>/dev/null | head -1)" ]; }

if has_crates && { [ "$RUST_ALL" -eq 1 ] || [ "${#RUST_CRATES[@]}" -gt 0 ]; }; then
    PKG_ARGS=(--workspace)
    if [ "$RUST_ALL" -eq 0 ]; then
        PKG_ARGS=()
        for c in "${RUST_CRATES[@]}"; do PKG_ARGS+=(-p "$c"); done
    fi
    echo "fast: rust ${PKG_ARGS[*]}"
    cargo fmt --all --check
    cargo clippy "${PKG_ARGS[@]}" --all-targets -- -D warnings
    if cargo nextest --version >/dev/null 2>&1; then
        cargo nextest run "${PKG_ARGS[@]}" --no-tests=pass
    else
        cargo test "${PKG_ARGS[@]}"
    fi
else
    echo "fast: no rust changes"
fi

# --- pnpm / turbo ---
if [ -d node_modules ] && [ -n "$(find apps packages infra -maxdepth 2 -name package.json 2>/dev/null | head -1)" ]; then
    if [ "$ALL" -eq 1 ]; then
        FILTER=()
    else
        FILTER=("--filter=...[${BASE}]")
    fi
    echo "fast: turbo ${FILTER[*]:-all}"
    pnpm exec turbo run lint typecheck test "${FILTER[@]}"
else
    echo "fast: no pnpm packages to check"
fi

# --- formatting of changed files ---
if [ -d node_modules ]; then
    if [ "$ALL" -eq 1 ]; then
        pnpm exec prettier --check .
    elif [ -n "$CHANGED" ]; then
        FILES=()
        while IFS= read -r f; do
            [ -f "$f" ] && FILES+=("$f")
        done < <(printf '%s\n' "$CHANGED" | grep -E '\.(js|jsx|cjs|mjs|ts|tsx|json|md|mdx|yml|yaml)$' || true)
        if [ "${#FILES[@]}" -gt 0 ]; then
            pnpm exec prettier --check --ignore-unknown "${FILES[@]}"
        fi
    fi
fi

echo "fast: ok"
