import { createAuthFromEnv } from '@/auth';

import { createApiClient, type ApiClient } from './client';

let instance: ApiClient | undefined;

/**
 * App-wide client. Reads env at first use. `process.env.EXPO_PUBLIC_*` must be referenced
 * literally so Metro inlines it.
 */
export function getApiClient(): ApiClient {
  instance ??= createApiClient({
    baseUrl: process.env.EXPO_PUBLIC_API_URL ?? 'http://localhost:8787',
    auth: createAuthFromEnv({
      EXPO_PUBLIC_FOTOKO_MODE: process.env.EXPO_PUBLIC_FOTOKO_MODE,
      EXPO_PUBLIC_AWS_REGION: process.env.EXPO_PUBLIC_AWS_REGION,
      EXPO_PUBLIC_COGNITO_IDENTITY_POOL_ID:
        process.env.EXPO_PUBLIC_COGNITO_IDENTITY_POOL_ID,
    }),
  });
  return instance;
}

/** Test seam. */
export function setApiClient(client: ApiClient | undefined): void {
  instance = client;
}

export * from './client';
export * from './errors';
export * from './types';
export * from './upload';
