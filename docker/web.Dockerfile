# syntax=docker/dockerfile:1
# The Expo web app (local mode) behind the same-origin gateway that also proxies the API.
# Build context: the repo root. EXPO_PUBLIC_API_URL is baked into the bundle, so it must be the
# address the browser uses for this container.

FROM node:22-bookworm-slim AS build
RUN corepack enable
WORKDIR /src
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY apps/mobile/package.json apps/mobile/package.json
COPY packages/e2e-web/package.json packages/e2e-web/package.json
COPY infra/cdk/package.json infra/cdk/package.json
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile --ignore-scripts --filter @fotoko/mobile...
COPY apps/mobile apps/mobile
ARG EXPO_PUBLIC_API_URL=http://localhost:8081
ENV CI=1 EXPO_PUBLIC_FOTOKO_MODE=local EXPO_PUBLIC_API_URL=${EXPO_PUBLIC_API_URL}
RUN pnpm --filter @fotoko/mobile exec expo export -p web --output-dir /dist

FROM node:22-bookworm-slim
WORKDIR /app
COPY scripts/e2e/gateway.mjs ./gateway.mjs
COPY --from=build /dist ./dist
USER node
ENV GATEWAY_HOST=0.0.0.0 GATEWAY_PORT=8081 STATIC_DIR=/app/dist
EXPOSE 8081
CMD ["node", "gateway.mjs"]
