import { createAuthFromEnv, parseMode } from './index';
import { IDENTITY_HEADER } from './local';

describe('parseMode', () => {
  it.each([
    ['local', 'local'],
    ['aws', 'aws'],
    [undefined, 'aws'],
    ['junk', 'aws'],
  ])('%p -> %p', (raw, mode) => {
    expect(parseMode(raw)).toBe(mode);
  });
});

describe('createAuthFromEnv', () => {
  it('local mode sends a persisted local-* identity', async () => {
    const auth = createAuthFromEnv({ EXPO_PUBLIC_FOTOKO_MODE: 'local' });
    const h = await auth.headers({ method: 'GET', url: 'https://a/x' });
    expect(h[IDENTITY_HEADER]).toMatch(/^local-uuid-\d+$/);
    const again = createAuthFromEnv({ EXPO_PUBLIC_FOTOKO_MODE: 'local' });
    expect(await again.headers({ method: 'GET', url: 'https://a/x' })).toEqual(
      h,
    );
  });

  it('aws mode requires region and pool', () => {
    expect(() => createAuthFromEnv({})).toThrow(/EXPO_PUBLIC_AWS_REGION/);
    expect(() => createAuthFromEnv({ EXPO_PUBLIC_AWS_REGION: 'r' })).toThrow(
      /POOL_ID/,
    );
  });

  it('aws mode signs using Cognito guest credentials', async () => {
    const spy = jest
      .spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ IdentityId: 'r:1' })),
      )
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            Credentials: {
              AccessKeyId: 'AK',
              SecretKey: 'SK',
              SessionToken: 'ST',
              Expiration: Date.now() / 1000 + 3600,
            },
          }),
        ),
      );
    const auth = createAuthFromEnv({
      EXPO_PUBLIC_FOTOKO_MODE: 'aws',
      EXPO_PUBLIC_AWS_REGION: 'ap-southeast-1',
      EXPO_PUBLIC_COGNITO_IDENTITY_POOL_ID: 'ap-southeast-1:pool',
    });
    const h = await auth.headers({
      method: 'GET',
      url: 'https://h.example/v1/orders/1',
    });
    expect(h.authorization).toContain('Credential=AK/');
    spy.mockRestore();
  });
});
