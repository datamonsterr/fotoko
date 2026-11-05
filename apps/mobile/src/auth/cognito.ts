/**
 * Minimal Cognito Identity client: guest (unauthenticated) credentials over the JSON 1.1 API.
 * Plain fetch instead of `@aws-sdk/client-cognito-identity` keeps the bundle small and avoids
 * the SDK's React Native polyfill needs (URL, crypto.getRandomValues).
 */
import type { KeyValueStore } from './types';

export interface AwsCredentials {
  accessKeyId: string;
  secretAccessKey: string;
  sessionToken: string;
  /** Epoch milliseconds. */
  expiresAt: number;
}

export interface CognitoOptions {
  region: string;
  identityPoolId: string;
  store: KeyValueStore;
  fetchImpl?: typeof fetch;
}

export const COGNITO_IDENTITY_KEY = 'fotoko.cognitoIdentityId';

type Json = Record<string, unknown>;

/** A Cognito service error; `type` is the `__type` name, e.g. `ResourceNotFoundException`. */
export class CognitoError extends Error {
  constructor(
    readonly target: string,
    readonly status: number,
    readonly type: string,
    detail: string,
  ) {
    super(`Cognito ${target} failed (${status}): ${detail}`);
    this.name = 'CognitoError';
  }
}

async function call(
  opts: CognitoOptions,
  target: 'GetId' | 'GetCredentialsForIdentity',
  body: Json,
): Promise<Json> {
  const doFetch =
    opts.fetchImpl ?? ((...a: Parameters<typeof fetch>) => fetch(...a));
  const res = await doFetch(
    `https://cognito-identity.${opts.region}.amazonaws.com/`,
    {
      method: 'POST',
      headers: {
        'content-type': 'application/x-amz-json-1.1',
        'x-amz-target': `AWSCognitoIdentityService.${target}`,
      },
      body: JSON.stringify(body),
    },
  );
  const json = (await res.json().catch(() => ({}))) as Json;
  if (!res.ok) {
    // __type can be "com.amazon...#ResourceNotFoundException"
    const type =
      String(json.__type ?? '')
        .split('#')
        .pop() ?? '';
    throw new CognitoError(
      target,
      res.status,
      type,
      String(json.message ?? json.__type ?? ''),
    );
  }
  return json;
}

function parseExpiration(v: unknown): number {
  // JSON 1.1 returns epoch seconds (float); tolerate an ISO string too.
  if (typeof v === 'number') return v * 1000;
  if (typeof v === 'string') {
    const t = Date.parse(v);
    if (!Number.isNaN(t)) return t;
  }
  throw new Error('Cognito credentials have no valid Expiration');
}

/** The stored identity is gone or not ours any more: get a fresh one. */
const STALE_IDENTITY = ['ResourceNotFoundException', 'NotAuthorizedException'];

async function newIdentity(opts: CognitoOptions): Promise<string> {
  const got = await call(opts, 'GetId', {
    IdentityPoolId: opts.identityPoolId,
  });
  if (typeof got.IdentityId !== 'string') {
    throw new Error('Cognito GetId returned no IdentityId');
  }
  await opts.store.set(COGNITO_IDENTITY_KEY, got.IdentityId);
  return got.IdentityId;
}

export async function getGuestCredentials(
  opts: CognitoOptions,
): Promise<AwsCredentials> {
  let identityId = (await opts.store.get(COGNITO_IDENTITY_KEY)) || null;
  let fresh = false;
  if (!identityId) {
    identityId = await newIdentity(opts);
    fresh = true;
  }
  let out: Json;
  try {
    out = await call(opts, 'GetCredentialsForIdentity', {
      IdentityId: identityId,
    });
  } catch (e) {
    if (
      fresh ||
      !(e instanceof CognitoError) ||
      !STALE_IDENTITY.includes(e.type)
    )
      throw e;
    // The pool was recreated or the identity deleted: forget it and retry once.
    await opts.store.set(COGNITO_IDENTITY_KEY, '');
    identityId = await newIdentity(opts);
    out = await call(opts, 'GetCredentialsForIdentity', {
      IdentityId: identityId,
    });
  }
  const c = out.Credentials as Json | undefined;
  const secret = c?.SecretKey ?? c?.SecretAccessKey;
  if (
    !c ||
    typeof c.AccessKeyId !== 'string' ||
    typeof secret !== 'string' ||
    typeof c.SessionToken !== 'string'
  ) {
    throw new Error(
      'Cognito GetCredentialsForIdentity returned no credentials',
    );
  }
  return {
    accessKeyId: c.AccessKeyId,
    secretAccessKey: secret,
    sessionToken: c.SessionToken,
    expiresAt: parseExpiration(c.Expiration),
  };
}
