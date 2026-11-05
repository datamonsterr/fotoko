import { AppState } from 'react-native';
import { act, renderHook } from '@testing-library/react-native';

import {
  ApiError,
  setApiClient,
  type ApiClient,
  type Order,
  type OrderStatus,
} from '@/api';
import { order } from '@/test/fixtures';

import { useOrderPolling } from './useOrderPolling';

const at = (status: OrderStatus): Order => ({ ...order, status });

function setup(results: (Order | Error)[]) {
  const getOrder = jest.fn(async () => {
    const r = results.length > 1 ? results.shift() : results[0];
    if (r instanceof Error) throw r;
    return r as Order;
  });
  setApiClient({ getOrder } as unknown as ApiClient);
  return { getOrder };
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
    const { getOrder } = setup([at('awaiting_payment')]);
    const { result } = await renderHook(() => useOrderPolling('ord-1'));
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
      const { getOrder } = setup([at('paid'), at(status)]);
      const { result } = await renderHook(() => useOrderPolling('ord-1'));
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
      const { getOrder } = setup([at(status)]);
      await renderHook(() => useOrderPolling('ord-1'));
      await flush();
      await advance(6100);
      expect(getOrder).toHaveBeenCalledTimes(4);
    },
  );

  it('honours a custom interval', async () => {
    const { getOrder } = setup([at('paid')]);
    await renderHook(() => useOrderPolling('ord-1', { intervalMs: 500 }));
    await flush();
    await advance(1000);
    expect(getOrder).toHaveBeenCalledTimes(3);
  });

  it('survives a transient error and clears it on the next success', async () => {
    const { getOrder } = setup([new ApiError('network', 'down'), at('paid')]);
    const { result } = await renderHook(() => useOrderPolling('ord-1'));
    await flush();
    expect(result.current.error).toMatchObject({ code: 'network' });
    expect(result.current.stopped).toBe(false);
    await advance(4000); // one failure: the next poll waits 4 s
    expect(result.current.error).toBeNull();
    expect(result.current.order?.status).toBe('paid');
    expect(getOrder).toHaveBeenCalledTimes(2);
  });

  it.each(['not_found', 'forbidden', 'validation'])(
    'stops on permanent error %s',
    async (code) => {
      const { getOrder } = setup([new ApiError(code, 'x')]);
      const { result } = await renderHook(() => useOrderPolling('ord-1'));
      await flush();
      await advance(10_000);
      expect(result.current.stopped).toBe(true);
      expect(getOrder).toHaveBeenCalledTimes(1);
    },
  );

  it('keeps polling after a non-API error', async () => {
    const { getOrder } = setup([new Error('boom')]);
    await renderHook(() => useOrderPolling('ord-1'));
    await flush();
    await advance(4200);
    expect(getOrder).toHaveBeenCalledTimes(2);
  });

  it('does not overlap requests: waits for the response before scheduling', async () => {
    let release: (o: Order) => void = () => {};
    const getOrder = jest.fn(() => new Promise<Order>((r) => (release = r)));
    setApiClient({ getOrder } as unknown as ApiClient);
    await renderHook(() => useOrderPolling('ord-1'));
    await advance(10_000);
    expect(getOrder).toHaveBeenCalledTimes(1);
    await act(async () => release(at('paid')));
    await advance(2000);
    expect(getOrder).toHaveBeenCalledTimes(2);
  });

  it('stops polling on unmount and ignores late responses', async () => {
    let release: (o: Order) => void = () => {};
    const getOrder = jest.fn(() => new Promise<Order>((r) => (release = r)));
    setApiClient({ getOrder } as unknown as ApiClient);
    const { unmount } = await renderHook(() => useOrderPolling('ord-1'));
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
    setApiClient({ getOrder } as unknown as ApiClient);
    const { unmount } = await renderHook(() => useOrderPolling('ord-1'));
    await unmount();
    await act(async () => fail(new Error('late')));
    await advance(5000);
    expect(getOrder).toHaveBeenCalledTimes(1);
  });

  it('setOrder replaces the order; a terminal one stops polling', async () => {
    const { getOrder } = setup([at('awaiting_payment')]);
    const { result } = await renderHook(() => useOrderPolling('ord-1'));
    await flush();
    await act(async () => result.current.setOrder(at('awaiting_upload')));
    expect(result.current.order?.status).toBe('awaiting_upload');
    expect(result.current.stopped).toBe(false);
    await act(async () => result.current.setOrder(at('cancelled')));
    expect(result.current.stopped).toBe(true);
    await advance(10_000);
    expect(getOrder).toHaveBeenCalledTimes(1);
  });

  describe('failures', () => {
    it('backs off 4 s, 8 s, then caps at 10 s, and resets after a success', async () => {
      const { getOrder } = setup([
        new ApiError('network', 'x'),
        new ApiError('network', 'x'),
        new ApiError('network', 'x'),
        new ApiError('network', 'x'),
        at('paid'),
      ]);
      await renderHook(() => useOrderPolling('ord-1'));
      await flush();
      expect(getOrder).toHaveBeenCalledTimes(1);
      await advance(3800);
      expect(getOrder).toHaveBeenCalledTimes(1);
      await advance(400); // ~4.2 s: second call
      expect(getOrder).toHaveBeenCalledTimes(2);
      await advance(7600);
      expect(getOrder).toHaveBeenCalledTimes(2);
      await advance(400); // 8 s after the second: third
      expect(getOrder).toHaveBeenCalledTimes(3);
      await advance(9400);
      expect(getOrder).toHaveBeenCalledTimes(3);
      await advance(600); // capped at 10 s, not 16 s: fourth
      expect(getOrder).toHaveBeenCalledTimes(4);
      await advance(9400);
      expect(getOrder).toHaveBeenCalledTimes(4);
      await advance(600); // fifth succeeds
      expect(getOrder).toHaveBeenCalledTimes(5);
      await advance(2200); // back to the normal 2 s
      expect(getOrder).toHaveBeenCalledTimes(6);
    });

    it('a first auth failure is retried; a second in a row stops with retry() available', async () => {
      const { getOrder } = setup([new ApiError('auth', 'Forbidden', 403)]);
      const { result } = await renderHook(() => useOrderPolling('ord-1'));
      await flush();
      expect(result.current.stopped).toBe(false);
      await advance(4200);
      expect(getOrder).toHaveBeenCalledTimes(2);
      expect(result.current.stopped).toBe(true);
      expect(result.current.error).toMatchObject({ code: 'auth' });
      await advance(30_000);
      expect(getOrder).toHaveBeenCalledTimes(2); // not spinning

      getOrder.mockResolvedValue(at('paid'));
      await act(async () => result.current.retry());
      await flush();
      expect(result.current.stopped).toBe(false);
      expect(result.current.error).toBeNull();
      expect(result.current.order?.status).toBe('paid');
    });

    it('an auth failure followed by a success resets the auth counter', async () => {
      const { getOrder } = setup([
        new ApiError('auth', 'x', 403),
        at('paid'),
        new ApiError('auth', 'x', 403),
        at('paid'),
      ]);
      const { result } = await renderHook(() => useOrderPolling('ord-1'));
      await flush();
      await advance(4200);
      await advance(2200);
      await advance(4200);
      expect(getOrder.mock.calls.length).toBeGreaterThanOrEqual(4);
      expect(result.current.stopped).toBe(false);
    });

    it('a non-auth failure between two auth failures resets the counter', async () => {
      const { result } = await (async () => {
        setup([
          new ApiError('auth', 'x', 403),
          new ApiError('network', 'x'),
          new ApiError('auth', 'x', 403),
          at('paid'),
        ]);
        return renderHook(() => useOrderPolling('ord-1'));
      })();
      await flush();
      await advance(4200);
      await advance(8200);
      expect(result.current.stopped).toBe(false);
    });
  });

  describe('app state', () => {
    type Listener = (s: string) => void;
    let listener: Listener = () => {};
    const remove = jest.fn();
    const addEventListener = AppState.addEventListener as jest.Mock;
    beforeEach(() => {
      remove.mockClear();
      addEventListener.mockImplementation(((_: string, l: Listener) => {
        listener = l;
        return { remove };
      }) as never);
    });
    afterEach(() =>
      addEventListener.mockImplementation(() => ({ remove: jest.fn() })),
    );

    it('pauses in the background and polls immediately on return', async () => {
      const { getOrder } = setup([at('paid')]);
      await renderHook(() => useOrderPolling('ord-1'));
      await flush();
      expect(getOrder).toHaveBeenCalledTimes(1);
      await act(async () => listener('background'));
      await advance(20_000);
      expect(getOrder).toHaveBeenCalledTimes(1);
      await act(async () => listener('active'));
      await flush();
      expect(getOrder).toHaveBeenCalledTimes(2);
      await advance(2100);
      expect(getOrder).toHaveBeenCalledTimes(3);
    });

    it('does not double-poll when told active while already active', async () => {
      const { getOrder } = setup([at('paid')]);
      await renderHook(() => useOrderPolling('ord-1'));
      await flush();
      await act(async () => listener('active'));
      await flush();
      expect(getOrder).toHaveBeenCalledTimes(1);
    });

    it('a response that lands while backgrounded does not reschedule', async () => {
      let release: (o: Order) => void = () => {};
      const getOrder = jest.fn(() => new Promise<Order>((r) => (release = r)));
      setApiClient({ getOrder } as unknown as ApiClient);
      await renderHook(() => useOrderPolling('ord-1'));
      await act(async () => listener('inactive'));
      await act(async () => release(at('paid')));
      await advance(10_000);
      expect(getOrder).toHaveBeenCalledTimes(1);
      await act(async () => listener('active'));
      expect(getOrder).toHaveBeenCalledTimes(2);
    });

    it('removes its AppState listener on unmount', async () => {
      setup([at('paid')]);
      const { unmount } = await renderHook(() => useOrderPolling('ord-1'));
      await unmount();
      expect(remove).toHaveBeenCalled();
    });

    it('starts paused when the app is already in the background', async () => {
      const original = Object.getOwnPropertyDescriptor(
        AppState,
        'currentState',
      );
      Object.defineProperty(AppState, 'currentState', {
        value: 'background',
        configurable: true,
      });
      const { getOrder } = setup([at('paid')]);
      await renderHook(() => useOrderPolling('ord-1'));
      await advance(5000);
      expect(getOrder).not.toHaveBeenCalled();
      if (original) Object.defineProperty(AppState, 'currentState', original);
    });
  });

  it('uses the app-wide client', async () => {
    const getOrder = jest.fn(async () => at('completed'));
    setApiClient({ getOrder } as unknown as ApiClient);
    await renderHook(() => useOrderPolling('ord-1'));
    await flush();
    expect(getOrder).toHaveBeenCalled();
  });
});
