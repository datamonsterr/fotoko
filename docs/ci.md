# CI/CD

One workflow checks every pull request and every push to `main`: `.github/workflows/ci.yml`. The only required check is **`ci-ok`**. It fails when any job failed or was cancelled; skipped jobs count as passed, so a docs-only PR is green after `format` alone.

## Job map

Every job runs `bash scripts/ci/repro.sh <job>`. The job id, the repro name and the row below are the same string.

| Job                  | Runs when                              | What                                                                                                               | Local                                             |
| -------------------- | -------------------------------------- | ------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------- |
| `changes`            | always                                 | path filter; outputs `rust`, `mobile`, `infra`, `e2e`, `shell`, `workflows`                                        | -                                                 |
| `rust-lint`          | rust                                   | `cargo fmt --check`, clippy `--workspace --all-targets --all-features -D warnings`                                 | `repro.sh rust-lint`                              |
| `rust-matrix`        | rust                                   | builds the `rust-test` matrix from `cargo metadata`                                                                | `python3 -I scripts/ci/cargo_meta.py test-matrix` |
| `rust-test`          | rust                                   | one entry per workspace crate: `cargo nextest run -p X --all-features --profile ci`, plus doctests                 | `repro.sh rust-test [crate]`                      |
| `rust-integration`   | rust, `crates/api`, compose file exist | `docker-compose.test.yml` (DynamoDB Local, MinIO), `FOTOKO_IT=1 cargo nextest run -p fotoko-api --run-ignored all` | `FOTOKO_IT=1 repro.sh rust-integration`           |
| `rust-e2e`           | e2e area, `crates/e2e` exists          | `cargo nextest run -p fotoko-e2e`                                                                                  | `repro.sh rust-e2e`                               |
| `deny`               | rust                                   | `cargo deny --all-features check`                                                                                  | `repro.sh deny`                                   |
| `pi-cross-build`     | rust                                   | `fotoko-pi-agent` for `aarch64-unknown-linux-gnu` with `gcc-aarch64-linux-gnu`; uploads the binary (3 days)        | `repro.sh pi-cross-build`                         |
| `lambda-build`       | rust, `crates/api` exists              | `cargo lambda build --release --arm64` for the `fotoko-api` bins without extra features; uploads (3 days)          | `repro.sh lambda-build`                           |
| `js` (mobile, infra) | the package changed                    | `turbo run lint typecheck test --filter=@fotoko/<pkg>`                                                             | `repro.sh js [pkg]`                               |
| `format`             | always                                 | `pnpm format:check` (Prettier)                                                                                     | `repro.sh format`                                 |
| `cdk-synth`          | infra                                  | `cdk synth` with the fixture Lambda dir                                                                            | `repro.sh cdk-synth`                              |
| `web-e2e`            | e2e area, `packages/e2e-web` exists    | Playwright (`test:e2e`); uploads the report on failure                                                             | `repro.sh web-e2e`                                |
| `shellcheck`         | shell                                  | `shellcheck -x` on every tracked `*.sh`                                                                            | `repro.sh shellcheck`                             |
| `actionlint`         | workflows                              | actionlint 1.7.12 (also shellchecks `run:` blocks)                                                                 | `repro.sh actionlint`                             |
| `ci-ok`              | always                                 | aggregates every job above                                                                                         | -                                                 |

Areas a PR touches (`changes`): `rust` is `crates/**`; `mobile` is `apps/mobile/**` and `packages/**`; `infra` is `infra/**`; `e2e` is crates, the app and the e2e paths; `shell` is `*.sh`; `workflows` is `.github/**`. A change to root config (`Cargo.toml`/`Cargo.lock`, `rust-toolchain.toml`, `deny.toml`, `.config/**`, `package.json`, `pnpm-*`, `turbo.json`, `.nvmrc`, Prettier config, `scripts/ci/**`, `ci.yml`, the local actions) selects everything. Pushes to `main` and manual runs always select everything.

## Adding crates, packages and jobs

- **New crate** under `crates/*`: it joins `rust-lint`, `rust-test`, `deny` automatically. A crate that needs its own job (like `fotoko-e2e`) goes into `OWN_JOB` in `scripts/ci/cargo_meta.py`.
- **`crates/api`** (T6) and its non-default feature `local`: tests run with `--all-features`; the Lambda build uses `--no-default-features` and the bins from `cargo_meta.py lambda-bins` (bins with `required-features`, such as `fotoko-local`, are skipped).
- **`crates/e2e`, `packages/e2e-web`, `docker-compose.test.yml`**: `changes` checks for them. While they are missing their jobs are skipped and a notice explains why; once the files exist the jobs run, with no workflow edit. `web-e2e` runs `pnpm --filter ./packages/e2e-web run test:e2e`, so the package's script must start whatever the test needs.
- **New JS package**: add it to the `changes` filters and to the default list in `job_js` in `scripts/ci/repro.sh`.
- **New job**: add a `job_<name>` function to `repro.sh`, call it from the job, and add it to `ci-ok.needs`.

## Fast feedback

- PR runs cancel superseded runs (`concurrency` per ref); runs on `main` are never cancelled.
- Rust tests run one matrix entry per crate, in parallel. nextest prints a failure's output once, at the end (`failure-output = "final"` in `.config/nextest.toml`), and only failures (`status-level = "fail"`). Clippy prints `--message-format short` in CI. Turbo prints logs of failed tasks only (`--output-logs=errors-only`).
- Rust cache (`Swatinem/rust-cache`): one `shared-key` per job kind (`lint`, `test-<crate>`, `e2e`, `pi-aarch64`, `lambda`), so a cache is not duplicated per job id. Only runs on `main` save caches; PRs restore them. Cache-explosion guard: the keys are bounded by crate count, not by branch.
- Every job has `timeout-minutes`, every matrix has `fail-fast: false`, so one failing crate does not hide the others.
- Third-party actions are pinned to commit SHAs with the version in a comment; Dependabot (`github-actions`) updates them weekly.

## How an agent iterates

1. Push the branch and open the PR. `ci-ok` and the other jobs start.
2. Open the failed job's **summary** (or the `ci-ok` summary, which lists every job and the repro command of each failed one). A failing Rust test is listed with `cargo nextest run -p <crate> --profile ci -E 'test(=<name>)'`.
3. Run that command locally and fix. For a whole job: `bash scripts/ci/repro.sh <job>`. Set `CI=1` for the same concise output as CI.
4. Before pushing again: `bash scripts/ci/fast.sh` (changed scope). `bash scripts/ci/all.sh` mirrors the full workflow; it skips jobs whose tool is missing (cargo-deny, shellcheck, actionlint, cargo-lambda, aarch64 gcc) and prints what it skipped.

Tools worth installing locally: `cargo install cargo-nextest cargo-deny --locked`, `shellcheck`, `actionlint`.

## Deploy

`.github/workflows/deploy.yml` deploys stage `dev` when the `CI` workflow completes successfully for a push to `main` (`workflow_run`, so a red `ci-ok` never deploys) and on manual dispatch (stage `dev` or `prod`). It assumes the AWS role in `vars.AWS_DEPLOY_ROLE_ARN` through GitHub OIDC and runs `scripts/deploy/aws.sh <stage>`. Runs of one stage queue and are never cancelled.

Setup, once per AWS account:

- GitHub environments `dev` and `prod`; add required reviewers to `prod`.
- An IAM role trusting `token.actions.githubusercontent.com` for this repository (limit the `sub` claim to `repo:<owner>/<repo>:environment:<stage>`) with the rights CDK needs; set its ARN as the `AWS_DEPLOY_ROLE_ARN` variable of each environment.
- `cdk bootstrap` for the account and `ap-southeast-1`.

`scripts/deploy/aws.sh [--dry-run] <dev|prod>`: build the Lambdas (`cargo lambda build`), build the Expo web export with `EXPO_PUBLIC_*` from the environment or the stack outputs, `cdk deploy -c stage=<stage> -c deployWeb=true`, print the outputs. On the first deploy the stack has no outputs yet, so it is deployed once without web assets, then again with them. `--dry-run` prints every command and runs none.

The deploy checks out the commit CI tested (`workflow_run.head_sha`). Also protect `main` with the `ci-ok` required check.

## Dependency advisories

`deny.toml` ignores six advisories (RUSTSEC-2026-0049, -0098, -0099, -0104, -0382, RUSTSEC-2025-0134). `rumqttc 0.25.1` and `rumqttd 0.20.0`, the latest releases on crates.io (checked 2026-11-05), still depend on `rustls-webpki 0.102.x` and `rustls-pemfile`. Tracking: re-check for a rumqttc release on `rustls-webpki >= 0.103.16` by 2027-02-05 (or sooner when Dependabot opens one), upgrade, and delete the ignore entries. Do not add new ignores without a reason and a review date.
