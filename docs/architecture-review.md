# Architecture review (2026-11-05)

Produced by following the `improve-codebase-architecture` skill (`.claude/skills/improve-codebase-architecture`, source mattpocock/skills at 4ae2317ddb22, MIT; vocabulary from its sibling `codebase-design`). Terms are used as the skill defines them: **module**, **interface**, **depth**, **seam**, **adapter**, **leverage**, **locality**, plus the _deletion test_ ("would deleting it concentrate complexity, or just move it?") and "one adapter is a hypothetical seam, two is a real one". Domain terms come from `CONTEXT.md` (the skill's `GLOSSARY.md`).

Adaptations of the process, all deliberate:

- The skill writes an HTML report to the OS temp dir and asks "which would you like to explore?". Here the output is this file (reviewed in a PR), and the grilling step is left for the owner.
- It asks for a fresh-context sub-agent to walk the code; the session had no sub-agent tool, so the walk was done directly: `cargo metadata`/`cargo tree` for dependency edges, file sizes with the test split, grep of cross-crate imports, and `git log` since 2026-10-01 for hot spots (`protocol/http.rs`, `core/service`, `core/testing.rs`, `pi-agent/handler.rs` and `agent.rs`, `api/lib.rs`, the mobile upload flow and `infra/cdk/lib/{api,functions}.ts`).
- No interfaces are proposed here ("Do NOT propose interfaces yet"). **No code was changed**; the candidates are follow-ups. ADR 0001 is not contradicted by any of them.

## 1. Module boundaries: verified

| Rule                                                                                                                                                                                                 | Result                                                                                               | Enforced by                  |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- | ---------------------------- |
| `protocol` has no workspace dependency and no IO crate (only chrono, serde, serde_json, thiserror)                                                                                                   | holds                                                                                                | `scripts/ci/check_arch.py`   |
| `core -> protocol` only; no aws-sdk, axum, tokio, reqwest, rumqtt in normal deps                                                                                                                     | holds (tokio is a dev-dependency)                                                                    | check_arch                   |
| `api -> core, protocol`; `api` does not depend on `pi-agent`                                                                                                                                         | holds                                                                                                | check_arch                   |
| `pi-agent -> protocol, printer-sim`; `pi-agent` does not depend on `core` or `api`                                                                                                                   | holds                                                                                                | check_arch                   |
| `e2e` is a test harness: nothing depends on it in any dependency kind                                                                                                                                | holds                                                                                                | check_arch                   |
| Lambda graph (`fotoko-api --no-default-features`) has no `rumqttc`/`rumqttd`/`wiremock` and no `fotoko-core/testing`; only `fotoko-local` requires feature `local`; `local` is not a default feature | holds                                                                                                | check_arch                   |
| Deploy paths (`scripts/deploy`, `lambda-build.sh`, `deploy/pi`, `deploy.yml`, `infra/cdk`) never pass `--features local/testing` or `--all-features`                                                 | holds                                                                                                | check_arch                   |
| Mobile mirrors `docs/contracts.md` types and limits                                                                                                                                                  | holds today (limits and DTO shapes compared by hand: 10 photos, copies 1..=5, 15 MiB, status unions) | **nothing**, see candidate 3 |

The graph is shaped like ports-and-adapters should be: `core` owns the ports (`OrderRepo`, `PrinterRepo`, `ObjectStore`, `JobPublisher`, `PaymentGateway`, `Clock`, `IdGen`), `api::aws::*` and `api::local::*` are the two adapter families (DynamoDB/S3/IoT/Secrets Manager versus in-memory/local-fs/MQTT), so those seams are real (two adapters each). The Pi is deliberately a separate world that shares only the wire types.

## 2. Depth

Deep (keep): `fotoko-protocol` (small surface of DTOs, ids and topic functions, exact wire-format tests), `core::state::transition` (one function encodes the whole Order status machine), `core::OrderService` (every use case behind the ports; tested through its interface in `service/tests.rs`), `pi-agent::dedupe` (job ledger behind a tiny interface), `printer-sim::SimPrinter`.

Shallow or leaky: candidates below.

## Candidates

### 1. The production Pi binary links the printer simulator (Strong)

- **Files**: `crates/pi-agent/Cargo.toml`, `src/download.rs` (`use fotoko_printer_sim::probe_file`), `src/config.rs` (`use fotoko_printer_sim::Fault`), `src/sim_backend.rs`; `crates/printer-sim/Cargo.toml` (dev-depends back on `fotoko-pi-agent`).
- **Problem**: `fotoko-printer-sim` is a normal dependency of `fotoko-pi-agent`, so the release agent for real Stands carries the simulator, `image` and the fault-injection types. Part of it is not simulation at all: `probe_file` is how the Pi validates inbound images (magic bytes, 50 MP cap, `invalid_image`) and lives in the sim crate only because that is where `image` was first needed. The dev-dependency from `printer-sim` back to `pi-agent` makes a cycle that Cargo tolerates but readers do not: which crate is the library and which the consumer?
- **Solution (plain words)**: the Pi's image validation belongs to the agent; the sim backend (`backend = "sim"`) belongs behind a cargo feature of the agent that e2e/compose enable and that the release build can switch off, or `printer-sim` stays a dependency but only through a narrow interface (`Printer` implementation + `Fault`) with the probing moved out.
- **Benefits**: locality (image validation sits next to the download code that calls it and its error codes); leverage (the release binary shrinks and has less attack surface); tests keep one seam, the `Printer` trait, and the dependency direction becomes one-way.
- **Strength**: Strong. Deletion test on `printer-sim` as a dependency of the release build: the complexity that reappears is only `probe_file`, so the sim is not earning its keep there.
- **Note**: documented behaviour `backend = "sim"` on a real Pi for bench tests would need the feature on in that build; decide with the owner.

### 2. `OrderService` needs every port, so two Lambdas get stub ports (Worth exploring)

- **Files**: `crates/api/src/aws/unavailable.rs`, `crates/core/src/service/mod.rs` (`Ports`), `crates/api/src/bin/fotoko-iot-status.rs`, `fotoko-expire.rs`.
- **Problem**: the status Lambda and the expire Lambda only touch the repositories, but `OrderService` is built from the full `Ports` bundle, so they construct "unavailable" adapters that fail at runtime ("the other ports fail loudly"). The interface of `OrderService` is wide (customer flow + status ingestion + expiry), and the seam for "which ports does this caller need" is a runtime error rather than a type.
- **Solution**: split the use cases by what ports they need (customer flow with all ports; status ingestion and expiry with the repositories and clock only) so the type system says what a Lambda can do, and the stub adapter module disappears.
- **Benefits**: locality (a Lambda's capability is visible in its constructor); fewer adapters to maintain; tests of expiry no longer fake payment or object-store ports.
- **Strength**: Worth exploring (moderate churn in `service/mod.rs`, 675 lines, and in `service/tests.rs`, 1672 lines).

### 3. The contract is mirrored by hand in three places (Strong, cheap)

- **Files**: `crates/protocol/src/{http,mqtt,topics,limits}.rs`, `apps/mobile/src/api/types.ts` and `src/lib/limits.ts`, `infra/cdk/lib/iot.ts` (`TOPIC_ROOT`, rule SQL, IAM ARNs) and `config.ts` (price, payOS URL), `.env.example` defaults.
- **Problem**: `docs/contracts.md` is authoritative, but only the Rust side has exact wire-format tests. The TypeScript DTOs say "keep in sync" in a comment; the topic root `fotoko/printers` and the default price 10000 are literals in Rust, CDK and docs. They agree today; nothing fails when they stop agreeing, and the failure would show up in production as a 400, a denied publish or a wrong price.
- **Solution**: make the protocol crate's golden JSON (one fixture per message and per HTTP shape) the shared artifact: Rust round-trip tests read them, a mobile jest test parses them through the DTO types/validators, a CDK test asserts the topic root against the fixture. Whether to generate TS types from Rust (check current docs of `ts-rs`/`typeshare` first) is a second step.
- **Benefits**: leverage (one fixture set is the test surface for three languages); locality of contract changes (change `contracts.md`, regenerate fixtures, the three suites tell you what broke).
- **Strength**: Strong; no behaviour change.

### 4. The test-support adapters are also the local server's adapters (Worth exploring)

- **Files**: `crates/core/src/testing.rs` (994 lines, feature `testing`), `crates/api/Cargo.toml` (`local = ["fotoko-core/testing", ...]`), `crates/api/src/local/*`, `crates/e2e`.
- **Problem**: the in-memory adapters are not only test doubles: `fotoko-local` (compose stack, e2e, Playwright) is built on them. The feature name `testing` hides that, and the safety property "never in a Lambda" rests on cargo feature unification not leaking (`--all-features` builds in CI are fine, a deploy script using them would not be). The property is now checked (`check_arch.py`), but it is a guard around a shape that invites the mistake.
- **Solution**: name the thing for its role (an in-memory adapter set), and decide whether it is a feature of `core`, a sibling crate (`fotoko-memory`, which `fotoko-local` and tests depend on and Lambdas cannot), or part of `api::local`. A sibling crate would make the rule structural: `api` would need it only under `local`, and `check_arch` becomes a one-line edge rule.
- **Benefits**: locality of the "not in production" property; clearer meaning of `core`'s feature list.
- **Strength**: Worth exploring. Two adapters (Dynamo, in-memory) exist, so the seam is real; this is about where the second adapter lives.

### 5. Big files are mostly inline tests (Speculative, hygiene)

`api/src/api.rs` (314 lines of code, 764 of tests), `pi-agent/src/cups.rs` (597/578), `handler.rs` (355/705), `printer-sim/src/fake_cups.rs` (699/462), `download.rs` (434/518), `aws/dynamo.rs` (654/231). The modules themselves have small interfaces; the size makes navigation and review heavy for agents (a 1,000-line read to reach 300 lines of code). `core/src/service` already uses a `tests.rs` sibling. Move inline tests to `tests.rs` siblings when they exceed the code (rule added to AGENTS.md); no restructuring of the modules is needed. `cups.rs` mixes command execution with `lpstat` output parsing (both ends of one seam, the process runner): worth a look when it next changes.

### Considered and not proposed

- `core::payos` (signature creation and verification) inside the domain crate: it is pure and it is the trust boundary of the order flow, so it concentrates complexity (deletion test: moving it to `api` would spread the rules about which fields are signed across the webhook handler and the mock).
- `AppState` holding `Arc<RwLock<OrderService>>` plus a `ServiceRefresher`: unusual but small, with one reason to exist (re-read the payOS secret after a failed signature).
- `fotoko-e2e` depends on almost everything: that is its job, and nothing depends on it.

## Top recommendation

Do **candidate 3 first** (golden contract fixtures): smallest change, no behaviour risk, closes the only boundary in the table above that nothing enforces. Then **candidate 1** (Pi binary vs simulator), which affects what is installed on Stands in the field and removes the dependency cycle. Candidates 2 and 4 share files in `core` and should be explored together.

## Proposed follow-ups (not implemented)

1. Golden JSON fixtures in `crates/protocol/tests/fixtures/` consumed by Rust, mobile and CDK tests (candidate 3).
2. Move `probe_file` into `pi-agent`; feature-gate the sim backend; drop the `printer-sim -> pi-agent` dev-dependency by moving the integration tests that need both into `pi-agent/tests` or `crates/e2e` (candidate 1). Then tighten `ALLOWED` in `check_arch.py` (`pi-agent` -> `printer-sim` only under the feature).
3. Split `OrderService` use cases by required ports; delete `aws/unavailable.rs` (candidate 2).
4. Decide the home of the in-memory adapters; if a sibling crate, add it to `ALLOWED` (candidate 4).
5. Move inline tests of files over ~800 lines into `tests.rs` siblings (candidate 5).
6. Enforce further in `check_arch.py` once the above land: no `println!`-style IO in `core`, a size budget for non-test code per file.
