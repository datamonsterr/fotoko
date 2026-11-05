# Local development

Everything runs on one machine without AWS, payOS or a printer. Variables: `.env.example` (check with `bash scripts/dev/check-env.sh`). Architecture rules and where code goes: `AGENTS.md`.

## Prerequisites

| Tool                | Version / note                                                                                                                                                         |
| ------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Node                | 22 (`.nvmrc`), pnpm 11 (`corepack enable`)                                                                                                                             |
| Rust                | stable via `rust-toolchain.toml` (rustfmt, clippy, `aarch64-unknown-linux-gnu` target)                                                                                 |
| Docker with compose | for the full stack and the integration tests                                                                                                                           |
| Optional            | `cargo-nextest`, `cargo-deny`, `shellcheck`, `actionlint`, `cargo-lambda` (`pip3 install cargo-lambda`), `aarch64-linux-gnu-gcc` or `cargo-zigbuild`, `jq`, `qrencode` |

In a git worktree add an untracked `.cargo/config.toml` with its own `[build] target-dir` first (`AGENTS.md`, "Builds and disk").

```sh
pnpm install                 # JS dependencies and the lefthook git hooks
cp .env.example .env         # then edit; `set -a; . ./.env; set +a` loads it into a shell
bash scripts/ci/fast.sh      # what changed
bash scripts/ci/all.sh       # everything CI runs, skipping tools you lack
```

## Whole stack in Docker

```sh
docker compose up --build                  # broker, API (fotoko-local), payOS mock, web app, Pi agent on the printer sim
open http://localhost:8081/p/stand-001     # what the QR sticker opens
docker compose exec pi-agent ls /var/lib/fotoko/sim-out   # printed pages (PNG)
docker compose down -v
```

Pick a photo, continue, press **Pay** on the mock at <http://localhost:8082>. Details in the README ("Run the whole stack locally"). Without Docker: `bash scripts/e2e/sim-up.sh` / `sim-down.sh` start and stop the same stack on free ports (addresses in `scripts/e2e/.sim/env.sh`).

## Pieces separately

**API (`fotoko-local`)** uses in-memory repositories (`fotoko-core`'s `testing` adapters), local-fs photo storage and optional MQTT. It exists only behind the cargo feature `local` and never ships in a Lambda:

```sh
# terminal 1: MQTT broker on 127.0.0.1:1883
cargo run -p fotoko-e2e --bin fotoko-sim-broker
# terminal 2: payOS mock (hosted checkout page + signed webhooks); needs the API address
PAYOS_CLIENT_ID=c PAYOS_API_KEY=k PAYOS_CHECKSUM_KEY=s FOTOKO_API_URL=http://127.0.0.1:8787 \
PAYOS_MOCK_ADDR=127.0.0.1:8082 PAYOS_MOCK_PUBLIC_URL=http://localhost:8082 \
  cargo run -p fotoko-e2e --bin fotoko-payos-mock
# terminal 3: the API
PAYOS_CLIENT_ID=c PAYOS_API_KEY=k PAYOS_CHECKSUM_KEY=s PAYOS_BASE_URL=http://127.0.0.1:8082 \
FOTOKO_MQTT_URL=mqtt://127.0.0.1:1883 PUBLIC_APP_URL=http://localhost:8081 \
  cargo run -p fotoko-api --features local --bin fotoko-local     # listens on 127.0.0.1:8787
```

Callers identify themselves with the `x-fotoko-identity` header; printers listed in `FOTOKO_LOCAL_PRINTERS` (default `stand-001`) start online. Run a Pi agent on the printer simulation with a config like `docker/pi-agent.toml` (`backend = "sim"`, plain MQTT, `allow_insecure_http = true`): `cargo run -p fotoko-pi-agent -- --config ./agent.toml`.

**Mobile app against the local API:**

```sh
printf 'EXPO_PUBLIC_API_URL=http://localhost:8787\nEXPO_PUBLIC_FOTOKO_MODE=local\n' > apps/mobile/.env
pnpm --filter @fotoko/mobile web         # or `start` for Expo Go / a simulator
```

On a phone, use your computer's LAN IP instead of `localhost` in `EXPO_PUBLIC_API_URL` and `FOTOKO_LOCAL_PUBLIC_URL`. Mobile checks: `pnpm --filter @fotoko/mobile lint | typecheck | test`.

**CDK without AWS:** `FOTOKO_LAMBDA_DIR=infra/cdk/test/fixtures/lambda pnpm --filter @fotoko/infra synth`; tests `pnpm --filter @fotoko/infra test`.

## Tests

```sh
cargo test -p fotoko-core                      # unit, in-memory adapters
cargo test -p fotoko-api --all-features        # unit + local server + embedded MQTT broker
docker compose -f docker-compose.test.yml up -d
FOTOKO_IT=1 cargo test -p fotoko-api --all-features -- --include-ignored   # DynamoDB Local + MinIO
docker compose -f docker-compose.test.yml down
cargo test -p fotoko-e2e                       # API + broker + payOS mock + Pi agent in one process
pnpm --filter @fotoko/e2e-web exec playwright install --with-deps chromium   # once
pnpm --filter @fotoko/e2e-web test:e2e         # Playwright on the simulated stack
bash scripts/ci/repro.sh <job>                 # any single CI job, e.g. arch, rust-lint, cdk-synth
bash scripts/ci/check-arch.sh                  # crate dependency rules (also the `arch` CI job)
bash scripts/dev/check-env.sh                  # .env.example vs the code
```

`FOTOKO_E2E_CHROMIUM=/path/to/chromium` uses an existing browser. A failed web e2e leaves `packages/e2e-web/playwright-report` with the stack logs.

## Agent tooling

Optional, developer machines only. Nothing here is required by CI. Catalogue and provenance: `docs/skills.md`.

### Skills routing (OtherSkill)

`docs/skills.md` ("Hook setup") installs the launcher; the hook in `.claude/settings.json` no-ops without it or without `OPENROUTER_API_KEY`.

### Chisle (terse output, tool-output compression)

[Chisle](https://github.com/JayPokale/Chisle) (MIT) makes Claude Code answer tersely and trims oversized tool output (`PostToolUse`) before it re-enters the context. It is enabled **per project** from `.claude/settings.json`, pinned to commit `1b3203fe394e` (version 3.8.1):

```json
"extraKnownMarketplaces": { "fotoko-pinned": { "source": { "source": "settings", "name": "fotoko-pinned",
  "plugins": [ { "name": "chisle", "source": { "source": "github", "repo": "JayPokale/Chisle", "ref": "main", "sha": "1b3203fe..." } } ] } } },
"enabledPlugins": { "chisle@fotoko-pinned": true }
```

The inline marketplace is used because a marketplace pointing at a repository cannot be pinned to a commit (only a branch or tag `ref`), while a plugin entry can (`sha`). Claude Code asks you to trust the project and install the plugin on the first session; nothing to run by hand. Its hooks (`SessionStart`, `UserPromptSubmit`, `PostToolUse`) are the plugin's own and run alongside the project hooks (OtherSkill `UserPromptSubmit`, subagent-model guard and rtk `PreToolUse`); hooks from several sources are merged, not replaced. Behaviour notes:

- Prose is terse (fragments, no filler); code, commits, security text and docs are written normally, so it does not contradict `AGENTS.md` (Conventional Commits, full sentences in docs).
- `Read`, `Edit` and `Write` output is never touched; only `Bash|Agent|WebFetch|WebSearch|Grep|Glob|mcp__*` results are compressed. Full output is spilled to `$CLAUDE_CONFIG_DIR/chisle-spill` when elided. Needs `node` on `PATH`; without it the hooks exit 0 silently.
- The `SessionStart` hook may check the npm registry for a newer Chisle (1.5 s timeout, cached). Switches: `CHISLE_DEFAULT_MODE=off` (rules off), `CHISLE_COMPRESS=0` (no output compression), `/chisle off` in a session.
- Update deliberately: read the diff of `JayPokale/Chisle` between the pinned commit and the new one, then change the `sha`. Remove by deleting the two settings keys.

### rtk (Rust Token Killer, command output filters)

[rtk](https://github.com/rtk-ai/rtk) rewrites shell commands (`git status`, `cargo test`, `ls`, `rg`, `pnpm ...`) to filtered equivalents (`rtk git status`) so the agent sees compact, failure-focused output. License: Apache-2.0 for the hook core and all command filters (`--no-default-features` / the `rtk-oss` download); the default build also contains a BUSL-1.1 compression engine that is free for internal production use. Prefer the Apache-2.0 edition. Install one of:

```sh
cargo install --git https://github.com/rtk-ai/rtk --rev 690616d41279db8a7cafb3d230bf01d2b29ec935 --locked --no-default-features   # Apache-2.0 build, needs Rust 1.91+ (~4 min)
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | RTK_EDITION=oss sh                          # release binary to ~/.local/bin, verifies checksums
brew install rtk-ai/tap/rtk-oss
rtk --version && rtk rewrite "git status"          # prints `rtk git status`
```

Also needs `jq`. The project hook `.claude/hooks/rtk-rewrite.sh` (registered as a `Bash` `PreToolUse` hook in `.claude/settings.json`) delegates to `rtk rewrite` and **does nothing when `rtk` or `jq` is missing**, so teammates without it are unaffected. Do **not** run `rtk init -g` for this repo: that writes a second, global hook. Rewritten commands are left to Claude Code's normal permission prompts unless one of your own allow rules matches.

**Never rewritten** (the hook skips them so the agent sees exact output): anything mentioning `scripts/ci/`, `scripts/dev/`, `scripts/deploy/`, `scripts/e2e/` or `deploy/pi/` (CI summaries and repro commands, deploy and Pi scripts), `git commit` and `git push` (lefthook output), and commands that already start with `rtk`. If a filtered result hides something you need, run the command through `rtk bypass <cmd>` or rerun it raw. `rtk gain` shows savings. Hook order: the subagent-model guard matches `Agent|Task`, rtk matches `Bash`; they never see the same call. Test the hook with `bash .claude/hooks/test-rtk-rewrite.sh` (uses a stub `rtk`).

### MCP servers and the architecture skill

`.mcp.json` lists Context7, AWS documentation and IaC, Expo and Playwright servers. The `improve-codebase-architecture` skill (with `codebase-design`) is installed natively in `.claude/skills/`: run it on demand ("improve codebase architecture"); the latest result is `docs/architecture-review.md`.
