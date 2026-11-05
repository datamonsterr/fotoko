import { act, renderHook } from '@testing-library/react-native';

import { ApiError, type ApiClient, type Order, type OrderStatus } from '@/api';
import { order } from '@/test/fixtures';

import { useOrderPolling } from './useOrderPolling';

const at = (status: OrderStatus): Order => ({ ...order, status });

function setup(results: (Order | Error)[]) {
  const getOrder = jest.fn(async () => {
    const r = results.length > 1 ? results.shift() : results[0];
    if (r instanceof Error) throw r;
    return r as Order;
  });
  const client = () => ({ getOrder }) as unknown as ApiClient;
  return { getOrder, client };
}

const flush = () => act(async () => {});
// A poll needs several microtask hops after its timer fires before it schedules the next one,
// so move time in small steps and let promises settle between them.
const advance = async (ms: number) => {
  for (let left = ms; left > 0; left -= 50) {
    await act(async () => {
      await jest.advanceTimersByTimeAsync(Math.min(50, left));
      for (let i = 0; i < 10; i++) await Promise.resolve();
    });
  }
};

beforeEach(() => jest.useFakeTimers());

describe('useOrderPolling', () => {
  it('fetches immediately, then every 2 s by default', async () => {
    const { getOrder, client } = setup([at('awaiting_payment')]);
    const { result } = await renderHook(() =>
      useOrderPolling('ord-1', { client }),
    );
    await flush();
    expect(getOrder).toHaveBeenCalledTimes(1);
    expect(result.current.order?.status).toBe('awaiting_payment');
    await advance(1999);
    expect(getOrder).toHaveBeenCalledTimes(1);
    await advance(1);
    expect(getOrder).toHaveBeenCalledTimes(2);
    await advance(2000);
    expect(getOrder).toHaveBeenCalledTimes(3);
    expect(result.current.stopped).toBe(false);
  });

  it.each(['completed', 'failed', 'cancelled', 'expired'] as const)(
    'stops on terminal %s',
    async (status) => {
      const { getOrder, client } = setup([at('paid'), at(status)]);
      const { result } = await renderHook(() =>
        useOrderPolling('ord-1', { client }),
      );
      await flush();
      await advance(2000);
      expect(result.current.order?.status).toBe(status);
      expect(result.current.stopped).toBe(true);
      await advance(10_000);
      expect(getOrder).toHaveBeenCalledTimes(2);
    },
  );

  it.each(['awaiting_upload', 'awaiting_payment', 'paid', 'printing'] as const)(
    'keeps polling on %s',
    async (status) => {
      const { getOrder, client } = setup([at(status)]);
      await renderHook(() => useOrderPolling('ord-1', { client }));
      await flush();
      await advance(6100);
      expect(getOrder).toHaveBeenCalledTimes(4);
    },
  );

  it('honours a custom interval', async () => {
    const { getOrder, client } = setup([at('paid')]);
    await renderHook(() =>
      useOrderPolling('ord-1', { client, intervalMs: 500 }),
    );
    await flush();
    await advance(1000);
    expect(getOrder).toHaveBeenCalledTimes(3);
  });

  it('survives a transient error and clears it on the next success', async () => {
    const { getOrder, client } = setup([
      new ApiError('network', 'down'),
      at('paid'),
    ]);
    const { result } = await renderHook(() =>
      useOrderPolling('ord-1', { client }),
    );
    await flush();
    expect(result.current.error).toMatchObject({ code: 'network' });
    expect(result.current.stopped).toBe(false);
    await advance(2000);
    expect(result.current.error).toBeNull();
    expect(result.current.order?.status).toBe('paid');
    expect(getOrder).toHaveBeenCalledTimes(2);
  });

  it.each(['not_found', 'forbidden', 'validation'])(
    'stops on permanent error %s',
    async (code) => {
      const { getOrder, client } = setup([new ApiError(code, 'x')]);
      const { result } = await renderHook(() =>
        useOrderPolling('ord-1', { client }),
      );
      await flush();
      await advance(10_000);
      expect(result.current.stopped).toBe(true);
      expect(getOrder).toHaveBeenCalledTimes(1);
    },
  );

  it('keeps polling after a non-API error', async () => {
    const { getOrder, client } = setup([new Error('boom')]);
    await renderHook(() => useOrderPolling('ord-1', { client }));
    await flush();
    await advance(2000);
    expect(getOrder).toHaveBeenCalledTimes(2);
  });

  it('does not overlap requests: waits for the response before scheduling', async () => {
    let release: (o: Order) => void = () => {};
    const getOrder = jest.fn(() => new Promise<Order>((r) => (release = r)));
    await renderHook(() =>
      useOrderPolling('ord-1', {
        client: () => ({ getOrder }) as unknown as ApiClient,
      }),
    );
    await advance(10_000);
    expect(getOrder).toHaveBeenCalledTimes(1);
    await act(async () => release(at('paid')));
    await advance(2000);
    expect(getOrder).toHaveBeenCalledTimes(2);
  });

  it('stops polling on unmount and ignores late responses', async () => {
    let release: (o: Order) => void = () => {};
    const getOrder = jest.fn(() => new Promise<Order>((r) => (release = r)));
    const { unmount } = await renderHook(() =>
      useOrderPolling('ord-1', {
        client: () => ({ getOrder }) as unknown as ApiClient,
      }),
    );
    await unmount();
    await act(async () => release(at('paid')));
    await advance(10_000);
    expect(getOrder).toHaveBeenCalledTimes(1);
  });

  it('ignores a late error after unmount', async () => {
    let fail: (e: Error) => void = () => {};
    const getOrder = jest.fn(
      () => new Promise<Order>((_, rej) => (fail = rej)),
    );
    const { unmount } = await renderHook(() =>
      useOrderPolling('ord-1', {
        client: () => ({ getOrder }) as unknown as ApiClient,
      }),
    );
    await unmount();
    await act(async () => fail(new Error('late')));
    await advance(5000);
    expect(getOrder).toHaveBeenCalledTimes(1);
  });

  it('setOrder replaces the order; a terminal one stops polling', async () => {
    const { getOrder, client } = setup([at('awaiting_payment')]);
    const { result } = await renderHook(() =>
      useOrderPolling('ord-1', { client }),
    );
    await flush();
    await act(async () => result.current.setOrder(at('awaiting_upload')));
    expect(result.current.order?.status).toBe('awaiting_upload');
    expect(result.current.stopped).toBe(false);
    await act(async () => result.current.setOrder(at('cancelled')));
    expect(result.current.stopped).toBe(true);
    await advance(10_000);
    expect(getOrder).toHaveBeenCalledTimes(1);
  });

  it('uses the default client accessor', async () => {
    const api = jest.requireActual('@/api') as typeof import('@/api');
    const getOrder = jest.fn(async () => at('completed'));
    api.setApiClient({ getOrder } as unknown as ApiClient);
    await renderHook(() => useOrderPolling('ord-1'));
    await flush();
    expect(getOrder).toHaveBeenCalled();
    api.setApiClient(undefined);
  });
});
