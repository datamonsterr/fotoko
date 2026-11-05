#!/usr/bin/env python3
"""PreToolUse guard: subagents in this project use only Haiku 5.5 or Sonnet 5.5 at medium effort.

Fails closed: a payload it cannot parse, or non-string model/effort/subagent_type, is denied.
Self-check: python3 .claude/hooks/test_enforce_subagent_models.py
"""
import json
import sys

ALLOWED_MODELS = {"haiku", "sonnet", "claude-haiku-5-5", "claude-sonnet-5-5"}
ALLOWED_EFFORT = {"medium"}
# Built-in agents that can edit code with the session model. Use the project agents instead.
BLOCKED_TYPES = {"general-purpose"}


def decide(raw: str):
    """Return a denial reason, or None to allow."""
    try:
        payload = json.loads(raw)
    except ValueError:
        return "Hook payload is not valid JSON; denying subagent call."
    if not isinstance(payload, dict):
        return "Hook payload is not a JSON object; denying subagent call."
    tool_input = payload.get("tool_input")
    if tool_input is None:
        tool_input = {}
    if not isinstance(tool_input, dict):
        return "Hook tool_input is not an object; denying subagent call."

    model = tool_input.get("model")
    effort = tool_input.get("effort")
    agent_type = tool_input.get("subagent_type") or "general-purpose"
    for name, value in (("model", model), ("effort", effort), ("subagent_type", agent_type)):
        if value is not None and not isinstance(value, str):
            return f"Parameter '{name}' must be a string; denying subagent call."

    if model is not None and model not in ALLOWED_MODELS:
        return (f"Model '{model}' is not allowed in this project. Use implementer-haiku or "
                "implementer-sonnet, which pin claude-haiku-5-5 / claude-sonnet-5-5 at medium "
                "effort, and omit the model parameter.")
    if effort is not None and effort not in ALLOWED_EFFORT:
        return (f"Effort '{effort}' is not allowed in this project. Subagents run at medium "
                "effort; omit the effort parameter.")
    if agent_type in BLOCKED_TYPES:
        return ("general-purpose subagents are disabled in this project. Use implementer-haiku, "
                "implementer-sonnet, spec-reviewer or code-quality-reviewer "
                "(subagent-driven-development skill), or Explore for read-only search.")
    return None


def main() -> None:
    try:
        reason = decide(sys.stdin.read())
    except BaseException as exc:  # fail closed on anything, including RecursionError
        reason = f"Hook failed ({type(exc).__name__}); denying subagent call."
    if reason is not None:
        print(json.dumps({"hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }}))
    sys.exit(0)


if __name__ == "__main__":
    main()
