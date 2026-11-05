export interface SignableRequest {
  method: string;
  /** Absolute URL, including query. */
  url: string;
  /** Headers the request will carry; the signer signs these. */
  headers?: Record<string, string>;
  /** JSON string or undefined. */
  body?: string;
}

/** Produces the auth headers to merge into an API request. */
export interface Authenticator {
  headers(request: SignableRequest): Promise<Record<string, string>>;
  /** Drop cached credentials so the next call fetches fresh ones (after a 401/403). */
  invalidate?(): void;
}

export interface KeyValueStore {
  get(key: string): Promise<string | null>;
  set(key: string, value: string): Promise<void>;
}
