# AWS setup from zero

Everything Fotoko needs on AWS, in the order to do it. Region is always **ap-southeast-1** (Singapore). One CloudFormation stack per stage: `Fotoko-dev`, `Fotoko-prod`. Variables and what they mean: `.env.example`. Hardware and the Pi: [hardware.md](hardware.md). Other services (payOS, Expo, GitHub): [services.md](services.md).

Check commands against the current AWS documentation before relying on them; the OIDC and Identity Center steps in particular change in the console from time to time. (This guide was written without access to docs.aws.amazon.com from the authoring sandbox, so the console click paths are from memory and the CLI/JSON are cross-checked against the repo only.)

## 1. Accounts

Best practice is **two accounts, `fotoko-dev` and `fotoko-prod`**, under one AWS Organization. A mistake in dev then cannot touch prod data, IAM or payOS secrets, and the GitHub OIDC role of each account is trusted only for its own environment. The minimum acceptable setup is **one account with both stages** (`Fotoko-dev` and `Fotoko-prod` stacks side by side): it works because every resource name carries the stage, but IAM and budgets are shared, so you lose the blast-radius separation. Everything below says "per account"; with one account do it once.

1. Create the management account with a dedicated email (a mailbox you control, not a person's name), then **AWS Organizations -> Add an AWS account -> Create** for `fotoko-dev` and `fotoko-prod` (separate email addresses, or `+dev`/`+prod` aliases if your provider supports them).
2. **Root user hygiene, in every account**: enable MFA on the root user (hardware key or authenticator app), set a long unique password in a password manager, delete any root access keys, and never use root for work. Add alternate contacts (billing, security, operations) in Account settings.
3. Enable **AWS IAM Identity Center** in the management account, region ap-southeast-1 (keep the Identity Center region the same as the work region to avoid surprises). Create a group `fotoko-admins` and your user, assign the group to the `fotoko-dev` and `fotoko-prod` accounts with the `AdministratorAccess` permission set (tighten to `PowerUserAccess` plus IAM later if you wish). Require MFA for the user at sign-in.
4. Configure the CLI with SSO instead of long-lived keys:

   ```sh
   aws configure sso            # session name fotoko, start URL from Identity Center, region ap-southeast-1
   aws sso login --profile fotoko-dev
   export AWS_PROFILE=fotoko-dev AWS_REGION=ap-southeast-1
   aws sts get-caller-identity  # shows the account you are about to change
   ```

   Install the AWS CLI v2 ([install guide](https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html)), Node 22 + pnpm (repo root `pnpm install`) and Rust with `cargo-lambda` (`pip3 install cargo-lambda`).

## 2. Budgets, alerts and quotas

1. **Budgets** (Billing -> Budgets, per account): a monthly cost budget with alerts at 50%, 80% and 100% of actual and 100% forecast, emailed to the ops address. A kiosk at this scale costs a few USD a month; set the budget accordingly (for example 20 USD dev, 50 USD prod). Also turn on **Cost Anomaly Detection** and **Billing preferences -> Receive billing alerts**.
2. **Operational alarms**: deploy with `-c alarmEmail=ops@example.com` (or subscribe to the SNS topic `fotoko-<stage>-alarms` by hand and confirm the email). Alarms cover Lambda errors and throttles, IoT rule failures and API 5xx (`infra/cdk/lib/monitoring.ts`).
3. **Lambda concurrency quota**: `fotoko-expire` reserves concurrency 1, and Lambda requires the account to keep at least **10 unreserved executions**. A brand-new account has a total of 10, so the first `cdk deploy` fails until you raise the quota (infra/cdk/README.md, "Known follow-ups"):

   ```sh
   aws service-quotas get-service-quota --service-code lambda --quota-code L-B99A9384 --query Quota.Value
   aws service-quotas request-service-quota-increase --service-code lambda --quota-code L-B99A9384 --desired-value 1000
   ```

   New accounts are often held at the low value for some days until AWS has seen usage; open a support case if the increase is pending. Do this on day one, in both accounts.

## 3. Bootstrap CDK

Once per account and region, with admin credentials (local, not CI):

```sh
pnpm install
export AWS_PROFILE=fotoko-dev
pnpm --filter @fotoko/infra exec cdk bootstrap aws://<ACCOUNT_ID>/ap-southeast-1
```

This creates the `CDKToolkit` stack with the default qualifier `hnb659fds`: the asset bucket and the roles `cdk-hnb659fds-{deploy,file-publishing,image-publishing,lookup}-role-<ACCOUNT_ID>-ap-southeast-1` and `cdk-hnb659fds-cfn-exec-role-...`. The deploy role of GitHub (below) only needs to assume these.

## 4. GitHub OIDC provider and deploy role

CI deploys without stored keys: GitHub issues a short-lived OIDC token for the job and AWS trades it for role credentials (`aws-actions/configure-aws-credentials` in `.github/workflows/deploy.yml`, role from the `AWS_DEPLOY_ROLE_ARN` variable). Do this **in each account**.

1. Create the identity provider (IAM -> Identity providers -> Add provider -> OpenID Connect): provider URL `https://token.actions.githubusercontent.com`, audience `sts.amazonaws.com`. CLI form:

   ```sh
   aws iam create-open-id-connect-provider \
     --url https://token.actions.githubusercontent.com \
     --client-id-list sts.amazonaws.com
   ```

   AWS validates the GitHub certificate chain itself; a thumbprint is no longer needed (the CLI accepts and ignores one for older tooling).

2. Create the role. Trust policy for the **prod** account, scoped to repository `datamonsterr/fotoko` and the GitHub environment `prod` (the `sub` claim of a job that uses `environment: prod`). Save as `trust-prod.json`, replacing `<ACCOUNT_ID>`:

   ```json
   {
     "Version": "2012-10-17",
     "Statement": [
       {
         "Effect": "Allow",
         "Principal": {
           "Federated": "arn:aws:iam::<ACCOUNT_ID>:oidc-provider/token.actions.githubusercontent.com"
         },
         "Action": "sts:AssumeRoleWithWebIdentity",
         "Condition": {
           "StringEquals": {
             "token.actions.githubusercontent.com:aud": "sts.amazonaws.com",
             "token.actions.githubusercontent.com:sub": "repo:datamonsterr/fotoko:environment:prod"
           }
         }
       }
     ]
   }
   ```

   For the dev account use `repo:datamonsterr/fotoko:environment:dev`. With one shared account create two roles (one per environment) so that `dev` jobs can never assume the prod role. Use `StringEquals` with the exact `sub`; a `StringLike` wildcard such as `repo:datamonsterr/*` would let every repository of the owner deploy.

3. Permissions policy (`fotoko-deploy-policy`): the role only assumes the CDK bootstrap roles (they hold the real rights, CloudFormation executes with `cfn-exec-role`) and reads stack outputs for `aws.sh`:

   ```json
   {
     "Version": "2012-10-17",
     "Statement": [
       {
         "Sid": "AssumeCdkRoles",
         "Effect": "Allow",
         "Action": "sts:AssumeRole",
         "Resource": "arn:aws:iam::<ACCOUNT_ID>:role/cdk-hnb659fds-*-role-<ACCOUNT_ID>-ap-southeast-1"
       },
       {
         "Sid": "ReadStackOutputs",
         "Effect": "Allow",
         "Action": "cloudformation:DescribeStacks",
         "Resource": "arn:aws:cloudformation:ap-southeast-1:<ACCOUNT_ID>:stack/Fotoko-*/*"
       }
     ]
   }
   ```

   ```sh
   aws iam create-role --role-name fotoko-github-deploy-prod --assume-role-policy-document file://trust-prod.json
   aws iam put-role-policy --role-name fotoko-github-deploy-prod --policy-name fotoko-deploy --policy-document file://deploy-policy.json
   aws iam get-role --role-name fotoko-github-deploy-prod --query Role.Arn --output text
   ```

   `cdk deploy` runs as the bootstrap deploy role; the `cfn-exec-role` default is `AdministratorAccess`. If you narrow it later (`cdk bootstrap --cloudformation-execution-policies`), the stack creates IAM roles, IoT policies, CloudFront, Cognito, DynamoDB, S3, Lambda, EventBridge, Secrets Manager, SNS, CloudWatch and a custom resource calling `iot:DescribeEndpoint`.

4. Sanity check the trust policy from CI later with the first deploy; an `Not authorized to perform sts:AssumeRoleWithWebIdentity` error means the `sub` does not match (wrong environment name, repository rename, or the job does not declare `environment:`).

## 5. GitHub repository settings

Repository `datamonsterr/fotoko` -> Settings.

1. **Environments** `dev` and `prod` (the deploy workflow names them). In each: **Deployment branches and tags -> Selected branches -> `main` only**; in `prod` add **Required reviewers** (yourself plus one more person if possible). Both deploy jobs use the environment, so a prod deploy asks for approval twice.
2. **Variables**: in each environment add `AWS_DEPLOY_ROLE_ARN` = the role ARN of that account (Environments -> dev -> Environment variables). It is not a secret. No AWS access keys and no repository secrets are needed.
3. **Branch protection** (Settings -> Branches -> rule for `main`): require a pull request, require status check **`ci-ok`** (the only required check; skipped jobs count as passed), require branches to be up to date, block force pushes and deletions. A green `CI` run on a push to `main` triggers the dev deploy; prod deploys come only from the manual **Deploy** workflow (Actions -> Deploy -> Run workflow -> stage `prod`, on `main`).
4. Actions permissions: allow actions from this repository and the pinned third-party actions the workflows use; Dependabot (`.github/dependabot.yml`) keeps the pins current.

Background: `docs/ci.md` ("Deploy").

## 6. First deploy

From a machine with admin credentials for the account (or let CI do it after step 5):

```sh
export AWS_PROFILE=fotoko-dev AWS_REGION=ap-southeast-1
bash scripts/deploy/aws.sh --dry-run all dev     # prints every command, runs none
bash scripts/deploy/aws.sh all dev               # build Lambdas, cdk deploy, build web, cdk deploy with the web assets
```

Add `-c alarmEmail=...` by running `cdk deploy` yourself (`pnpm --filter @fotoko/infra exec cdk deploy Fotoko-dev -c stage=dev -c alarmEmail=ops@example.com ...`; see `infra/cdk/README.md`). The outputs you need afterwards (also `aws cloudformation describe-stacks --stack-name Fotoko-dev --query 'Stacks[0].Outputs' --output table`):

| Output           | Use                                                                 |
| ---------------- | ------------------------------------------------------------------- |
| `ApiUrl`         | `EXPO_PUBLIC_API_URL`, payOS webhook base                           |
| `WebUrl`         | CloudFront URL of the web app, QR sticker base, `PUBLIC_APP_URL`    |
| `IdentityPoolId` | `EXPO_PUBLIC_COGNITO_IDENTITY_POOL_ID`                              |
| `TableName`      | DynamoDB table (`fotoko-<stage>`)                                   |
| `BucketName`     | photo bucket                                                        |
| `IotEndpoint`    | MQTT host in the Pi's `agent.toml` (written by `provision-printer`) |

## 7. payOS secret

CDK creates the secret with placeholders (`clientId` and `apiKey` = `REPLACE_ME`, a generated `checksumKey`). Overwrite all three with the values from the payOS dashboard ([services.md](services.md)). The secret is a JSON object; the Lambda reads it at start and re-reads it (at most once a minute) after a failed signature check, so a rotation needs no redeploy.

```sh
export STAGE=dev
SECRET_ARN=$(aws lambda get-function-configuration --function-name "fotoko-$STAGE-fotoko-http" \
  --query 'Environment.Variables.PAYOS_SECRET_ARN' --output text)
read -r -s -p 'payOS Client ID: ' PAYOS_CLIENT_ID; echo
read -r -s -p 'payOS API Key: ' PAYOS_API_KEY; echo
read -r -s -p 'payOS Checksum Key: ' PAYOS_CHECKSUM_KEY; echo
aws secretsmanager put-secret-value --secret-id "$SECRET_ARN" --secret-string \
  "$(jq -nc --arg c "$PAYOS_CLIENT_ID" --arg a "$PAYOS_API_KEY" --arg k "$PAYOS_CHECKSUM_KEY" \
     '{clientId:$c, apiKey:$a, checksumKey:$k}')"
unset PAYOS_CLIENT_ID PAYOS_API_KEY PAYOS_CHECKSUM_KEY
```

`read -s` keeps the values out of shell history and the process list; do not paste secrets into `--secret-string` literally. Never put them into git, GitHub variables or `.env` files that get committed.

## 8. Register the payOS webhook

The webhook URL is `<ApiUrl>/v1/payments/payos/webhook` (public route, authenticated by the payOS HMAC signature, no IAM). `ApiUrl` ends with a slash:

```sh
API_URL=$(aws cloudformation describe-stacks --stack-name Fotoko-dev --query "Stacks[0].Outputs[?OutputKey=='ApiUrl'].OutputValue" --output text)
echo "${API_URL%/}/v1/payments/payos/webhook"
```

Paste it into the payOS dashboard (payment channel -> webhook URL) or call payOS's confirm-webhook API; see [services.md](services.md). payOS probes the URL with a sample payload when you save it: the API answers `200 {"success":true}` for an unknown `orderCode`, which makes the confirmation succeed (docs/contracts.md, "Webhook handling").

## 9. Seed a printer

The API only accepts orders for printers that exist in DynamoDB. Item format (docs/contracts.md "DynamoDB", decoded by `crates/api/src/aws/dynamo.rs`): `pk=PRINTER#<printerId>`, `sk=META`, `name`, `pricePerPrintVnd` (number, falls back to `DEFAULT_PRICE_VND`), `status` (`online|offline|busy|error`). The Pi's heartbeat updates `status` and `lastSeenAt` and fails if the item is missing, so seed first:

```sh
bash scripts/dev/seed-printer.sh --printer-id stand-001 --stage dev --name "Stand 1" --price 10000
# preview: add --dry-run ; overwrite an existing item: add --force
```

The script writes this item (printer id charset `[A-Za-z0-9:_-]`, the same rule as the IoT thing name):

```json
{
  "pk": { "S": "PRINTER#stand-001" },
  "sk": { "S": "META" },
  "name": { "S": "Stand 1" },
  "pricePerPrintVnd": { "N": "10000" },
  "status": { "S": "offline" }
}
```

It needs `dynamodb:PutItem` on `fotoko-<stage>` (your admin profile has it). Check: `curl -s "${API_URL}v1/printers/stand-001"` needs SigV4 (IAM auth), so check through the app or `aws dynamodb get-item --table-name fotoko-dev --key '{"pk":{"S":"PRINTER#stand-001"},"sk":{"S":"META"}}'`.

## 10. Provision the Pi certificate

Creates the IoT thing (name = printer id), a certificate, attaches the device policy (limited to the printer's own topics) and writes the files for the Pi:

```sh
pnpm --filter @fotoko/infra provision-printer --printer-id stand-001 --stage dev --out ./printer-out/stand-001 --dry-run
pnpm --filter @fotoko/infra provision-printer --printer-id stand-001 --stage dev --out ./printer-out/stand-001
```

Output: `device.pem.crt`, `private.pem.key` (0600), `AmazonRootCA1.pem`, `agent.toml`. **The private key is a secret**: keep `printer-out/` out of git (the `*.key`/`*.pem` patterns are ignored), copy it to the Pi over ssh only, and delete the local copy afterwards or keep it in a password manager. Needs `iot:*` rights for the thing, certificate and policy (your admin profile). Install and verify on the Pi with the runbook in `deploy/pi/README.md` (steps 2-6); rotation is step 7 there. Rotating retires the old certificate immediately.

## 11. Smoke test

1. `bash scripts/deploy/aws.sh deploy-web dev` printed the stack outputs; open `WebUrl` in a browser: the app loads.
2. API health: `curl -s "${API_URL}healthz"` is a public route and answers `200`.
3. After the Pi is installed: `journalctl -u fotoko-agent -n 50` on the Pi shows the broker connection and heartbeats, and the printer item turns `online` (the app shows the Stand as online).
4. Open `<WebUrl>/p/stand-001` on a phone, upload one photo, pay a small amount (see sandbox notes in [services.md](services.md)), watch the order go `paid -> printing -> completed` and the print come out. `aws logs tail /aws/lambda/fotoko-dev-fotoko-http --follow` shows the webhook arriving.
5. Check alarms are quiet and the budget exists.

## Teardown (dev)

`pnpm --filter @fotoko/infra exec cdk destroy Fotoko-dev -c stage=dev` removes the stack. The prod stack retains its data stores by design (removal policy); delete them by hand only on purpose. IoT certificates created by `provision-printer` are not CDK resources: deactivate and delete them in IoT Core.
