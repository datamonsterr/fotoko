import type { Authenticator, KeyValueStore } from './types';

export const IDENTITY_KEY = 'fotoko.identity';
export const IDENTITY_HEADER = 'x-fotoko-identity';

export interface LocalAuthOptions {
  store: KeyValueStore;
  /** Generates a new random id the first time. */
  newId: () => string;
}

/**
 * Local/sim mode: a stable random id, persisted, sent as `x-fotoko-identity`.
 * On web the id lives in localStorage: any script on the origin can read or replace it, which
 * is fine for local/sim mode (it is a test identity, not a credential) and why aws mode exists.
 */
export function createLocalAuth(options: LocalAuthOptions): Authenticator {
  let pending: Promise<string> | undefined;

  function identity(): Promise<string> {
    pending ??= (async () => {
      const existing = await options.store.get(IDENTITY_KEY);
      if (existing) return existing;
      const id = options.newId();
      await options.store.set(IDENTITY_KEY, id);
      return id;
    })().catch((e: unknown) => {
      // Do not cache a rejection: the next request tries again.
      pending = undefined;
      throw e;
    });
    return pending;
  }

  return {
    async headers() {
      return { [IDENTITY_HEADER]: await identity() };
    },
  };
}
