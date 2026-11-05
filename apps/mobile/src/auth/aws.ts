import { Sha256 } from '@aws-crypto/sha256-js';
import { HttpRequest } from '@smithy/protocol-http';
import { SignatureV4 } from '@smithy/signature-v4';

import type { AwsCredentials } from './cognito';
import type { Authenticator, SignableRequest } from './types';

/** Refresh this long before expiry. */
export const REFRESH_SKEW_MS = 5 * 60 * 1000;

export interface AwsAuthOptions {
  region: string;
  getCredentials: () => Promise<AwsCredentials>;
  now?: () => number;
  /** API Gateway execute-api by default. */
  service?: string;
}

/**
 * Splits an absolute http(s) URL by hand: React Native's `URL` is only partly implemented
 * (`searchParams` historically throws), and the signer needs exactly these parts.
 */
export function parseUrl(raw: string): {
  protocol: string;
  host: string;
  hostname: string;
  port: number | undefined;
  path: string;
  query: Record<string, string>;
} {
  const m = /^(https?:)\/\/([^/?#]+)([^?#]*)(?:\?([^#]*))?/i.exec(raw);
  if (!m) throw new Error(`not an absolute http(s) URL: ${raw}`);
  const host = m[2] as string;
  const portMatch = /:(\d+)$/.exec(host);
  const query: Record<string, string> = {};
  for (const pair of (m[4] ?? '').split('&')) {
    if (!pair) continue;
    const eq = pair.indexOf('=');
    const key = eq < 0 ? pair : pair.slice(0, eq);
    const value = eq < 0 ? '' : pair.slice(eq + 1);
    query[decodeURIComponent(key.replace(/\+/g, ' '))] = decodeURIComponent(
      value.replace(/\+/g, ' '),
    );
  }
  return {
    protocol: (m[1] as string).toLowerCase(),
    host,
    hostname: portMatch ? host.slice(0, -portMatch[0].length) : host,
    port: portMatch ? Number(portMatch[1]) : undefined,
    path: m[3] || '/',
    query,
  };
}

/** Cognito guest credentials, cached until near expiry, used to SigV4-sign API calls. */
export function createAwsAuth(options: AwsAuthOptions): Authenticator {
  const now = options.now ?? Date.now;
  let cached: AwsCredentials | undefined;
  let inflight: Promise<AwsCredentials> | undefined;

  async function credentials(): Promise<AwsCredentials> {
    if (cached && cached.expiresAt - now() > REFRESH_SKEW_MS) return cached;
    inflight ??= options
      .getCredentials()
      .then((c) => (cached = c))
      .finally(() => {
        inflight = undefined;
      });
    return inflight;
  }

  return {
    async headers(req: SignableRequest) {
      const creds = await credentials();
      const signer = new SignatureV4({
        credentials: {
          accessKeyId: creds.accessKeyId,
          secretAccessKey: creds.secretAccessKey,
          sessionToken: creds.sessionToken,
        },
        region: options.region,
        service: options.service ?? 'execute-api',
        sha256: Sha256,
      });
      const url = parseUrl(req.url);
      const signed = await signer.sign(
        new HttpRequest({
          method: req.method,
          protocol: url.protocol,
          hostname: url.hostname,
          port: url.port,
          path: url.path,
          query: url.query,
          headers: { ...req.headers, host: url.host },
          body: req.body,
        }),
        { signingDate: new Date(now()) },
      );
      // `host` is set by the HTTP stack; sending it from fetch is forbidden.
      const out: Record<string, string> = {};
      for (const [k, v] of Object.entries(signed.headers)) {
        if (k.toLowerCase() !== 'host') out[k] = v;
      }
      return out;
    },
  };
}
