#!/usr/bin/env bash
# PreToolUse(Bash) hook: rewrite shell commands to their `rtk` equivalent (Rust Token Killer,
# https://github.com/rtk-ai/rtk) so noisy output is filtered before it reaches the context.
#
# Adapted from rtk's own hook (hooks/claude/rtk-rewrite.sh, Apache-2.0, rtk 0.49.0). All rewrite
# and permission logic stays in `rtk rewrite`; this wrapper only adds the project rules:
#   - no-op (exit 0, no output) when rtk or jq is not installed, so a fresh clone works;
#   - commands that must show exact output are never rewritten (EXCLUDE below).
#
# `rtk rewrite` exit codes: 0 rewrite + an allow rule matched (auto-allow), 1 no rtk equivalent,
# 2 deny rule matched, 3 rewrite but ask (leave the permission decision to Claude Code).
# Install: docs/setup/local-dev.md ("Agent tooling").
set -u

command -v jq >/dev/null 2>&1 || exit 0
command -v rtk >/dev/null 2>&1 || exit 0

INPUT=$(cat)
CMD=$(jq -r '.tool_input.command // empty' <<<"$INPUT" 2>/dev/null) || exit 0
[ -n "$CMD" ] || exit 0

# Verbatim output wanted: CI/check scripts (failure summaries and the repro command), deploy and
# provisioning scripts, the rtk CLI itself, and git commit/push (hook failures from lefthook).
EXCLUDE='(^|[[:space:];&|(])(rtk[[:space:]]|git[[:space:]]+(-[^[:space:]]+[[:space:]]+)*(commit|push)([[:space:]]|$))|scripts/(ci|dev|deploy|e2e)/|deploy/pi/'
if [[ $CMD =~ $EXCLUDE ]]; then
    exit 0
fi

# RTK_REWRITE_HOST relaxes rtk's approval gate; never let an inherited value reach it.
REWRITTEN=$(env -u RTK_REWRITE_HOST rtk rewrite "$CMD" 2>/dev/null)
CODE=$?

case $CODE in
0)
    [ "$CMD" = "$REWRITTEN" ] && exit 0
    jq -c --arg cmd "$REWRITTEN" \
        '.tool_input.command = $cmd | {
          hookSpecificOutput: {
            hookEventName: "PreToolUse",
            permissionDecision: "allow",
            permissionDecisionReason: "RTK auto-rewrite",
            updatedInput: .tool_input
          }
        }' <<<"$INPUT"
    ;;
3)
    [ -n "$REWRITTEN" ] || exit 0
    jq -c --arg cmd "$REWRITTEN" \
        '.tool_input.command = $cmd | {
          hookSpecificOutput: {
            hookEventName: "PreToolUse",
            updatedInput: .tool_input
          }
        }' <<<"$INPUT"
    ;;
*)
    exit 0
    ;;
esac
