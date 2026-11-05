# Contracts

Single source for interfaces shared across tasks. Change here first, then code.

## Repository layout

```
apps/mobile            Expo app (iOS, Android, web)          @fotoko/mobile
crates/protocol        MQTT + HTTP DTOs (serde), no IO       fotoko-protocol
crates/core            domain, state machine, pricing, payOS signing, ports   fotoko-core
crates/api             Lambda bins + AWS adapters + local server              fotoko-api
crates/pi-agent        Pi MQTT agent + CUPS printer backend                   fotoko-pi-agent
crates/printer-sim     Gutenprint/CUPS simulation (lib + fake `lp`/`lpstat`)  fotoko-printer-sim
crates/e2e             simulation e2e tests (whole flow)                       fotoko-e2e
infra/cdk              AWS CDK app (TypeScript)              @fotoko/infra
deploy/pi              Pi provisioning, systemd, cross-build scripts
packages/e2e-web       Playwright web e2e                    @fotoko/e2e-web
scripts/ci             CI + local fast-loop scripts
```

Root `Cargo.toml` workspace members: `["crates/*"]`. `pnpm-workspace.yaml`: `apps/*`, `packages/*`, `infra/*`.

## HTTP API (base `/v1`, JSON, camelCase)

Auth: API Gateway IAM auth with Cognito identity pool guest credentials (SigV4). Handler reads the Cognito identity id from the request context. Local/sim mode only (`FOTOKO_MODE=local`): identity from header `x-fotoko-identity`. Public routes: `GET /v1/printers/{id}`, webhook, `GET /healthz`.

Errors: `{"error":{"code":"<snake_case>","message":"..."}}`. Codes: `not_found`, `validation`, `forbidden`, `conflict`, `payment_provider`, `internal`.

| Method + path                        | Request                                                                            | Response                                                                                                               |
| ------------------------------------ | ---------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `GET /healthz`                       | -                                                                                  | `200 {"ok":true}`                                                                                                      |
| `GET /v1/printers/{printerId}`       | -                                                                                  | `200 {printerId,name,status:"online"\|"offline"\|"busy"\|"error",pricePerPrintVnd,media:["4x6"]}`                      |
| `POST /v1/orders`                    | `{printerId, photos:[{contentType:"image/jpeg"\|"image/png", sizeBytes, copies}]}` | `201 {orderId,status:"awaiting_upload",amountVnd,expiresAt,uploads:[{photoId,url,method:"PUT",headers:{},expiresAt}]}` |
| `GET /v1/orders/{orderId}`           | -                                                                                  | `200 Order` (owner only)                                                                                               |
| `POST /v1/orders/{orderId}/checkout` | -                                                                                  | `200 {orderId,status:"awaiting_payment",orderCode,checkoutUrl,qrCode}`; fails `conflict` if a photo is not uploaded    |
| `POST /v1/orders/{orderId}/cancel`   | -                                                                                  | `200 Order`; only before `paid`                                                                                        |
| `POST /v1/payments/payos/webhook`    | payOS webhook body                                                                 | `200 {"success":true}`; bad signature -> `400 validation`                                                              |

Validation: 1..=10 photos, copies 1..=5 each, sizeBytes 1..=15 MiB, printer must be `online` or `busy` (printing; jobs queue on the Pi, bound 32), not `offline` or `error`. `amountVnd = sum(copies) * pricePerPrintVnd`. Order expires 15 min after creation unless paid. `uploads[i]` corresponds to request `photos[i]` (same order).

Webhook handling (all return `200 {"success":true}` unless noted): paid -> order `paid` + job published; duplicate while `paid` -> job republished with the same `jobId` (Pi dedupes); duplicate after `printing` -> no-op; signed `data.code != "00"` -> no-op; payment for a `cancelled`/`expired` order -> no-op, logged at warn for manual refund (late payment); unknown `orderCode` -> no-op (payOS sends a sample on webhook URL confirmation); bad signature or missing `data.code` -> `400 validation`; amount mismatch -> `400 validation`, order unchanged.

`Order`: `{orderId, printerId, status, amountVnd, orderCode?, photos:[{photoId,copies,contentType,uploaded}], createdAt, updatedAt, expiresAt, error?:{code,message}}` (RFC 3339 times).

Order status machine:

```
awaiting_upload --checkout--> awaiting_payment --webhook paid--> paid --job received--> printing --completed--> completed
awaiting_upload|awaiting_payment --cancel--> cancelled
awaiting_upload|awaiting_payment --now>expiresAt--> expired
paid|printing --pi failed--> failed
```

Any other transition is rejected with `conflict`.

## payOS

Check current docs before coding (payos.vn docs, `payOSHQ` SDKs). Expected:

- Create link: `POST {PAYOS_BASE_URL}/v2/payment-requests`, headers `x-client-id`, `x-api-key`; body `orderCode, amount, description (<=25 chars), cancelUrl, returnUrl, signature`. `signature = HMAC_SHA256(checksumKey, "amount=..&cancelUrl=..&description=..&orderCode=..&returnUrl=..")` hex.
- Webhook: `{code, desc, success, data:{orderCode, amount, ...}, signature}`; `signature = HMAC_SHA256(checksumKey, data keys sorted alphabetically joined as k=v&...)`. The paid decision uses the signed `data.code == "00"` (the outer `code` is unsigned and only cross-checked). Null values sign as empty strings. Amount must equal order amount.
- `returnUrl`/`cancelUrl`: `{PUBLIC_APP_URL}/orders/{orderId}`.

## MQTT (AWS IoT Core; local: embedded `rumqttd` or mosquitto)

Thing name = `printerId`. QoS 1. JSON, camelCase.

| Topic                                   | Direction   | Payload                                                                                                                                    |
| --------------------------------------- | ----------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `fotoko/printers/{printerId}/jobs`      | cloud -> Pi | `PrintJob {jobId, orderId, printerId, media:"4x6", createdAt, photos:[{photoId, url, contentType, copies}]}` (url = presigned GET, 15 min) |
| `fotoko/printers/{printerId}/status`    | Pi -> cloud | `JobStatus {jobId, orderId, printerId, state:"received"\|"printing"\|"completed"\|"failed", printedCopies, error?:{code,message}, at}`     |
| `fotoko/printers/{printerId}/heartbeat` | Pi -> cloud | `Heartbeat {printerId, printerState:"idle"\|"printing"\|"error"\|"offline", agentVersion, at}`; also the MQTT last will with `offline`     |

Pi error codes: `paper_out`, `ribbon_out`, `printer_offline`, `download_failed`, `invalid_image`, `driver_error`.
Pi rules: reject jobs whose `printerId` != own id; dedupe by `jobId` (persisted file); download only `https` URLs (allow `http` when `allow_insecure_http = true` for sim); max 15 MiB per image; validate magic bytes match `contentType`.

Status mapping (cloud): `received|printing` -> order `printing`; `completed` -> `completed`; `failed` -> `failed`. Heartbeat updates printer `status` + `lastSeenAt`.

## DynamoDB (single table, `TABLE_NAME`)

| Item                   | pk                      | sk     | attrs                                                                                                                        |
| ---------------------- | ----------------------- | ------ | ---------------------------------------------------------------------------------------------------------------------------- |
| Printer                | `PRINTER#{printerId}`   | `META` | name, pricePerPrintVnd, status, lastSeenAt                                                                                   |
| Order                  | `ORDER#{orderId}`       | `META` | printerId, owner, status, amountVnd, orderCode, photos, createdAt, updatedAt, expiresAt, ttl, gsi1pk=`ORDERCODE#{orderCode}` |
| Order code reservation | `ORDERCODE#{orderCode}` | `META` | orderId, ttl (written in the same transaction that first sets the code; enforces uniqueness)                                 |

GSI `gsi1` on `gsi1pk` (kept; webhook lookup reads the reservation item with a consistent read instead). GSI `gsi2` (`gsi2pk = "DUE"`, `gsi2sk = expiresAt` fixed-width RFC 3339) is set only while an order is `awaiting_*` and drives the expiry sweep. `ttl` = expiresAt + 7 days. Status updates use conditional writes on the expected current status.

S3 (`BUCKET_NAME`): `orders/{orderId}/{photoId}.{jpg|png}`. Lifecycle: delete after 7 days.

## Lambda binaries (`crates/api`)

| bin                 | trigger                                                                                                                   | build output                                |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------- |
| `fotoko-http`       | API Gateway HTTP API                                                                                                      | `target/lambda/fotoko-http/bootstrap`       |
| `fotoko-iot-status` | IoT rule `SELECT *, topic(3) AS printerId FROM 'fotoko/printers/+/+' WHERE topic(4) = 'status' OR topic(4) = 'heartbeat'` | `target/lambda/fotoko-iot-status/bootstrap` |
| `fotoko-expire`     | EventBridge `rate(5 minutes)`, reserved concurrency 1                                                                     | `target/lambda/fotoko-expire/bootstrap`     |
| `fotoko-local`      | local dev / e2e server, cargo feature `local` (not deployed)                                                              | cargo bin                                   |

Built with `cargo lambda build --release --arm64`. CDK reads `FOTOKO_LAMBDA_DIR` (default `<repo>/target/lambda`).

Env: `FOTOKO_MODE` (`aws`\|`local`), `TABLE_NAME`, `BUCKET_NAME`, `IOT_DATA_ENDPOINT`, `PAYOS_BASE_URL` (default `https://api-merchant.payos.vn`), `PAYOS_SECRET_ARN` (Secrets Manager JSON `{clientId,apiKey,checksumKey}`) or `PAYOS_CLIENT_ID`/`PAYOS_API_KEY`/`PAYOS_CHECKSUM_KEY` in local, `PUBLIC_APP_URL`, `DEFAULT_PRICE_VND` (10000).

## Pi agent config (`/etc/fotoko/agent.toml`)

```toml
printer_id = "stand-001"
[mqtt]
host = "xxxx-ats.iot.ap-southeast-1.amazonaws.com"
port = 8883
ca = "/etc/fotoko/AmazonRootCA1.pem"
cert = "/etc/fotoko/device.pem.crt"
key = "/etc/fotoko/private.pem.key"   # mode 0600
tls = true
[printer]
backend = "cups"          # or "sim"
queue = "fotoko"          # CUPS queue (Gutenprint PPD)
media = "w288h432"        # 4x6 in points
lp_bin = "lp"
lpstat_bin = "lpstat"
[sim]                     # backend = "sim"
out_dir = "/var/lib/fotoko/sim-out"
paper = 50                # sheets before paper_out
fail = ""                 # "", "offline", "ribbon_out", "driver_error"
[storage]
state_dir = "/var/lib/fotoko"
allow_insecure_http = false            # only valid with backend = "sim"
allowed_hosts = [".amazonaws.com"]     # leading dot = subdomains; ignored in sim mode
```

Optional keys (defaults): `mqtt.keep_alive_secs` (30), `mqtt.port` (8883 with TLS, 1883 without), `printer.cancel_bin` ("cancel"), `printer.print_timeout_secs` (300), `printer.poll_interval_ms` (1000), `sim.latency_ms` (0), `[agent] heartbeat_secs` (30), `download_timeout_secs` (60), `shutdown_grace_secs` (20).

Pi behaviour notes: job ledger `jobs.jsonl` in `state_dir`; a jobId is refused while in progress or once finished, except after `download_failed` (cloud republishes `job-{orderId}`); unfinished jobs found at startup are reported `failed {driver_error}`; job queue bound 32 (overflow -> `failed {driver_error, "queue full"}`); images capped at 15 MiB and 50 MP; oversized/bad magic/undecodable -> `invalid_image`. CUPS completion: after a job leaves `lpstat -o`, the agent reads `lpstat -l -W completed -o <queue>` `Alerts:` reasons; `aborted*`, `job-completed-with-errors`, `canceled*` -> `driver_error`; missing history or query failure is assumed printed (warning logged; verify wording on real CUPS). Jobs interrupted by restart/shutdown report `driver_error` and are retriable like `download_failed`. Sim output files: `{jobId}-{idx}-{photoId}-{n}.png`.
