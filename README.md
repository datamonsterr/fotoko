# Fotoko

On-site pay-to-print photo kiosk. A Stand has a photo printer wired to a Raspberry Pi and a QR sticker. The customer scans it, picks photos in the Expo app (iOS, Android, web), pays with payOS, and the Pi prints through CUPS + Gutenprint. The backend is Rust on AWS Lambda; the Pi talks to AWS IoT Core over MQTT.

```
Expo app -> API Gateway -> Lambda (Rust) -> DynamoDB / S3 / payOS
                                  |
                              IoT Core (MQTT) <-> Raspberry Pi agent -> CUPS -> printer
```

Docs: [terms](CONTEXT.md), [contracts](docs/contracts.md), [architecture decision](docs/adr/0001-architecture.md), [plan](docs/plans/0001-initial-setup.md), [skills](docs/skills.md). Contributor and agent guide: [AGENTS.md](AGENTS.md).

## Quickstart

Requires Node 22 (`.nvmrc`), pnpm 11, and a stable Rust toolchain (`rust-toolchain.toml` picks it up, with rustfmt, clippy and the `aarch64-unknown-linux-gnu` target).

```sh
pnpm install                # JS deps and git hooks
cp .env.example .env        # local settings, no secrets committed
bash scripts/ci/fast.sh     # check what you changed
bash scripts/ci/all.sh      # check everything
```

Optional tools: `cargo-nextest`, `cargo-deny`, `shellcheck`.

## Layout

`crates/*` Rust workspace (`protocol` today), `apps/*` Expo app, `packages/*` shared JS, `infra/*` CDK, `deploy/pi` Pi provisioning, `scripts/ci` check scripts.
