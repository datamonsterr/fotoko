/**
 * Known-answer tests. The expected signatures come from src/test/sigv4_vector.py, an
 * independent stdlib-only implementation (run it to regenerate); vector 1 is AWS's published
 * "get-vanilla" case, so the reference itself is anchored to AWS.
 */
import { createAwsAuth } from './aws';

const sign = (
  req: Parameters<ReturnType<typeof createAwsAuth>['headers']>[0],
  opts: {
    region: string;
    service?: string;
    now: number;
    accessKeyId: string;
    secretAccessKey: string;
    sessionToken: string;
  },
) =>
  createAwsAuth({
    region: opts.region,
    service: opts.service,
    now: () => opts.now,
    getCredentials: async () => ({
      accessKeyId: opts.accessKeyId,
      secretAccessKey: opts.secretAccessKey,
      sessionToken: opts.sessionToken,
      expiresAt: opts.now + 3_600_000,
    }),
  }).headers(req);

describe('SigV4 known answers', () => {
  it('AWS test suite: get-vanilla', async () => {
    const h = await sign(
      { method: 'GET', url: 'https://example.amazonaws.com/' },
      {
        region: 'us-east-1',
        service: 'service',
        now: Date.UTC(2015, 7, 30, 12, 36, 0),
        accessKeyId: 'AKIDEXAMPLE',
        secretAccessKey: 'wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY',
        sessionToken: '',
      },
    );
    expect(h.authorization).toBe(
      'AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, ' +
        'SignedHeaders=host;x-amz-date, ' +
        'Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31',
    );
    expect(h['x-amz-date']).toBe('20150830T123600Z');
  });

  it('what the app sends: POST, port, encoded path, repeated and encoded query, body, token', async () => {
    const h = await sign(
      {
        method: 'POST',
        url: 'https://abc123.execute-api.ap-southeast-1.amazonaws.com:8443/v1/orders/a%20b/caf%C3%A9?z=2&a=1&a=%2F&m=x%20y',
        headers: { 'content-type': 'application/json' },
        body: '{"printerId":"stand-001"}',
      },
      {
        region: 'ap-southeast-1',
        now: Date.UTC(2026, 0, 2, 3, 4, 5),
        accessKeyId: 'ASIAEXAMPLE',
        secretAccessKey: 'SECRETKEYEXAMPLE/abc+def',
        sessionToken: 'SESSIONTOKEN/+=',
      },
    );
    expect(h.authorization).toBe(
      'AWS4-HMAC-SHA256 Credential=ASIAEXAMPLE/20260102/ap-southeast-1/execute-api/aws4_request, ' +
        'SignedHeaders=content-type;host;x-amz-date;x-amz-security-token, ' +
        'Signature=6d5f8f6c085b1b265c57d76bbb7f9c3d0dff2c91a8e562828b4c33dc778f4292',
    );
    expect(h['x-amz-security-token']).toBe('SESSIONTOKEN/+=');
  });
});
