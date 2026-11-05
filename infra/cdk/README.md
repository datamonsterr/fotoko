# @fotoko/infra

AWS CDK v2 app (TypeScript) for Fotoko: one stack per stage (`Fotoko-dev`, `Fotoko-prod`) in `ap-southeast-1`. Interfaces come from `docs/contracts.md`.

| File                | Concern                                                                       |
| ------------------- | ----------------------------------------------------------------------------- |
| `lib/data.ts`       | DynamoDB table, photos bucket, payOS secret                                   |
| `lib/web.ts`        | Web bucket + CloudFront (OAC), SPA fallback, optional `BucketDeployment`      |
| `lib/functions.ts`  | `fotoko-http` and `fotoko-iot-status` Lambdas and their least-privilege roles |
| `lib/api.ts`        | HTTP API, routes and IAM authorizer, throttling, access logs                  |
| `lib/auth.ts`       | Cognito guest identity pool and role, placeholder operator user pool          |
| `lib/iot.ts`        | IoT data endpoint lookup, thing type, device policy, status topic rule        |
| `lib/monitoring.ts` | Lambda error/throttle, IoT rule failure and API 5xx alarms, SNS topic         |
| `lib/nag.ts`        | cdk-nag `AwsSolutionsChecks` (acknowledgments live next to each resource)     |

## Commands

```sh
pnpm --filter @fotoko/infra lint | typecheck | test
pnpm --filter @fotoko/infra synth      # cdk synth
pnpm --filter @fotoko/infra diff
pnpm --filter @fotoko/infra deploy
```

Stage: `-c stage=prod` or `FOTOKO_STAGE=prod` (default `dev`). Extra CORS origins: `-c extraOrigins=https://a,https://b` (https, or http for localhost; no wildcard or path). Alarm email: `-c alarmEmail=ops@example.com` (otherwise the topic has no subscription). Upload the Expo web export with `-c deployWeb=true` (needs `apps/mobile/dist`).

### Lambda binaries

CDK reads `fotoko-http/bootstrap` and `fotoko-iot-status/bootstrap` from `FOTOKO_LAMBDA_DIR`. A relative value is resolved from the repository root; the default is `<repo>/target/lambda` (the `cargo lambda build --release --arm64` output). Without binaries, synth with the test fixtures:

```sh
FOTOKO_LAMBDA_DIR=infra/cdk/test/fixtures/lambda pnpm --filter @fotoko/infra synth
```

### Provision a printer

```sh
pnpm --filter @fotoko/infra provision-printer --printer-id stand-001 --stage dev --out ./printer-out/stand-001 [--dry-run] [--rotate] [--force]
```

Creates the IoT thing and certificate, attaches the device policy, and writes `device.pem.crt`, `private.pem.key` (0600), `AmazonRootCA1.pem` and `agent.toml`. Needs AWS credentials. Copy the files to `/etc/fotoko/` on the Pi.

- The endpoint and root CA are fetched before anything is created. If a later step fails, the certificate, attachments and a thing created in this run are rolled back; rollback failures are listed in the error.
- An existing thing is refused unless `--rotate`: the new certificate is attached first, then the old ones are detached, deactivated and deleted. Rotation retires the old certificate immediately, so the Pi goes offline until the new files are installed: back up the current files first and copy the new ones to the Pi promptly. Existing files in the output directory are refused unless `--force`.

### After the first deploy

Replace the placeholders in the payOS secret (`clientId`, `apiKey`); `checksumKey` is generated, overwrite it with the payOS value too. Subscribe operators to the `fotoko-<stage>-alarms` SNS topic (or deploy with `-c alarmEmail=...`).

## Notes

- Thing types are unique per account and region: prod owns `fotoko-printer`, other stages use `fotoko-printer-<stage>`.
- The SPA fallback returns `index.html` with 200 for any 403/404 from the web bucket, so a missing asset is not reported as 404.
- No operator user pool yet: add one with the operator features.
- Known follow-ups: WAF in front of the API and CloudFront, and reserved concurrency on the Lambdas (needs a raised account concurrency quota). Per-route throttling is in place meanwhile.
- cdk-nag v3 acknowledgments are per finding (`Validations.of(...).acknowledge`), so wildcard ARN ids embed logical ids and are computed in code.
