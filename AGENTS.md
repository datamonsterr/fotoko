# Fotoko

On-site pay-to-print photo kiosk. Customers scan a QR code at a Stand, upload photos in an Expo app, pay with payOS, and a Raspberry Pi prints them through CUPS + Gutenprint. Read `CONTEXT.md` (terms), `docs/contracts.md` (authoritative interfaces) and `docs/adr/` (decisions) first. Use the glossary terms exactly.

## Layout

| Path                                                   | What                                                   |
| ------------------------------------------------------ | ------------------------------------------------------ |
| `crates/protocol`                                      | HTTP + MQTT DTOs, topics, limits (`fotoko-protocol`)   |
| `crates/core`, `api`, `pi-agent`, `printer-sim`, `e2e` | Domain, Lambdas, Pi agent, printer simulation, sim e2e |
| `apps/mobile`                                          | Expo app (iOS, Android, web)                           |
| `infra/cdk`                                            | AWS CDK app                                            |
| `packages/e2e-web`                                     | Playwright web e2e                                     |
| `deploy/pi`                                            | Pi provisioning and cross-build                        |
| `scripts/ci`                                           | `fast.sh` and `all.sh`                                 |

Cargo members are `crates/*`; pnpm packages are `apps/*`, `packages/*`, `infra/*`. A new crate or package is picked up by the globs with no root edit, except that a crate other crates depend on needs a `path` entry in the root `[workspace.dependencies]`.

## Commands

```sh
pnpm install                      # also installs lefthook git hooks
bash scripts/ci/fast.sh           # changed scope: workspace clippy, tests of changed crates + their reverse deps, turbo, Prettier
bash scripts/ci/all.sh            # everything CI runs (fmt, clippy, tests, doctests, cargo deny, turbo, Prettier)
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p <crate>             # or `cargo nextest run -p <crate>` when installed
pnpm format:check                 # Prettier
pnpm exec turbo run lint typecheck test --filter=<pkg>
```

Pre-commit checks formatting of staged files; commit-msg enforces Conventional Commits; pre-push runs `fast.sh`.

## Workflow

- Plans live in `docs/plans/` with goal, acceptance commands and out-of-scope per task.
- Implement through the `subagent-driven-development` skill. Native project agents (`.claude/agents/`) are the default: `implementer-haiku` (mechanical), `implementer-sonnet` (judgment), `spec-reviewer`, `code-quality-reviewer`. Do not pass `model` or `effort`; `.claude/hooks/enforce-subagent-models.py` denies others and `general-purpose`. Superorchestrator is optional.
- Work on a branch or worktree, never on `main`. Conventional Commits.
- Check current docs before coding against any library, framework, SDK or CLI (Context7 MCP, AWS documentation MCP, vendor docs). Never write an API from memory. Pin versions.
- Change `docs/contracts.md` first when an interface changes.
- Skills: see `docs/skills.md`. The OtherSkill hook routes bank skills per prompt when `OPENROUTER_API_KEY` is set. Suggested MCP servers are in `.mcp.json`.

## Test pyramid

1. **Unit**: every branch of every function. Pure logic runs against in-memory adapters (`fotoko-core` ports). Protocol types need round-trip and exact wire-format tests.
2. **Integration**: real local dependencies (local MQTT broker, DynamoDB Local or equivalent, MinIO or local S3 stand-in, the printer sim), not mocks of the thing under test.
3. **Simulation e2e** (`crates/e2e`, `packages/e2e-web`): the whole flow in one process or compose: API + MQTT broker + payOS mock + Pi agent + printer sim, driven by the web app with Playwright. No real AWS, payOS or printer in CI.

Test helpers must not do expensive per-process setup (key generation, big fixtures) without caching it on disk.

## Rust rules

Edition 2024, `thiserror` in libraries, no `unwrap`/`expect`/`panic!` outside tests (workspace lints enforce it), `cargo fmt` and `clippy -D warnings`, `cargo deny check` for dependencies. Share dependency versions through `[workspace.dependencies]` and `lints.workspace = true`. The workspace lints warn on `print_stdout`/`print_stderr`; a binary that legitimately prints may add `#![allow(clippy::print_stdout, clippy::print_stderr)]`. Features such as chrono `clock` are not in the workspace default: a crate enables the ones it needs.

## Builds and disk

The disk is small and several agents build at once.

- Never share one target dir between checkouts and never set `CARGO_TARGET_DIR`. In a git worktree, add an untracked `.cargo/config.toml` (`/.cargo/config.toml` is gitignored) with `[build] target-dir = "<scratch path>/cargo-target/<worktree>"`.
- Do not enable incremental builds (profiles set `incremental = false`; sccache cannot cache them). Keep `debug = "line-tables-only"` and dependencies without debug info.
- While iterating use `cargo check` / `cargo clippy` and `cargo test --no-run`; run `cargo test` for the crate you changed when the logic is complete and `bash scripts/ci/all.sh` once before reporting.
- Run at most one implementer per checkout.
- After a worktree's branch is merged, remove the worktree (`git worktree remove`, no `--force`) and `cargo clean` its own target dir only if it is not shared.

## Secrets

`.env.example` lists every variable. Never commit `.env`, keys, certificates or payOS credentials.
