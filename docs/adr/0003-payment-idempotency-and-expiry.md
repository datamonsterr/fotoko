# ADR 0003: Payment idempotency and expiry

Status: accepted (2026-11-05)

## Decision

- The paid decision uses the signed `data.code == "00"` of the payOS webhook; the outer `code` is unsigned and only cross-checked. Null values sign as empty strings.
- Order code uniqueness: an `ORDERCODE#{code}` / `META` reservation item is written in the same DynamoDB transaction that first sets the code (`attribute_not_exists`). The webhook resolves the Order with a consistent read of that item; `gsi1` is kept but not read.
- Duplicate webhooks: while the Order is `paid` the same Print job is republished under the stable `job-{orderId}`; after `printing` a duplicate is a no-op. The Pi job ledger refuses a `jobId` that is in progress or finished, except after `download_failed` or an interrupted run, so republishes after those failures print.
- Expiry: unpaid Orders expire 15 min after creation. `gsi2` (`gsi2pk = "DUE"`, `gsi2sk = expiresAt`) is set only while `awaiting_*`. `fotoko-expire` sweeps it every 5 minutes (reserved concurrency 1); reads also expire lazily.
- payOS checksum key rotation: on a signature failure the API re-reads the secret at most once per 60 s and retries verification once.

## Consequences

Webhook handling is safe under payOS retries and concurrent deliveries. Every update of an Order with a code is a conditional Put; the first code assignment is a transaction.
