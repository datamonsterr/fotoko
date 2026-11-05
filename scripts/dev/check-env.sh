#!/usr/bin/env bash
# Check that .env.example and the code agree: every variable the code reads is documented, and
# every documented variable is read somewhere (or is listed in EXTERNAL below).
#
#   bash scripts/dev/check-env.sh
#
# "Read" means one of these patterns in tracked files (docs, .otherskill and node_modules are
# skipped):
#   Rust   get(lookup, "X") / required(lookup, "X") / get("X") / env::var("X")
#   JS/TS  process.env.X, env.X (CDK), literal EXPO_PUBLIC_* names in apps/mobile/src
#   shell  ${X:-...} ${X:?...} ${X:=...}, `export X=` in scripts/e2e, and .mcp.json
#   CI     vars.X in .github/workflows
# Documented means a `NAME=` or `# NAME=` line in .env.example.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# Provided by the OS, the shell, GitHub runners or the CI scripts themselves: not configuration.
IGNORE_RE='^(CI|EXPO_PUBLIC|HOME|PATH|PWD|USER|LOGNAME|NODE_ENV|EXPO_OS|GITHUB_[A-Z_]+|JOB_START|JOB_STATUS|JUNIT|SSH_CONNECTION|XDG_CACHE_HOME|CLAUDE_PROJECT_DIR|CARGO_[A-Z_]+|RUSTUP_[A-Z_]+)$'
# Documented because other tools read them (not this repo's code).
EXTERNAL_RE='^(RUST_LOG|OPENROUTER_API_KEY|CHISLE_[A-Z_]+|AWS_ACCESS_KEY_ID|AWS_SECRET_ACCESS_KEY)$'

NAME='[A-Z][A-Z0-9_]+[A-Z0-9]'
SRC=(':!docs' ':!.otherskill' ':!.claude' ':!**/node_modules/**' ':!pnpm-lock.yaml' ':!scripts/dev/check-env.sh')

documented=$(grep -oE "^#? ?${NAME}=" .env.example | tr -d '# =' | sort -u)

reads=$(
    {
        git grep -hoE "(get|required|var)\(\s*(lookup,\s*)?\"${NAME}\"" -- 'crates/**/*.rs' |
            grep -oE "\"${NAME}\"" | tr -d '"'
        git grep -hoE "process\.env\.${NAME}" -- apps packages infra scripts "${SRC[@]}" | sed 's/process\.env\.//'
        git grep -hoE "\benv\.${NAME}" -- 'infra/cdk/lib/*.ts' | sed 's/env\.//'
        git grep -hoE "EXPO_PUBLIC_[A-Z][A-Z0-9_]*[A-Z0-9]" -- apps/mobile/src ':!**/*.test.*' | sort -u
        git grep -hoE "\\$\{${NAME}:[-?=]" -- scripts deploy docker .mcp.json "${SRC[@]}" | sed -E 's/^\$\{//; s/:[-?=]$//'
        git grep -hoE "^export ${NAME}=" -- scripts/e2e | sed 's/^export //; s/=$//'
        git grep -hoE "vars\.${NAME}" -- .github | sed 's/vars\.//'
    } | grep -Ev "$IGNORE_RE" | sort -u
)

status=0
missing=$(comm -13 <(echo "$documented") <(echo "$reads") || true)
if [ -n "$missing" ]; then
    echo "check-env: read by the code but missing from .env.example:" >&2
    while IFS= read -r v; do echo "  $v" >&2; done <<<"$missing"
    status=1
fi

# Documented names must appear in the code, apart from EXTERNAL ones.
stale=""
while IFS= read -r var; do
    [ -n "$var" ] || continue
    if grep -Eq "$EXTERNAL_RE" <<<"$var"; then continue; fi
    if ! grep -qx "$var" <<<"$reads"; then
        stale+="  $var"$'\n'
    fi
done <<<"$documented"
if [ -n "$stale" ]; then
    echo "check-env: in .env.example but never read:" >&2
    printf '%s' "$stale" >&2
    status=1
fi

if [ "$status" -eq 0 ]; then
    echo "check-env: ok ($(wc -l <<<"$documented" | tr -d ' ') documented, $(wc -l <<<"$reads" | tr -d ' ') read)"
fi
exit "$status"
