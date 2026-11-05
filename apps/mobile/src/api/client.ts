import type { Authenticator } from '@/auth/types';

import { ApiError, parseErrorBody } from './errors';
import type {
  CheckoutResponse,
  CreateOrderRequest,
  CreateOrderResponse,
  Order,
  Printer,
} from './types';

export interface ApiClientOptions {
  /** Base URL without a trailing slash; `/v1/...` is appended. */
  baseUrl: string;
  /** Omit for unauthenticated use. */
  auth?: Authenticator;
  /** Defaults to the global `fetch`, resolved at call time. */
  fetchImpl?: typeof fetch;
}

export interface ApiClient {
  getPrinter(printerId: string, signal?: AbortSignal): Promise<Printer>;
  createOrder(req: CreateOrderRequest): Promise<CreateOrderResponse>;
  getOrder(orderId: string, signal?: AbortSignal): Promise<Order>;
  checkout(orderId: string): Promise<CheckoutResponse>;
  cancelOrder(orderId: string): Promise<Order>;
}

interface RequestOptions {
  method: 'GET' | 'POST';
  path: string;
  body?: unknown;
  /** Public routes skip the auth round trip. */
  authenticated: boolean;
  signal?: AbortSignal;
}

export function createApiClient(options: ApiClientOptions): ApiClient {
  const base = options.baseUrl.replace(/\/+$/, '');

  async function request<T>(opts: RequestOptions): Promise<T> {
    const url = `${base}${opts.path}`;
    const body =
      opts.body === undefined ? undefined : JSON.stringify(opts.body);
    const headers: Record<string, string> = { accept: 'application/json' };
    if (body !== undefined) headers['content-type'] = 'application/json';
    if (opts.authenticated && options.auth) {
      Object.assign(
        headers,
        await options.auth.headers({
          method: opts.method,
          url,
          headers: { ...headers },
          body,
        }),
      );
    }

    const doFetch =
      options.fetchImpl ?? ((...a: Parameters<typeof fetch>) => fetch(...a));
    let res: Response;
    try {
      res = await doFetch(url, {
        method: opts.method,
        headers,
        body,
        signal: opts.signal,
      });
    } catch (e) {
      if (e instanceof Error && e.name === 'AbortError') throw e;
      throw new ApiError(
        'network',
        e instanceof Error ? e.message : 'network error',
      );
    }

    const text = await res.text();
    if (!res.ok) throw parseErrorBody(res.status, text);
    try {
      return JSON.parse(text) as T;
    } catch {
      throw new ApiError(
        'invalid_response',
        'response is not JSON',
        res.status,
      );
    }
  }

  const enc = encodeURIComponent;
  return {
    getPrinter: (id, signal) =>
      request({
        method: 'GET',
        path: `/v1/printers/${enc(id)}`,
        authenticated: false,
        signal,
      }),
    createOrder: (req) =>
      request({
        method: 'POST',
        path: '/v1/orders',
        body: req,
        authenticated: true,
      }),
    getOrder: (id, signal) =>
      request({
        method: 'GET',
        path: `/v1/orders/${enc(id)}`,
        authenticated: true,
        signal,
      }),
    checkout: (id) =>
      request({
        method: 'POST',
        path: `/v1/orders/${enc(id)}/checkout`,
        authenticated: true,
      }),
    cancelOrder: (id) =>
      request({
        method: 'POST',
        path: `/v1/orders/${enc(id)}/cancel`,
        authenticated: true,
      }),
  };
}
