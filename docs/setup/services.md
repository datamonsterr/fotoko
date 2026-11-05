# Third-party services

Accounts and settings outside AWS. AWS itself: [aws.md](aws.md). Variable reference: `.env.example`. Vendor consoles change; where this guide and the vendor's current documentation disagree, the vendor wins (and please fix this file). The authoring sandbox could not reach docs.payos.vn or docs.github.com, so the payOS dashboard labels and endpoints below are from memory and flagged **verify**.

## payOS (payments)

payOS ([payos.vn](https://payos.vn)) turns a bank transfer into a hosted checkout page with a signed webhook. Fotoko creates a payment link per order, sends the customer to `checkoutUrl`, and marks the order paid when the signed webhook arrives (`docs/contracts.md`, "payOS").

1. **Merchant account**: sign up at the payOS dashboard (my.payos.vn, **verify**) with the business owner's details and complete the merchant verification. You need a Vietnamese bank account that payOS supports; transfers land there directly.
2. **Payment channel**: create one channel per environment (for example `Fotoko dev`, `Fotoko prod`) and link the bank account. Each channel has its own three credentials and its own webhook URL, which is how dev and prod stay apart.
3. **Credentials** (channel settings, "Integration" / "Thông tin tích hợp", **verify**): **Client ID**, **API Key** and **Checksum Key**. These are secrets. The Checksum Key signs and verifies webhooks; treat a leaked one as compromised and regenerate it in the dashboard (the API re-reads the secret within a minute of a failed signature, see `crates/api/README.md`). Where they go:
   - AWS: Secrets Manager secret JSON `{clientId, apiKey, checksumKey}` ([aws.md](aws.md), step 7). Never in env vars of the Lambda, GitHub or git.
   - Local server: `PAYOS_CLIENT_ID`, `PAYOS_API_KEY`, `PAYOS_CHECKSUM_KEY` in your shell (only needed against real payOS; the payOS mock accepts any value).
4. **Sandbox vs live**: the code uses a single base URL (`PAYOS_BASE_URL`, default `https://api-merchant.payos.vn`). To my knowledge payOS has no separate sandbox host: a channel is either not yet activated/linked or live, and "testing" means a real transfer of a small amount (the smallest amount the channel allows) that you can refund by hand. **Verify** in the payOS docs whether a test mode exists; if it does, point a dev channel at it with `PAYOS_BASE_URL` (CDK sets it from `payosBaseUrl` in `infra/cdk/lib/config.ts`). For automated tests nothing real is used: the e2e stack runs `fotoko-payos-mock` (`crates/e2e/src/bin/fotoko-payos-mock.rs`), a hosted checkout page with a Pay button that posts a correctly signed webhook.
5. **Webhook URL**: `<ApiUrl>/v1/payments/payos/webhook` of the stage ([aws.md](aws.md), step 8). Register it in the channel settings, or with the confirm-webhook API (**verify** the exact path and headers):

   ```sh
   curl -s -X POST https://api-merchant.payos.vn/confirm-webhook \
     -H "x-client-id: $PAYOS_CLIENT_ID" -H "x-api-key: $PAYOS_API_KEY" \
     -H 'content-type: application/json' \
     -d "{\"webhookUrl\":\"${API_URL%/}/v1/payments/payos/webhook\"}"
   ```

   payOS validates the URL by posting a sample payload to it. The API **returns 200 `{"success":true}` for an unknown `orderCode`** (no-op), so the confirmation passes; a bad signature answers `400`. The handler is idempotent: duplicates re-publish the same job id, the Pi dedupes.

6. **After going live**: late payments for an expired or cancelled order are logged at warn and need a manual refund in the payOS dashboard (see the contract).

## Expo and EAS (mobile app)

The app is `apps/mobile` (Expo SDK 58, Expo Router). The **web build** ships through CDK/CloudFront ([aws.md](aws.md)), which is what the QR sticker opens; native builds are optional and not wired into CI.

1. Create an Expo account (expo.dev), `npm i -g eas-cli` (or `pnpm dlx eas-cli`), `eas login`. Add `EXPO_TOKEN` as a GitHub secret only if you later build in CI.
2. There is no `eas.json` yet. Create one from the app directory: `cd apps/mobile && eas build:configure` (writes `eas.json` and a project id into `app.json`). A sensible start:

   ```json
   {
     "build": {
       "development": { "developmentClient": true, "distribution": "internal" },
       "preview": {
         "distribution": "internal",
         "env": { "EXPO_PUBLIC_FOTOKO_MODE": "aws" }
       },
       "production": { "env": { "EXPO_PUBLIC_FOTOKO_MODE": "aws" } }
     }
   }
   ```

   `EXPO_PUBLIC_API_URL`, `EXPO_PUBLIC_AWS_REGION` and `EXPO_PUBLIC_COGNITO_IDENTITY_POOL_ID` are public and inlined at build time: put the stage's values in the profile `env` (they are not secrets), or create them as EAS environment variables (`eas env:create`).

3. `eas build -p android --profile preview` (APK/AAB) and `eas build -p ios --profile preview` (needs an Apple Developer account, USD 99 per year, and device registration for internal distribution). Bundle id and package are `vn.fotoko.app` (`app.json`).
4. Store notes: the app asks for camera (QR scan) and photo library access; the permission strings are in `app.json`. Apple requires a privacy policy URL and an App Privacy questionnaire (photos are uploaded to S3 for 7 days, then deleted; no accounts); Google Play needs a Data safety form. Because customers normally arrive by QR code into the web app, the native app is a convenience, not a launch requirement.
5. Maestro flows (`apps/mobile/.maestro/`) are not run in CI.

## GitHub

Repository `datamonsterr/fotoko`. Settings that matter are in [aws.md](aws.md) step 5; summary:

- **Actions**: `ci.yml` (required check `ci-ok`), `deploy.yml` (OIDC to AWS, environments `dev`/`prod`), `deny.yml` (weekly `cargo deny`). Pinned action SHAs are kept current by **Dependabot** (`.github/dependabot.yml`: cargo, npm, github-actions, weekly; Expo-pinned packages are ignored, upgrade them with `npx expo install --fix`).
- **Variables**: `AWS_DEPLOY_ROLE_ARN` per environment. **Secrets**: none today (the workflows use the automatic `GITHUB_TOKEN`). Add new ones at environment level, never in repository-wide secrets for prod.
- **Security features** worth enabling (Settings -> Code security): Dependabot alerts and security updates, secret scanning with push protection (payOS keys and AWS keys have recognizable shapes).
- **Branch protection** and environment reviewers: [aws.md](aws.md) step 5.

## Domain and HTTPS

Today the app is served from the default CloudFront domain (`https://dxxxxxxxx.cloudfront.net`, CDK output `WebUrl`) and the API from the default API Gateway URL; both are HTTPS already. QR stickers encode `<WebUrl>/p/<printerId>`, so **changing the domain later invalidates printed stickers**: decide before printing in bulk, or put a redirect behind the new domain. Outline for a custom domain (not implemented in CDK yet; plan a change in `infra/cdk/lib/web.ts` and `api.ts`):

1. Register or bring a domain; create a Route 53 hosted zone (or keep DNS elsewhere and use CNAME validation).
2. Request an ACM certificate for `app.<domain>` **in us-east-1** (CloudFront requirement) and one for `api.<domain>` in ap-southeast-1 (API Gateway custom domain), DNS-validated.
3. Add the alias names and certificate to the CloudFront distribution; add an API Gateway custom domain + API mapping; create the alias records.
4. Redeploy with `-c extraOrigins=https://app.<domain>` so CORS allows the new origin, set `PUBLIC_APP_URL` (CDK derives it from the web URL; adjust there) so payOS return URLs use it, rebuild the web app with the new `EXPO_PUBLIC_API_URL`, and re-register the payOS webhook URL if the API domain changed.

## OtherSkill and OpenRouter (optional, developer machines)

The `UserPromptSubmit` hook in `.claude/settings.json` routes bank skills per prompt through [OtherSkill](https://github.com/datamonsterr/other-skill). It needs `OPENROUTER_API_KEY`: create a free key at openrouter.ai/keys and export it in your shell profile (never in the repo). Without the key or the launcher the hook does nothing. One-time launcher install: `docs/skills.md` ("Hook setup").

## Context7 (optional)

`.mcp.json` starts the Context7 MCP server, which works without a key at a low rate limit. For heavier use create a key at context7.com/dashboard and export `CONTEXT7_API_KEY` (the config reads it from the environment).

## Monitoring

CDK creates the SNS topic `fotoko-<stage>-alarms` and CloudWatch alarms for Lambda errors/throttles, IoT rule failures and API 5xx. Deploy with `-c alarmEmail=ops@example.com` to subscribe an address (confirm the subscription email), or subscribe later in the SNS console (email, or an SMS/Slack chatbot target). Pair it with AWS Budgets alerts ([aws.md](aws.md) step 2). Logs: `/aws/lambda/fotoko-<stage>-<function>` (retention set per stage in `config.ts`); the Pi logs to journald (`journalctl -u fotoko-agent`). A Stand that stops heartbeating shows as offline in the app; there is no push alert for that yet (a follow-up: alarm on stale `lastSeenAt`).
