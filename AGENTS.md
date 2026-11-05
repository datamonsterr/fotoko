# Fotoko

On-site pay-to-print photo kiosk. Customers scan a QR code at a Stand, upload photos in an Expo app, pay with payOS, and a Raspberry Pi prints them through CUPS + Gutenprint. Read `CONTEXT.md` (terms), `docs/contracts.md` (authoritative interfaces) and `docs/adr/` (decisions) first. Use the glossary terms exactly.

## Layout

| Path                                                   | What                                                                  |
| ------------------------------------------------------ | --------------------------------------------------------------------- |
| `crates/protocol`                                      | HTTP + MQTT DTOs, topics, limits (`fotoko-protocol`)                  |
| `crates/core`, `api`, `pi-agent`, `printer-sim`, `e2e` | Domain, Lambdas, Pi agent, printer simulation, sim e2e                |
| `apps/mobile`                                          | Expo app (iOS, Android, web)                                          |
| `infra/cdk`                                            | AWS CDK app                                                           |
| `packages/e2e-web`                                     | Playwright web e2e                                                    |
| `deploy/pi`                                            | Pi provisioning and cross-build                                       |
| `scripts/ci`                                           | `fast.sh`, `all.sh`, `repro.sh` (one CI job locally), `check-arch.sh` |
| `scripts/dev`, `scripts/deploy`                        | `seed-printer.sh`, `check-env.sh`; `aws.sh` deploy                    |
| `docs/setup`                                           | Setup guides: AWS, services, hardware, local dev                      |
| `.github`                                              | CI (`ci.yml`), deploy, Dependabot, PR template                        |

Cargo members are `crates/*`; pnpm packages are `apps/*`, `packages/*`, `infra/*`. A new crate or package is picked up by the globs with no root edit, except that a crate other crates depend on needs a `path` entry in the root `[workspace.dependencies]`.

## Commands

```sh
pnpm install                      # also installs lefthook git hooks
bash scripts/ci/fast.sh           # changed scope: workspace clippy, tests of changed crates + their reverse deps, turbo, Prettier
bash scripts/ci/all.sh            # everything CI runs (fmt, clippy, tests, doctests, cargo deny, turbo, Prettier, arch)
bash scripts/ci/check-arch.sh     # crate dependency rules (CI job `arch`)
bash scripts/dev/check-env.sh     # .env.example vs the variables the code reads
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
- On demand: the `improve-codebase-architecture` skill (with `codebase-design`) reviews module depth and seams; it reads project terms from `CONTEXT.md` (it calls that file `GLOSSARY.md`). Latest result and open follow-ups: `docs/architecture-review.md`.

## Setup guides

Accounts, cloud, hardware and local runs are documented in `docs/setup/`: [`aws.md`](docs/setup/aws.md) (account, OIDC deploy role, first deploy, payOS secret and webhook, seeding a printer, Pi certificate), [`services.md`](docs/setup/services.md) (payOS, Expo/EAS, GitHub, domain, monitoring, optional keys), [`hardware.md`](docs/setup/hardware.md) (bill of materials, flashing, QR sticker, maintenance) and [`local-dev.md`](docs/setup/local-dev.md) (everything locally, tests, agent tooling). `.env.example` is annotated per variable; `scripts/dev/check-env.sh` keeps it in sync with the code.

## Architecture rules

`scripts/ci/check_arch.py` (CI job `arch`) enforces the dependency rules; the review behind them is `docs/architecture-review.md`. Use the vocabulary of the `codebase-design` skill: prefer **deep modules** (a small interface in front of much behaviour), put a **seam** only where something varies (one adapter is a hypothetical seam, two is real), and test through the interface.

- **Dependency direction**: `fotoko-protocol` <- `fotoko-core` <- `fotoko-api`; `fotoko-protocol` <- `fotoko-pi-agent` -> `fotoko-printer-sim`. `pi-agent` never depends on `core` or `api`; `api` never on `pi-agent`; nothing depends on `fotoko-e2e` (test harness). `protocol` has no IO dependencies (chrono, serde, serde_json, thiserror only). `core` has no IO crates (no aws-sdk, axum, tokio, reqwest, rumqtt, lambda); a new workspace edge needs a deliberate edit of `ALLOWED` in `scripts/ci/check_arch.py`, with the reason in the PR.
- **Where code goes**: wire types, topics and limits in `protocol`; business rules, the order state machine and ports in `core` (pure, no IO, tested with in-memory adapters); HTTP routing, AWS adapters and Lambda `main`s in `api`; Pi behaviour (MQTT, download, CUPS) in `pi-agent`; simulation only in `printer-sim` and `e2e`; infra only in `infra/cdk`; scripts that touch the world in `scripts/` or `deploy/`. A new capability gets a port in `core` first, then an adapter in `api`.
- **Contract first**: change `docs/contracts.md`, then `protocol`, then `core`/`api`/`pi-agent`, then `apps/mobile/src/api/types.ts` (hand-mirrored DTOs and limits, `lib/limits.ts`) and CDK literals (topic root, price). Add or update the exact wire-format test in the same change.
- **Feature flags**: `local` (fotoko-api) and `testing` (fotoko-core) are for the local/sim server and tests only. They are never enabled by a deploy path (Lambda build, Pi build, CDK, `deploy.yml`), `local` is not a default feature, and `fotoko-local` is the only binary that requires it. CI builds with `--all-features` for tests and without for the Lambda bins; `check_arch.py` checks both.
- **Size and shape**: aim for modules under about 500 lines of non-test code and functions that fit a screen. When inline tests outgrow the code (above ~300 lines or more than the code), move them to a sibling `tests.rs` (`core/src/service` is the pattern). Do not add pass-through modules: if deleting a module only moves its code into its caller unchanged, it was not deep enough to exist.
- **Tests**: unit tests live next to the code (or `tests.rs`), integration tests in the crate's `tests/` against real local dependencies, cross-component flows only in `crates/e2e` and `packages/e2e-web`. No test helper may depend on a production crate that depends on it back (no dev-dependency cycles such as the current `printer-sim` <-> `pi-agent`, a tracked follow-up).
- **Errors and IO**: libraries return typed errors (`thiserror`); IO lives at the edges (adapters, binaries) and is injected (`Clock`, `IdGen`, repos) so domain code stays testable.

## Agent tooling (token savings)

Project-scoped, optional, and silent when absent. Install steps and exclusions: `docs/setup/local-dev.md` ("Agent tooling").

- **Chisle** (terse prose, tool-output compression): enabled for this repo through `extraKnownMarketplaces`/`enabledPlugins` in `.claude/settings.json`, pinned to a commit. Code, commits and docs stay normal-length; nothing in Chisle contradicts these rules.
- **rtk** (command output filters): the `Bash` `PreToolUse` hook `.claude/hooks/rtk-rewrite.sh` rewrites commands when `rtk` and `jq` are installed and does nothing otherwise. It never rewrites `scripts/ci|dev|deploy|e2e/*`, `deploy/pi/*`, `git commit` or `git push`, so CI summaries and hook output arrive verbatim. If a filtered output hides what you need, rerun raw with `rtk bypass <cmd>`.

## CI

`.github/workflows/ci.yml` runs on every PR and push to `main`; the single required check is `ci-ok` (skipped jobs count as passed). Every job runs `bash scripts/ci/repro.sh <job>`, so the job id is also the local repro command. The fix loop: push, read the failed job's summary (it lists the failing tests and the exact command), run that command locally, fix, repeat. `bash scripts/ci/fast.sh` is the pre-push check and `bash scripts/ci/all.sh` mirrors the whole workflow. Job map, caching and how to add jobs: `docs/ci.md`.

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
