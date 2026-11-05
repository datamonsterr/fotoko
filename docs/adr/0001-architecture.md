# ADR 0001: Architecture and managed services

Status: accepted (2026-11-05)

## Decision

- Clients: one Expo (React Native + Expo Router) app for iOS, Android and web. Web build is the quick-view entry point opened from a Stand QR.
- Backend: Rust on AWS Lambda behind API Gateway HTTP API (`lambda_http` + axum router). No servers.
- Managed services (no home-built auth/db):
  - Auth: Amazon Cognito. Identity pool with unauthenticated (guest) identities for Stand users; user pool for operators/admin later.
  - DB: DynamoDB, single table.
  - Images: S3 private bucket, presigned PUT for upload, presigned GET (short TTL) for the Pi.
  - Device link: AWS IoT Core MQTT, X.509 cert per Pi, IoT policy scoped to `fotoko/printers/${iot:Connection.Thing.ThingName}/*`. Status messages routed by an IoT rule to a Lambda.
  - Payments: payOS payment links + signed webhook.
- Infra as code: AWS CDK (TypeScript) in `infra/cdk`.
- Pi: Rust agent, cross-compiled to `aarch64-unknown-linux-gnu`, systemd service, prints with CUPS `lp` + Gutenprint driver. Pi has no inbound ports.
- User flow: scan QR on Stand -> `/p/{printerId}` -> pick photos -> upload -> pay via payOS -> webhook marks Paid -> job published to the Pi -> Pi prints -> status back -> app shows done.

## Consequences

- All business logic lives in `fotoko-core` behind ports, so unit tests and the simulation e2e run without AWS.
- Refunds, operator dashboard, multi-printer stands: out of scope for the initial commit.
