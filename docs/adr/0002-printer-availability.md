# ADR 0002: Printer availability and queueing

Status: accepted (2026-11-05)

## Context

A Stand serves walk-up customers. While one Order prints (tens of seconds per 4x6 page), others should still be able to pay.

## Decision

- Orders are accepted when the Printer status is `online` or `busy`; `offline` and `error` are rejected (`validation`).
- Heartbeat mapping: `idle` -> `online`, `printing` -> `busy`, `error` -> `error`, `offline` -> `offline`; the MQTT last will publishes `offline`.
- The Pi prints one Print job at a time from a bounded queue of 32; overflow reports `failed {driver_error, "queue full"}`.
- A payment that arrives for a `cancelled` or `expired` Order is acknowledged (200), logged at warn and refunded manually. Refunds are not automated yet.
- A Print job interrupted by a Pi restart or shutdown is reported `failed {driver_error}`; the Order stays `failed` and needs a manual reprint or refund.

## Consequences

- The mobile app shows a busy Printer as bookable with a "will print next" note.
- Manual operator procedures exist for late payments and interrupted jobs (see `docs/setup/hardware.md`). An operator retry flow is future work.
