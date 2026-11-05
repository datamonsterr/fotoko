# Plan 0001: initial setup and vertical slice

> Historical: executed 2026-11-05. Absolute paths below refer to the machine the plan ran on. Deviations and later decisions are recorded in ADRs 0002–0004.

Goal: monorepo where a Stand user can scan QR -> upload -> pay (payOS) -> print, fully runnable in simulation, with unit/integration/e2e tests, AWS CDK infra, Pi deployment scripts, agent tooling and fast CI. Read `docs/contracts.md` and `docs/adr/0001-architecture.md`. Terms: `CONTEXT.md`.

Global rules for every task:

- TDD: failing test first. Unit tests cover every branch of every public function (happy path, each error, boundaries).
- Check current docs (Context7 or official docs) before using any library/SDK/CLI API. Pin versions.
- Rust: edition 2024, `cargo fmt`, `cargo clippy --all-targets -- -D warnings` clean, `thiserror` in libs, no `unwrap()` outside tests.
- TS: strict, ESLint + Prettier clean.
- Do not edit files owned by another task except where listed.
- Commit with Conventional Commits on the current branch.

## Wave 1

### T1 Monorepo scaffold + agent tooling (sonnet)

Files: root config only, `crates/protocol`, `.claude/`, `.otherskill/`, `otherskill.config.json`, `.mcp.json`, `AGENTS.md`, `CLAUDE.md`, `README.md`.

- pnpm 11 workspace + Turborepo (`lint`, `typecheck`, `test`, `test:integration`, `test:e2e`, `build`, `format:check`), Node 22 (`.nvmrc`), Prettier, commitlint, lefthook (pre-commit: fmt check on staged; pre-push: fast check), `.editorconfig`, `.gitignore`, `.env.example` (all env names in contracts, no secrets).
- Cargo workspace `members = ["crates/*"]`, `resolver = "3"`, `rust-toolchain.toml` (stable + rustfmt, clippy, target aarch64-unknown-linux-gnu), `rustfmt.toml`, `clippy.toml`, `deny.toml`, `.config/nextest.toml`, workspace deps for shared crates, dev/test profiles `debug = "line-tables-only"`.
- `crates/protocol`: serde DTOs + enums for every HTTP body and MQTT payload in contracts, topic builder fns, constants (limits). Unit tests: round-trip JSON with camelCase/snake_case enum values for every type, topic fns, limit constants.
- `scripts/ci/fast.sh`: changed-scope quick loop (fmt, clippy, unit tests for touched Rust crates / JS packages via `git diff --name-only origin/main...`), `scripts/ci/all.sh`: everything.
- Agent tooling: copy `.claude/agents/{implementer-haiku,implementer-sonnet,spec-reviewer,code-quality-reviewer}.md` from `/home/user/superorchestrator/.claude/agents` (adapt paths). Install skills with OtherSkill (`bun /home/user/other-skill/otherskill.ts`, `--scope project`, explicit `--category` since no OpenRouter key): native `use-otherskill` companion; bank categories `frontend/react`, `frontend/ui-ux`, `backend/rust`, `devops`, `testing`, `security`, `management`, `docs`. Find skills on GitHub (`otherskill search`, GH_TOKEN set) for: Expo/React Native, Rust (axum, tokio, error handling, lambda), AWS CDK/serverless, DynamoDB, IoT/MQTT, Raspberry Pi/CUPS, payOS/payments, testing (TDD, Playwright, Maestro), CI GitHub Actions, skill-creator, find-skills, subagent-driven-development, writing-plans. Inspect source + license before adding. Native (always-on) skills under `.claude/skills/`: `use-otherskill`, `subagent-driven-development`, `test-driven-development`, `skill-creator`, `find-skills`. Record each skill + source + license in `docs/skills.md`.
- `.mcp.json`: Context7, AWS documentation MCP, AWS CDK/IaC MCP, Expo MCP, Playwright MCP (check current package names/URLs). No secrets.
- `AGENTS.md` (+ `CLAUDE.md` = `@AGENTS.md`): layout, commands, workflow (subagent-driven-development, native fallback), test pyramid rules, disk rules.
  Acceptance: `pnpm install --frozen-lockfile=false && pnpm format:check`; `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p fotoko-protocol`; `bash scripts/ci/fast.sh` exits 0.
  Out of scope: app code, CI workflows (T8).

## Wave 2 (parallel, disjoint dirs)

### T2 `crates/core` (sonnet)

Domain: `Order`, `Photo`, `Printer`, `OrderStatus` transitions (contracts table, exhaustive tests of every allowed and rejected transition), pricing, validation, expiry, `OrderService` use cases (`get_printer`, `create_order`, `checkout`, `get_order`, `cancel`, `handle_payos_webhook`, `handle_job_status`, `handle_heartbeat`, `expire_due`). Ports: `OrderRepo`, `PrinterRepo`, `ObjectStore` (presign put/get, exists), `JobPublisher`, `PaymentGateway` (create link), `Clock`, `IdGen`. payOS: `sign_payment_request`, `verify_webhook` (pure, HMAC-SHA256, test vectors from payOS docs). In-memory adapters in `fotoko_core::testing` (feature `testing`).
Acceptance: `cargo test -p fotoko-core --all-features`; clippy clean.
Out of scope: AWS SDK, HTTP.

### T3 `crates/printer-sim` + `crates/pi-agent` (sonnet)

- printer-sim lib: `SimPrinter` emulating Gutenprint pipeline: decode JPEG/PNG (`image`), auto-rotate to landscape/portrait to fit, scale+crop to 4x6 @300dpi (1800x1200), write PNG per copy to `out_dir`, paper counter -> `paper_out`, fault modes (`offline`, `ribbon_out`, `driver_error`), optional latency. Bins `fake-lp` and `fake-lpstat` mimicking CUPS CLI output/exit codes (`request id is <queue>-<n> (1 file(s))`, `lpstat -o`/`-p` formats; check CUPS man pages) and rendering via `SimPrinter`, state in a dir from `FAKE_CUPS_DIR`.
- pi-agent: config (contracts TOML), `Printer` trait with `CupsPrinter` (runs `lp -d queue -n copies -o media=... -o fit-to-page file`, polls `lpstat`, maps errors) and sim backend, MQTT client (`rumqttc`, TLS with device cert, last will), job handler (validation rules from contracts, dedupe store, download with size cap + magic bytes, status publishing, heartbeat every 30 s), graceful shutdown on SIGTERM.
- Integration tests: agent + embedded `rumqttd` broker + local HTTP image server + `SimPrinter`; `CupsPrinter` against `fake-lp`/`fake-lpstat`.
  Acceptance: `cargo test -p fotoko-printer-sim -p fotoko-pi-agent`; clippy clean.
  Out of scope: deploy scripts (T7).

### T4 `apps/mobile` (sonnet)

Expo (latest SDK, check docs) + Expo Router + TypeScript, iOS/Android/web. Screens: `/p/[printerId]` (printer info + price, offline state), photo picker (`expo-image-picker`, up to 10, copies 1..5, total price), upload (presigned PUT, progress, retry), checkout (open payOS `checkoutUrl` with `expo-web-browser`, web: same tab), `/orders/[orderId]` status poll (2 s, stop on terminal), done/failed screens; QR scan screen (`expo-camera`) parsing `https://<app>/p/<id>` or `fotoko://p/<id>`. API client module typed from contracts, base URL `EXPO_PUBLIC_API_URL`, auth abstraction: Cognito guest SigV4 (Amplify, check docs) in aws mode, `x-fotoko-identity` header in local mode (`EXPO_PUBLIC_FOTOKO_MODE`). i18n vi + en. Jest + React Native Testing Library: unit test every hook/util/component branch, API client against mocked fetch. Maestro flows in `apps/mobile/.maestro/` (not run in CI). Scripts: `lint`, `typecheck`, `test`, `build:web` (`expo export -p web`).
Acceptance: `pnpm --filter @fotoko/mobile lint typecheck test` and `pnpm --filter @fotoko/mobile build:web`.
Out of scope: Playwright (T7), admin UI.

### T5 `infra/cdk` (sonnet)

CDK v2 TypeScript app, stacks parametrized by stage (`dev`, `prod`), region `ap-southeast-1`: DynamoDB table + gsi1 + TTL, S3 bucket (private, CORS PUT from app origin, 7-day lifecycle, SSE, block public), Cognito identity pool with guest access + IAM role allowed `execute-api:Invoke` on API, HTTP API with IAM auth (public routes per contracts), Lambda `fotoko-http` + `fotoko-iot-status` (arm64, `provided.al2023`, code from `FOTOKO_LAMBDA_DIR`), Secrets Manager payOS secret (placeholder, no value), IoT: thing type, IoT policy scoped by thing name (contracts topics), topic rule -> `fotoko-iot-status`, Lambda permission to `iot:Publish` jobs topic; web hosting S3 + CloudFront for Expo web export; least-privilege grants; alarms on Lambda errors. Tests: Jest/Vitest `aws-cdk-lib/assertions` for every resource + IAM policy + snapshot. Scripts: `synth`, `deploy`, `diff`, `test`, `lint`, `typecheck`. `scripts/provision-printer.ts` (or sh): create thing + cert, attach policy, write cert files for a Pi.
Acceptance: `pnpm --filter @fotoko/infra lint typecheck test` and `pnpm --filter @fotoko/infra synth` (with fixture lambda dir).
Out of scope: real deploy.

## Wave 3

### T6 `crates/api` (sonnet)

axum router implementing contracts HTTP API over `fotoko-core::OrderService`; `lambda_http` bin `fotoko-http`, `lambda_runtime` bin `fotoko-iot-status`, bin `fotoko-local` (axum on `:8787`, in-memory repos, local-fs object store serving presigned-style URLs, MQTT publisher via `rumqttc`, payOS base URL configurable). AWS adapters: DynamoDB (`aws-sdk-dynamodb`, conditional writes), S3 presign (`aws-sdk-s3`), IoT data publish (`aws-sdk-iotdataplane`), payOS HTTP client (`reqwest` rustls), Secrets Manager loader. Unit tests: router with in-memory adapters, every route + error code. Integration tests (`tests/`, `#[ignore]` unless `FOTOKO_IT=1`): DynamoDB Local + MinIO via docker compose `docker-compose.test.yml`, payOS client vs `wiremock`.
Acceptance: `cargo test -p fotoko-api`; `docker compose -f docker-compose.test.yml up -d && FOTOKO_IT=1 cargo test -p fotoko-api -- --include-ignored`.

## Wave 4

### T7 Simulation e2e + deploy (sonnet)

- `crates/e2e`: in-process full flow: `fotoko-local` router + embedded `rumqttd` + payOS mock (wiremock, signs webhooks with checksum key) + pi-agent with `SimPrinter`. Cases: happy path (assert PNG pages count == copies), paper_out -> order failed, bad webhook signature, duplicate webhook, wrong amount, expired order, cancel, printer offline (heartbeat last will), wrong printerId job ignored.
- `packages/e2e-web`: Playwright against `expo export -p web` served statically + `fotoko-local` + pi-agent sim + payOS mock (scripted via `scripts/e2e/sim-up.sh`): open `/p/stand-001`, upload fixture image, pay (mock redirects back), wait completed.
- `docker-compose.yml` full local stack (api local, mosquitto, pi-agent sim, payos mock, web).
- `deploy/pi`: `install.sh` (apt cups, printer-driver-gutenprint, create queue with lpadmin, user, dirs, perms 0600 key), `fotoko-agent.service` (systemd hardening: NoNewPrivileges, ProtectSystem=strict, ReadWritePaths), `build.sh` (cross build aarch64 via `cross` or `cargo zigbuild`, check docs), `deploy.sh` (scp + restart), firewall (ufw deny incoming), README. `scripts/deploy/aws.sh`: cargo lambda build -> cdk deploy -> expo export -> s3 sync + invalidation. shellcheck clean.
  Acceptance: `cargo test -p fotoko-e2e`; `pnpm --filter @fotoko/e2e-web test:e2e`; `shellcheck deploy/pi/*.sh scripts/**/*.sh`.

### T8 CI/CD (sonnet)

`.github/workflows/ci.yml` for PRs + main: concurrency cancel-in-progress, `dorny/paths-filter` change detection, parallel jobs: `rust-lint` (fmt, clippy), `rust-test` matrix per crate with nextest + `Swatinem/rust-cache`, `rust-integration` (compose services), `rust-e2e`, `deny`, `js` matrix (`mobile`, `infra`) lint/typecheck/test with pnpm cache, `cdk-synth`, `web-e2e` (Playwright, upload report artifact on failure), `pi-cross-build` (aarch64), `shellcheck`, final `ci-ok` aggregator (required check). Failed-job logs concise (`--message-format short`, nextest `--failure-output final`). `deploy.yml`: manual/`main` -> OIDC AWS role -> `scripts/deploy/aws.sh` (stage env). `dependabot.yml`. `.github/pull_request_template.md`. Validate with `actionlint`.
Acceptance: `actionlint` clean; `bash scripts/ci/all.sh` passes locally.

## Final

Whole-branch code-quality review; `bash scripts/ci/all.sh`.
