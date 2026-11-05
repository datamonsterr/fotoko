# fotoko-api

The Fotoko backend: an axum router over `fotoko-core::OrderService`, the AWS adapters and the
binaries. Interfaces are in `docs/contracts.md`.

| Bin                 | What                                                       | Features |
| ------------------- | ---------------------------------------------------------- | -------- |
| `fotoko-http`       | API Gateway HTTP API Lambda (`lambda_http` + the router)   | -        |
| `fotoko-iot-status` | IoT rule Lambda: Pi job statuses and heartbeats            | -        |
| `fotoko-expire`     | EventBridge schedule Lambda: expires overdue unpaid orders | -        |
| `fotoko-local`      | Local/sim server (in-memory repos, local-fs objects, MQTT) | `local`  |

The Lambda bins build with the default features, so none of the local/sim code (in-memory
adapters from `fotoko-core`'s `testing` feature, `rumqttc`) ends up in them.

## Run the local server

```sh
PAYOS_CLIENT_ID=... PAYOS_API_KEY=... PAYOS_CHECKSUM_KEY=... \
FOTOKO_MQTT_URL=mqtt://127.0.0.1:1883 \
cargo run -p fotoko-api --features local --bin fotoko-local
```

Environment: `FOTOKO_LOCAL_ADDR` (default `127.0.0.1:8787`; port `0` picks a free port, the
chosen address is printed), `FOTOKO_LOCAL_PUBLIC_URL` (base of the presigned-style object URLs,
default `http://<addr>`), `FOTOKO_LOCAL_DIR` (photos), `FOTOKO_LOCAL_SECRET` (URL signing),
`FOTOKO_LOCAL_PRINTERS` (comma separated, default `stand-001`, seeded online),
`FOTOKO_MQTT_URL` (unset: jobs are only recorded), `PAYOS_BASE_URL`, `PUBLIC_APP_URL`,
`DEFAULT_PRICE_VND`. Callers identify themselves with the `x-fotoko-identity` header.

From Rust (for the e2e crate): `fotoko_api::local::LocalApp::builder()...build().await`, then
`app.router` in-process or `app.serve(listener)`.

## Tests

```sh
cargo test -p fotoko-api --all-features        # unit + local server + embedded MQTT broker
cargo test -p fotoko-api                       # without the local server

docker compose -f docker-compose.test.yml up -d
FOTOKO_IT=1 cargo test -p fotoko-api --all-features -- --include-ignored   # DynamoDB Local + MinIO
docker compose -f docker-compose.test.yml down
```

`--include-ignored` without `FOTOKO_IT=1` fails on purpose.

## Operational notes

- The payOS client makes one attempt per call (5 s connect, 15 s total, no redirects). Give the
  `fotoko-http` Lambda a timeout above that, e.g. 29 s.
- A rotated payOS checksum key is picked up without a restart: the first failed webhook
  signature or failed checkout re-reads the secret (at most once a minute).
- `list_due_for_expiry` reads one page of the `gsi2` index per call; `expire_due` calls it until
  it returns nothing. Corrupt items in the index are logged (`pk`) and skipped.
