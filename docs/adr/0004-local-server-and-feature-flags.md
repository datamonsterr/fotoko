# ADR 0004: Local server, simulation and feature flags

Status: accepted (2026-11-05)

## Decision

- `fotoko-core` feature `testing` provides in-memory adapters for tests and for the local server.
- `fotoko-api` feature `local` (non-default) gates `fotoko_api::local` and the `fotoko-local` bin (`required-features`). Lambda builds use `--no-default-features`; no deploy path enables `local`, `testing` or `--all-features`.
- Simulation runs without AWS, payOS or a printer: `fotoko-local` + `fotoko-sim-broker` (rumqttd) + `fotoko-payos-mock` + Pi agent with `backend = "sim"` (`fotoko-printer-sim`). `allow_insecure_http` is only valid with the sim backend.
- `scripts/ci/check-arch.sh` (CI job `arch`) enforces the dependency rules in AGENTS.md "Architecture rules", including the Lambda feature graph.

## Consequences

- The production Pi binary still links `fotoko-printer-sim` for `probe_file` and `Fault`; moving image validation into the agent is the first follow-up (`docs/architecture-review.md` candidate 1).
- Using core's `testing` adapters for the local server is a deliberate shortcut (review candidate 4).
