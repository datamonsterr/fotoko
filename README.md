# Fotoko

On-site pay-to-print photo kiosk. A Stand has a photo printer wired to a Raspberry Pi and a QR sticker. The customer scans it, picks photos in the Expo app (iOS, Android, web), pays with payOS, and the Pi prints through CUPS + Gutenprint. The backend is Rust on AWS Lambda; the Pi talks to AWS IoT Core over MQTT.

```
Expo app -> API Gateway -> Lambda (Rust) -> DynamoDB / S3 / payOS
                                  |
                              IoT Core (MQTT) <-> Raspberry Pi agent -> CUPS -> printer
```

Docs: [terms](CONTEXT.md), [contracts](docs/contracts.md), [architecture decision](docs/adr/0001-architecture.md), [plan](docs/plans/0001-initial-setup.md), [skills](docs/skills.md), [architecture review](docs/architecture-review.md). Contributor and agent guide: [AGENTS.md](AGENTS.md).

## Quickstart

Requires Node 22 (`.nvmrc`), pnpm 11, and a stable Rust toolchain (`rust-toolchain.toml` picks it up, with rustfmt, clippy and the `aarch64-unknown-linux-gnu` target).

```sh
pnpm install                # JS deps and git hooks
cp .env.example .env        # local settings, no secrets committed
bash scripts/ci/fast.sh     # check what you changed
bash scripts/ci/all.sh      # check everything
```

Optional tools: `cargo-nextest`, `cargo-deny`, `shellcheck`.

## Setup

- [AWS setup](docs/setup/aws.md): accounts, budgets, quotas, CDK bootstrap, GitHub OIDC deploy role, first deploy, payOS secret and webhook, seed a printer, Pi certificate, smoke test.
- [Services](docs/setup/services.md): payOS, Expo/EAS, GitHub, domain and HTTPS, optional OpenRouter and Context7 keys, monitoring.
- [Hardware](docs/setup/hardware.md): bill of materials, flashing the Pi, installing, QR sticker, security, maintenance, offline behaviour.
- [Local development](docs/setup/local-dev.md): run and test everything locally, agent tooling (Chisle, rtk).
- `.env.example` documents every environment variable; `bash scripts/dev/check-env.sh` verifies it against the code.

## Layout

`crates/*` Rust workspace (`protocol` today), `apps/*` Expo app, `packages/*` shared JS, `infra/*` CDK, `deploy/pi` Pi provisioning, `scripts/ci` check scripts.

## Run the whole stack locally

Everything runs simulated: no AWS, no payOS, no printer. `docker-compose.yml` starts the MQTT broker, `fotoko-local` (API), the payOS mock, the Expo web app (local mode, behind a proxy to the API) and a Pi agent on the printer simulation.

```sh
docker compose up --build                 # first build compiles the Rust workspace and the web app
open http://localhost:8081/p/stand-001    # the page the QR sticker opens
```

Pick a photo, continue, then press **Pay** on the mock payOS page (<http://localhost:8082>). The order turns `completed` when the simulated printer is done; its pages are PNG files:

```sh
docker compose exec pi-agent ls /var/lib/fotoko/sim-out    # {jobId}-{index}-{photoId}-{n}.png
docker compose down -v                                       # stop and drop the printed pages
```

The agent shares the `web` container's network namespace so the photo URLs (`http://localhost:8081/...`) resolve for the agent too. The compose file uses fixed demo credentials for the payOS mock; they protect nothing.

Without Docker, the same stack runs from scripts: `bash scripts/e2e/sim-up.sh` starts it on free ports (addresses in `scripts/e2e/.sim/env.sh`), `bash scripts/e2e/sim-down.sh` stops it.

## Simulation e2e

```sh
cargo test -p fotoko-e2e                       # API + broker + payOS mock + Pi agent in one process
pnpm --filter @fotoko/e2e-web test:e2e         # Playwright against the web app on the simulated stack
```

`crates/e2e` covers the happy path, `paper_out`, webhook signature, duplicate, amount and not-paid cases, expiry and late payment, cancel, printer offline (shutdown and last will), jobs for another printer and a Pi restart. The Playwright run needs Chromium: `pnpm --filter @fotoko/e2e-web exec playwright install --with-deps chromium`, or set `FOTOKO_E2E_CHROMIUM` to an existing binary. A failed run leaves `packages/e2e-web/playwright-report` (with the stack's logs in `sim-logs/`).
