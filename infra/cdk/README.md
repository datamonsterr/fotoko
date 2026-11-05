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
| `lib/monitoring.ts` | Lambda error and API 5xx alarms, SNS topic (no subscription)                  |
| `lib/nag.ts`        | cdk-nag `AwsSolutionsChecks` (acknowledgments live next to each resource)     |

## Commands

```sh
pnpm --filter @fotoko/infra lint | typecheck | test
pnpm --filter @fotoko/infra synth      # cdk synth
pnpm --filter @fotoko/infra diff
pnpm --filter @fotoko/infra deploy
```

Stage: `-c stage=prod` or `FOTOKO_STAGE=prod` (default `dev`). Extra CORS origins: `-c extraOrigins=https://a,https://b`. Upload the Expo web export with `-c deployWeb=true` (needs `apps/mobile/dist`).

### Lambda binaries

CDK reads `fotoko-http/bootstrap` and `fotoko-iot-status/bootstrap` from `FOTOKO_LAMBDA_DIR`. A relative value is resolved from the repository root; the default is `<repo>/target/lambda` (the `cargo lambda build --release --arm64` output). Without binaries, synth with the test fixtures:

```sh
FOTOKO_LAMBDA_DIR=infra/cdk/test/fixtures/lambda pnpm --filter @fotoko/infra synth
```

### Provision a printer

```sh
pnpm --filter @fotoko/infra provision-printer --printer-id stand-001 --stage dev --out ./printer-out/stand-001 [--dry-run]
```

Creates the IoT thing and certificate, attaches the device policy, and writes `device.pem.crt`, `private.pem.key` (0600), `AmazonRootCA1.pem` and `agent.toml`. Needs AWS credentials. Copy the files to `/etc/fotoko/` on the Pi.

### After the first deploy

Replace the placeholders in the payOS secret (`clientId`, `apiKey`); `checksumKey` is generated, overwrite it with the payOS value too. Subscribe operators to the `fotoko-<stage>-alarms` SNS topic.

## Notes

- Thing types are unique per account and region: prod owns `fotoko-printer`, other stages use `fotoko-printer-<stage>`.
- cdk-nag v3 acknowledgments are per finding (`Validations.of(...).acknowledge`), so wildcard ARN ids embed logical ids and are computed in code.
