import type { Authenticator, KeyValueStore } from './types';

export const IDENTITY_KEY = 'fotoko.identity';
export const IDENTITY_HEADER = 'x-fotoko-identity';

export interface LocalAuthOptions {
  store: KeyValueStore;
  /** Generates a new random id the first time. */
  newId: () => string;
}

/** Local/sim mode: a stable random id, persisted, sent as `x-fotoko-identity`. */
export function createLocalAuth(options: LocalAuthOptions): Authenticator {
  let pending: Promise<string> | undefined;

  function identity(): Promise<string> {
    pending ??= (async () => {
      const existing = await options.store.get(IDENTITY_KEY);
      if (existing) return existing;
      const id = options.newId();
      await options.store.set(IDENTITY_KEY, id);
      return id;
    })();
    return pending;
  }

  return {
    async headers() {
      return { [IDENTITY_HEADER]: await identity() };
    },
  };
}
