---
name: subagent-driven-development
description: Use when executing an implementation plan with independent tasks in the current session. Dispatches each task to a fresh implementer subagent, then runs spec review and code quality review. Native project agents are the default; superorchestrator is optional.
---

# Subagent-Driven Development

Execute a plan one task at a time. A fresh implementer subagent builds each task, then two read-only reviewers check it: spec compliance first, code quality second. Fixes go back to the same implementer. You (the controller) plan, brief, dispatch, review and decide; you do not edit source files for delegated work.

**Why subagents:** each gets exactly the brief it needs and none of your history. That keeps it focused and keeps your context for coordination.

**Continuous execution:** do not pause between tasks. Stop only for a BLOCKED status you cannot resolve, real ambiguity, or when all tasks are done.

## Before the first task

1. Have a plan: per task a goal, acceptance criteria (the commands that must pass) and out-of-scope items. Commit it (`docs/plans/`).
2. Work on a feature branch or worktree, never `main` without the user's consent.
3. Read the plan once. Extract every task with its full text and create the task list.

## Native process (default)

Use the Agent tool with the project agents in `.claude/agents/`:

| Role                                                               | `subagent_type`         |
| ------------------------------------------------------------------ | ----------------------- |
| Mechanical task: 1-2 files, complete spec, isolated functions      | `implementer-haiku`     |
| Integration or judgment task: multi-file, debugging, new patterns  | `implementer-sonnet`    |
| Spec compliance review (read-only)                                 | `spec-reviewer`         |
| Code quality review (read-only), and the final whole-branch review | `code-quality-reviewer` |

Per task:

1. **Dispatch the implementer** with the full task text, context (where it fits, files, dependencies) and the acceptance commands. Never make it read the plan file.
2. **Answer questions** it asks before it starts. Do not leave one unanswered.
3. **Read its report** (`DONE`, `DONE_WITH_CONCERNS`, `NEEDS_CONTEXT`, `BLOCKED`) and look at the diff yourself: `git diff BASE_SHA..HEAD_SHA`.
4. **Spec review.** Dispatch `spec-reviewer` with the task text and the git range. If not compliant, send the issues to the same implementer (SendMessage), then review again with a fresh reviewer.
5. **Code quality review**, only after spec compliance passes. Dispatch `code-quality-reviewer` with the range. Send Critical and Important issues back and review again. Decide on Minor ones yourself.
6. Mark the task complete and move on.

After all tasks: one final `code-quality-reviewer` over `git merge-base main HEAD..HEAD`, then `bash scripts/ci/all.sh`, then finish the branch (see the `finishing-a-development-branch` skill in the OtherSkill bank).

Rules for native subagents:

- Do not pass `model` or `effort`. The agent files pin `claude-haiku-5-5` and `claude-sonnet-5-5` at medium effort, and `.claude/hooks/enforce-subagent-models.py` denies other models, other effort levels and `general-purpose` subagents.
- Native subagents share the checkout. Run one implementer at a time, on the feature branch.
- If `implementer-haiku` is blocked on reasoning, re-dispatch to `implementer-sonnet`. If `implementer-sonnet` is blocked, split the task or ask the human.

## Handling implementer status

- **DONE:** go to spec review.
- **DONE_WITH_CONCERNS:** read the concerns. Fix correctness or scope concerns before review; note observations and proceed.
- **NEEDS_CONTEXT:** send the missing context and let it continue.
- **BLOCKED:** missing context, send it; needs more reasoning, re-dispatch to a stronger agent; too large, split; plan is wrong, escalate to the human. Never retry the same agent unchanged.

## Optional: superorchestrator

When the superorchestrator MCP is available and a non-Claude account has headroom, the same process can run through it: `spawn_agent(agent: "implementer", review: true, checks, wait: false)` per task, `get_run(diff: true)` for review, `reject_run` for fixes, `accept_run(merge: true)` to land. Load the `use-superorchestrator-mcp` skill first and call `list_harnesses`. Leave `harness` at `auto`; never pick the Claude harness or a Claude account for a subagent. If anything about it fails, use the native process above.

## Red flags

Never:

- start on `main` without consent, or skip either review, or start quality review before spec compliance passes
- move on with open review issues, or accept "close enough" on spec compliance
- run parallel implementers on tasks that share files or depend on each other
- fix a failed task yourself (it pollutes your context); send instructions to the implementer or re-dispatch

## Fotoko specifics

- Acceptance commands per task usually include `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test -p <crate>`, `pnpm exec turbo run lint typecheck test --filter=<pkg>` and `bash scripts/ci/fast.sh`.
- `docs/contracts.md` is the interface authority. If a task must change an interface, change the contract first.
- Build rules (own target dir per worktree, no `CARGO_TARGET_DIR`) are in `AGENTS.md`.
