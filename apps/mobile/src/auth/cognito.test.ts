import { COGNITO_IDENTITY_KEY, getGuestCredentials } from './cognito';
import type { KeyValueStore } from './types';

function memStore(initial: Record<string, string> = {}) {
  const m = new Map(Object.entries(initial));
  const store: KeyValueStore & { m: Map<string, string> } = {
    m,
    get: async (k) => m.get(k) ?? null,
    set: async (k, v) => void m.set(k, v),
  };
  return store;
}

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status });
const credsBody = {
  IdentityId: 'ap-southeast-1:abc',
  Credentials: {
    AccessKeyId: 'AK',
    SecretKey: 'SK',
    SessionToken: 'ST',
    Expiration: 1_800_000_000.5,
  },
};

function opts(fetchImpl: jest.Mock, store = memStore()) {
  return {
    region: 'ap-southeast-1',
    identityPoolId: 'ap-southeast-1:pool',
    store,
    fetchImpl: fetchImpl as unknown as typeof fetch,
  };
}

describe('getGuestCredentials', () => {
  it('GetId then GetCredentialsForIdentity, remembering the identity', async () => {
    const f = jest
      .fn()
      .mockResolvedValueOnce(json(200, { IdentityId: 'ap-southeast-1:abc' }))
      .mockResolvedValueOnce(json(200, credsBody));
    const store = memStore();
    const c = await getGuestCredentials(opts(f, store));
    expect(c).toEqual({
      accessKeyId: 'AK',
      secretAccessKey: 'SK',
      sessionToken: 'ST',
      expiresAt: 1_800_000_000_500,
    });
    expect(store.m.get(COGNITO_IDENTITY_KEY)).toBe('ap-southeast-1:abc');
    const [url1, init1] = f.mock.calls[0] as [string, RequestInit];
    expect(url1).toBe('https://cognito-identity.ap-southeast-1.amazonaws.com/');
    expect(init1.headers).toEqual({
      'content-type': 'application/x-amz-json-1.1',
      'x-amz-target': 'AWSCognitoIdentityService.GetId',
    });
    expect(JSON.parse(init1.body as string)).toEqual({
      IdentityPoolId: 'ap-southeast-1:pool',
    });
    const init2 = f.mock.calls[1]?.[1] as RequestInit;
    expect(init2.headers).toMatchObject({
      'x-amz-target': 'AWSCognitoIdentityService.GetCredentialsForIdentity',
    });
    expect(JSON.parse(init2.body as string)).toEqual({
      IdentityId: 'ap-southeast-1:abc',
    });
  });

  it('skips GetId when the identity is stored; accepts SecretAccessKey and ISO expiry', async () => {
    const f = jest.fn().mockResolvedValueOnce(
      json(200, {
        Credentials: {
          AccessKeyId: 'AK',
          SecretAccessKey: 'SK2',
          SessionToken: 'ST',
          Expiration: '2030-01-01T00:00:00Z',
        },
      }),
    );
    const c = await getGuestCredentials(
      opts(f, memStore({ [COGNITO_IDENTITY_KEY]: 'id-1' })),
    );
    expect(f).toHaveBeenCalledTimes(1);
    expect(c.secretAccessKey).toBe('SK2');
    expect(c.expiresAt).toBe(Date.UTC(2030, 0, 1));
  });

  it('fails on HTTP errors, with and without a body', async () => {
    const f1 = jest
      .fn()
      .mockResolvedValue(
        json(400, { __type: 'NotAuthorizedException', message: 'no guest' }),
      );
    await expect(getGuestCredentials(opts(f1))).rejects.toThrow(
      /GetId failed \(400\): no guest/,
    );
    const f2 = jest
      .fn()
      .mockResolvedValue(new Response('<html>', { status: 502 }));
    await expect(getGuestCredentials(opts(f2))).rejects.toThrow(
      /GetId failed \(502\)/,
    );
    const f3 = jest.fn().mockResolvedValue(json(400, { __type: 'X' }));
    await expect(getGuestCredentials(opts(f3))).rejects.toThrow(/: X/);
  });

  it('fails on malformed answers', async () => {
    await expect(
      getGuestCredentials(opts(jest.fn().mockResolvedValue(json(200, {})))),
    ).rejects.toThrow(/no IdentityId/);
    const stored = memStore({ [COGNITO_IDENTITY_KEY]: 'id' });
    await expect(
      getGuestCredentials(
        opts(jest.fn().mockResolvedValue(json(200, {})), stored),
      ),
    ).rejects.toThrow(/no credentials/);
    const bad = {
      Credentials: {
        AccessKeyId: 'a',
        SecretKey: 's',
        SessionToken: 't',
        Expiration: 'soon',
      },
    };
    await expect(
      getGuestCredentials(
        opts(jest.fn().mockResolvedValue(json(200, bad)), stored),
      ),
    ).rejects.toThrow(/Expiration/);
  });

  it('defaults to the global fetch', async () => {
    const spy = jest
      .spyOn(globalThis, 'fetch')
      .mockResolvedValue(json(200, credsBody));
    await getGuestCredentials({
      region: 'r',
      identityPoolId: 'p',
      store: memStore({ [COGNITO_IDENTITY_KEY]: 'id' }),
    });
    expect(spy).toHaveBeenCalledTimes(1);
    spy.mockRestore();
  });
});
