#!/usr/bin/env bash
# Simulates the rtk hook with sample PreToolUse payloads and a stub `rtk`.
#   bash .claude/hooks/test-rtk-rewrite.sh
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
hook="$here/rtk-rewrite.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
fail=0

payload() { jq -nc --arg c "$1" '{tool_name:"Bash",tool_input:{command:$c}}'; }

check() { # check <name> <expected> <actual>
    if [ "$2" = "$3" ]; then
        echo "ok   $1"
    else
        echo "FAIL $1: expected [$2] got [$3]" >&2
        fail=1
    fi
}

# Stub rtk: rewrites `cargo ...` and `git status` (exit 3, ask), `ls` (exit 0, allow), else exit 1.
mkdir -p "$tmp/bin"
cat >"$tmp/bin/rtk" <<'STUB'
#!/usr/bin/env bash
[ "$1" = rewrite ] || exit 2
case "$2" in
cargo\ *|git\ status*) printf 'rtk %s' "$2"; exit 3 ;;
ls*) printf 'rtk %s' "$2"; exit 0 ;;
*) exit 1 ;;
esac
STUB
chmod +x "$tmp/bin/rtk"

# Directory with only the tools the hook needs (jq, env, bash), no rtk.
mkdir -p "$tmp/nortk"
for t in jq env bash cat; do ln -s "$(command -v "$t")" "$tmp/nortk/$t"; done

run() { PATH="$tmp/bin:$PATH" bash "$hook" <<<"$(payload "$1")"; }

# 1. rtk absent: silent no-op.
out=$(PATH="$tmp/nortk" "$tmp/nortk/bash" "$hook" <<<"$(payload 'git status')" 2>&1; echo "rc=$?")
check "rtk missing is a no-op" "rc=0" "$out"

# 2. ask rewrite: updatedInput only, no permissionDecision.
out=$(run 'cargo test -p fotoko-core')
check "ask rewrite command" "rtk cargo test -p fotoko-core" "$(jq -r .hookSpecificOutput.updatedInput.command <<<"$out")"
check "ask rewrite has no permissionDecision" "null" "$(jq -r .hookSpecificOutput.permissionDecision <<<"$out")"

# 3. allow rewrite.
out=$(run 'ls -la')
check "allow rewrite" "allow" "$(jq -r .hookSpecificOutput.permissionDecision <<<"$out")"

# 4. no equivalent: no output.
check "unknown command untouched" "" "$(run 'echo hi')"

# 5. exclusions: never rewritten even though the stub would.
for c in 'bash scripts/ci/fast.sh' 'cargo test && bash scripts/ci/all.sh' 'bash scripts/deploy/aws.sh --dry-run all dev' \
    'bash deploy/pi/deploy.sh 10.0.0.2' 'git commit -m x' 'git -C . push origin HEAD' 'rtk gain'; do
    check "excluded: $c" "" "$(run "$c")"
done

# 6. non-excluded git still rewritten.
out=$(run 'git status')
check "git status rewritten" "rtk git status" "$(jq -r .hookSpecificOutput.updatedInput.command <<<"$out")"

# 7. malformed or empty payloads.
check "empty command" "" "$(PATH="$tmp/bin:$PATH" bash "$hook" <<<'{"tool_input":{}}')"
check "garbage stdin" "" "$(PATH="$tmp/bin:$PATH" bash "$hook" <<<'not json')"

exit "$fail"
