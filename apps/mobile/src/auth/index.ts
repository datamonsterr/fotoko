import { randomUUID } from 'expo-crypto';

import { createAwsAuth } from './aws';
import { getGuestCredentials } from './cognito';
import { createLocalAuth } from './local';
import { persistentStore } from './storage';
import type { Authenticator } from './types';

export type FotokoMode = 'local' | 'aws';

export function parseMode(raw: string | undefined): FotokoMode {
  return raw === 'local' ? 'local' : 'aws';
}

export interface AuthEnv {
  EXPO_PUBLIC_FOTOKO_MODE?: string;
  EXPO_PUBLIC_AWS_REGION?: string;
  EXPO_PUBLIC_COGNITO_IDENTITY_POOL_ID?: string;
}

/** `local` -> identity header; anything else -> aws (Cognito guest + SigV4). */
export function createAuthFromEnv(env: AuthEnv): Authenticator {
  if (parseMode(env.EXPO_PUBLIC_FOTOKO_MODE) === 'local') {
    return createLocalAuth({
      store: persistentStore,
      newId: () => `local-${randomUUID()}`,
    });
  }
  const region = env.EXPO_PUBLIC_AWS_REGION;
  const identityPoolId = env.EXPO_PUBLIC_COGNITO_IDENTITY_POOL_ID;
  if (!region || !identityPoolId) {
    throw new Error(
      'aws mode needs EXPO_PUBLIC_AWS_REGION and EXPO_PUBLIC_COGNITO_IDENTITY_POOL_ID',
    );
  }
  return createAwsAuth({
    region,
    getCredentials: () =>
      getGuestCredentials({ region, identityPoolId, store: persistentStore }),
  });
}

export type { Authenticator } from './types';
