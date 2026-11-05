import { serverClock } from '@/api/clock';

import { createAwsAuth, parseUrl, REFRESH_SKEW_MS } from './aws';
import type { AwsCredentials } from './cognito';

const NOW = Date.UTC(2026, 0, 2, 3, 4, 5);

function creds(expiresAt: number, id = 'AKIDEXAMPLE'): AwsCredentials {
  return {
    accessKeyId: id,
    secretAccessKey: 'secret',
    sessionToken: 'tok',
    expiresAt,
  };
}

function make(getCredentials: () => Promise<AwsCredentials>, now = () => NOW) {
  return createAwsAuth({ region: 'ap-southeast-1', getCredentials, now });
}

describe('aws auth', () => {
  it('SigV4-signs with execute-api scope, session token and date', async () => {
    const auth = make(async () => creds(NOW + 3_600_000));
    const h = await auth.headers({
      method: 'POST',
      url: 'https://abc.execute-api.ap-southeast-1.amazonaws.com/v1/orders?b=2&a=1',
      headers: { 'content-type': 'application/json' },
      body: '{"x":1}',
    });
    expect(h['x-amz-date']).toBe('20260102T030405Z');
    expect(h['x-amz-security-token']).toBe('tok');
    expect(h.authorization).toMatch(
      /^AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE\/20260102\/ap-southeast-1\/execute-api\/aws4_request, SignedHeaders=[a-z0-9;-]*content-type[a-z0-9;-]*, Signature=[0-9a-f]{64}$/,
    );
    expect(h.authorization).toContain('host');
    expect(Object.keys(h).map((k) => k.toLowerCase())).not.toContain('host');
  });

  it('is deterministic and depends on the body and query', async () => {
    const auth = make(async () => creds(NOW + 3_600_000));
    const base = {
      method: 'POST',
      url: 'https://h.example:8443/v1/orders?q=1',
    };
    const sig = async (r: { method: string; url: string; body?: string }) =>
      (await auth.headers(r)).authorization;
    const a = await sig({ ...base, body: 'a' });
    expect(await sig({ ...base, body: 'a' })).toBe(a);
    expect(await sig({ ...base, body: 'b' })).not.toBe(a);
    expect(
      await sig({
        ...base,
        url: 'https://h.example:8443/v1/orders?q=2',
        body: 'a',
      }),
    ).not.toBe(a);
  });

  it('honours a custom service name', async () => {
    const auth = createAwsAuth({
      region: 'us-east-1',
      service: 'custom',
      getCredentials: async () => creds(NOW + 3_600_000),
      now: () => NOW,
    });
    const h = await auth.headers({ method: 'GET', url: 'https://h.example/x' });
    expect(h.authorization).toContain('/us-east-1/custom/aws4_request');
  });

  it('defaults the clock to Date.now', async () => {
    const auth = createAwsAuth({
      region: 'r',
      getCredentials: async () => creds(Date.now() + 3_600_000),
    });
    const h = await auth.headers({ method: 'GET', url: 'https://h.example/x' });
    expect(h['x-amz-date']).toMatch(/^\d{8}T\d{6}Z$/);
  });

  it('caches credentials until close to expiry, then refreshes', async () => {
    let t = NOW;
    const get = jest
      .fn<Promise<AwsCredentials>, []>()
      .mockResolvedValueOnce(creds(NOW + 3_600_000, 'FIRST'))
      .mockResolvedValueOnce(creds(NOW + 7_200_000, 'SECOND'));
    const auth = make(get, () => t);
    const r = { method: 'GET', url: 'https://h.example/x' };
    expect((await auth.headers(r)).authorization).toContain(
      'Credential=FIRST/',
    );
    await auth.headers(r);
    expect(get).toHaveBeenCalledTimes(1);
    t = NOW + 3_600_000 - REFRESH_SKEW_MS + 1;
    expect((await auth.headers(r)).authorization).toContain(
      'Credential=SECOND/',
    );
    expect(get).toHaveBeenCalledTimes(2);
  });

  it('shares one in-flight credential request', async () => {
    const get = jest.fn(async () => creds(NOW + 3_600_000));
    const auth = make(get);
    const r = { method: 'GET', url: 'https://h.example/x' };
    await Promise.all([auth.headers(r), auth.headers(r)]);
    expect(get).toHaveBeenCalledTimes(1);
  });

  it('retries credentials after a failure', async () => {
    const get = jest
      .fn<Promise<AwsCredentials>, []>()
      .mockRejectedValueOnce(new Error('cognito down'))
      .mockResolvedValueOnce(creds(NOW + 3_600_000));
    const auth = make(get);
    const r = { method: 'GET', url: 'https://h.example/x' };
    await expect(auth.headers(r)).rejects.toThrow('cognito down');
    await expect(auth.headers(r)).resolves.toHaveProperty('authorization');
  });
});

describe('parseUrl', () => {
  it('splits host, port, path and decoded query', () => {
    expect(
      parseUrl('HTTPS://h.example:8443/v1/orders/1?a=1&b=x%20y&c&d=p+q#frag'),
    ).toEqual({
      protocol: 'https:',
      host: 'h.example:8443',
      hostname: 'h.example',
      port: 8443,
      path: '/v1/orders/1',
      query: { a: '1', b: 'x y', c: '', d: 'p q' },
    });
  });

  it('defaults path to / and tolerates empty query pieces', () => {
    expect(parseUrl('http://h.example?&')).toMatchObject({
      path: '/',
      port: undefined,
      query: {},
    });
  });

  it('rejects relative URLs', () => {
    expect(() => parseUrl('/v1/orders')).toThrow(/absolute/);
  });
});

describe('aws auth: invalidate and clock', () => {
  it('invalidate drops the cache so the next call fetches fresh credentials', async () => {
    const get = jest
      .fn<Promise<AwsCredentials>, []>()
      .mockResolvedValueOnce(creds(NOW + 3_600_000, 'OLD'))
      .mockResolvedValueOnce(creds(NOW + 3_600_000, 'NEW'));
    const auth = make(get);
    const r = { method: 'GET', url: 'https://h.example/x' };
    expect((await auth.headers(r)).authorization).toContain('Credential=OLD/');
    auth.invalidate?.();
    expect((await auth.headers(r)).authorization).toContain('Credential=NEW/');
  });

  it('signs with the server-corrected clock by default', async () => {
    serverClock.observe(new Date(Date.UTC(2031, 5, 7, 8, 9, 10)).toUTCString());
    const auth = createAwsAuth({
      region: 'r',
      getCredentials: async () => creds(Date.UTC(2031, 5, 7, 12)),
    });
    const h = await auth.headers({ method: 'GET', url: 'https://h.example/x' });
    expect(h['x-amz-date']).toMatch(/^20310607T0809\d\dZ$/);
    serverClock.reset();
  });

  it('omits the session token header when the credentials have none', async () => {
    const auth = make(async () => ({
      ...creds(NOW + 3_600_000),
      sessionToken: '',
    }));
    const h = await auth.headers({ method: 'GET', url: 'https://h.example/x' });
    expect(h).not.toHaveProperty('x-amz-security-token');
  });
});
