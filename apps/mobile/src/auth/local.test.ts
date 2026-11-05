import { createLocalAuth, IDENTITY_HEADER, IDENTITY_KEY } from './local';
import type { KeyValueStore } from './types';

function memStore(initial: Record<string, string> = {}) {
  const m = new Map(Object.entries(initial));
  const store: KeyValueStore & { m: Map<string, string> } = {
    m,
    get: jest.fn(async (k: string) => m.get(k) ?? null),
    set: jest.fn(async (k: string, v: string) => void m.set(k, v)),
  };
  return store;
}
const req = { method: 'GET', url: 'https://a/v1/x' };

describe('local auth', () => {
  it('generates, persists and reuses a stable id', async () => {
    const store = memStore();
    const newId = jest
      .fn()
      .mockReturnValueOnce('local-1')
      .mockReturnValueOnce('local-2');
    const auth = createLocalAuth({ store, newId });
    const [a, b] = await Promise.all([auth.headers(req), auth.headers(req)]);
    expect(a).toEqual({ [IDENTITY_HEADER]: 'local-1' });
    expect(b).toEqual(a);
    expect(newId).toHaveBeenCalledTimes(1);
    expect(store.m.get(IDENTITY_KEY)).toBe('local-1');
    await expect(auth.headers(req)).resolves.toEqual(a);
  });

  it('reuses the id from an earlier session', async () => {
    const newId = jest.fn();
    const auth = createLocalAuth({
      store: memStore({ [IDENTITY_KEY]: 'local-old' }),
      newId,
    });
    await expect(auth.headers(req)).resolves.toEqual({
      'x-fotoko-identity': 'local-old',
    });
    expect(newId).not.toHaveBeenCalled();
  });
});
