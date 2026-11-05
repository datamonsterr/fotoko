# @fotoko/mobile

Expo app (iOS, Android, web) for Fotoko. Scan a Stand's QR code, pick photos, pay with payOS, watch the print. Terms: `CONTEXT.md`; API: `docs/contracts.md`.

Expo SDK 58, Expo Router (typed routes), TypeScript strict. Scheme `fotoko`.

## Commands

```sh
pnpm --filter @fotoko/mobile start      # dev server
pnpm --filter @fotoko/mobile web        # web only
pnpm --filter @fotoko/mobile lint       # ESLint (eslint-config-expo, flat)
pnpm --filter @fotoko/mobile typecheck  # tsc --noEmit
pnpm --filter @fotoko/mobile test       # jest --ci (jest-expo + React Native Testing Library)
pnpm --filter @fotoko/mobile build:web  # expo export -p web -> dist/
```

Maestro flows are in `.maestro/` and are not run in CI. Test IDs: `TESTIDS.md`.

## Configuration

`EXPO_PUBLIC_*` variables are inlined at build time. See `.env.example`.

| Variable                               | Meaning                                                     |
| -------------------------------------- | ----------------------------------------------------------- |
| `EXPO_PUBLIC_API_URL`                  | Backend base URL (default `http://localhost:8787`)          |
| `EXPO_PUBLIC_FOTOKO_MODE`              | `local` (identity header) or `aws` (default, Cognito+SigV4) |
| `EXPO_PUBLIC_AWS_REGION`               | aws mode: region of the identity pool and API Gateway       |
| `EXPO_PUBLIC_COGNITO_IDENTITY_POOL_ID` | aws mode: identity pool with guest access                   |

## Routes

| Route                   | Screen                                                      |
| ----------------------- | ----------------------------------------------------------- |
| `/`                     | Scan button, manual printer id                              |
| `/scan`                 | QR scanner (`https://<host>/p/<id>` or `fotoko://p/<id>`)   |
| `/p/[printerId]`        | Printer info, price, status, start                          |
| `/p/[printerId]/photos` | Pick up to 10 photos, 1..5 copies each, live total          |
| `/p/[printerId]/upload` | Create order, upload with progress, checkout, open payOS    |
| `/orders/[orderId]`     | Poll every 2 s until terminal, timeline, cancel before paid |

payOS `returnUrl`/`cancelUrl` are `{PUBLIC_APP_URL}/orders/{orderId}`. On web the checkout navigates the same tab; on native it opens the in-app browser and then shows the order screen, which polls the real status.

## Auth (`src/auth`)

- `local`: a random `local-<uuid>` persisted in expo-secure-store (localStorage on web) is sent as `x-fotoko-identity`.
- `aws`: Cognito identity pool guest credentials, requests SigV4-signed for `execute-api`. Public `GET /v1/printers/{id}` is sent unsigned.

Why not the AWS SDK or Amplify: Hermes has no `crypto.subtle`, React Native's `URL` is only partly implemented (the signer parses URLs by hand), and `@aws-sdk/credential-providers` pulls in the full SDK client stack. The app instead calls the two Cognito Identity JSON endpoints (`GetId`, `GetCredentialsForIdentity`, no signing needed) with `fetch`, caches credentials until 5 minutes before expiry, and signs with `@smithy/signature-v4` plus the pure-JS `@aws-crypto/sha256-js`. This works unchanged on iOS, Android and web.

Robustness: a gateway-level 401/403 (`{"message": ...}`, not the contract's `forbidden`) drops the cached credentials and retries once; a second one surfaces as `ApiError('auth')`. Every response's `Date` header updates a server-clock offset that is used for the signing time and for judging whether a presigned URL or the order has expired, so a phone with a wrong clock still works. If Cognito says the stored identity no longer exists (`ResourceNotFoundException`, `NotAuthorizedException`), the app forgets it, calls `GetId` again and retries once.

Presigned S3 PUTs carry no auth headers; the headers from `uploads[].headers` are sent as given.

## Web and CORS

The web build calls the API and S3 from the browser, so both need CORS:

- API Gateway: allow origin = the web app's origin; allow headers `content-type`, `accept`, `authorization`, `x-amz-date`, `x-amz-security-token`, `x-fotoko-identity` (local mode); allow methods `GET, POST, OPTIONS`; **expose header `Date`** (`Access-Control-Expose-Headers: Date`), otherwise browsers hide it and the clock-skew correction silently stays at 0.
- S3 bucket: allow `PUT` from the web origin with the headers the presigned URL signs (at least `content-type`).
- Cognito Identity (`cognito-identity.<region>.amazonaws.com`) already answers browser CORS.

On web the local-mode identity lives in `localStorage`: readable by any script on the origin. That is acceptable for a test identity; production uses aws mode.

## Leaving the upload screen

While the order is created, photos upload or checkout opens, the back gesture, header back button and Android hardware back are blocked, and unmounting aborts in-flight uploads without checking out. After a failure they work again. If the order or an upload URL expires, the screen offers "Start over": it cancels the old order (best effort) and creates a new one from the same photos.

## i18n

Vietnamese is the default; English when the device language is English, or via the toggle on the home screen. Keys live in `src/i18n/vi.ts` (typed) and `en.ts` (must have the same keys; a test enforces it).

## Layout

```
src/api       types.ts mirrors contracts.md; client.ts; errors.ts (ApiError); upload.ts (presigned PUT)
src/auth      local, aws, cognito, storage
src/hooks     usePrinter, useOrderPolling, checkoutFlow (create -> upload -> checkout state machine)
src/lib       price, qr, status, photos, limits
src/screens   screen components (routes in src/app are thin wrappers)
```
