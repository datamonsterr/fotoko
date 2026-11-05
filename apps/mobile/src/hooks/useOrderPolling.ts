import { useCallback, useEffect, useRef, useState } from 'react';

import { ApiError, getApiClient, type ApiClient, type Order } from '@/api';
import { POLL_INTERVAL_MS } from '@/lib/limits';
import { isTerminal } from '@/lib/status';

export interface OrderPolling {
  order: Order | null;
  /** Last fetch error; cleared by the next success. Polling continues after transient errors. */
  error: unknown;
  /** True once polling stopped: terminal status or a permanent error. */
  stopped: boolean;
  /** Replace the cached order (e.g. with the cancel response). */
  setOrder: (o: Order) => void;
}

/** Codes that will not fix themselves; stop polling on them. */
const PERMANENT = ['not_found', 'forbidden', 'validation'];

/**
 * Polls GET /v1/orders/{id} every `intervalMs`, waiting for each response before scheduling the
 * next (no overlap). Stops on a terminal status or a permanent error. State starts empty; mount
 * one instance per order id (the route does).
 */
export function useOrderPolling(
  orderId: string,
  options: { client?: () => ApiClient; intervalMs?: number } = {},
): OrderPolling {
  const { client = getApiClient, intervalMs = POLL_INTERVAL_MS } = options;
  const [order, setOrderState] = useState<Order | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [stopped, setStopped] = useState(false);
  const stopRef = useRef<() => void>(() => {});

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const ctrl = new AbortController();

    const tick = async () => {
      try {
        const next = await client().getOrder(orderId, ctrl.signal);
        if (cancelled) return;
        setOrderState(next);
        setError(null);
        if (isTerminal(next.status)) {
          setStopped(true);
          return;
        }
      } catch (e) {
        if (cancelled) return;
        setError(e);
        if (e instanceof ApiError && PERMANENT.includes(e.code)) {
          setStopped(true);
          return;
        }
      }
      timer = setTimeout(tick, intervalMs);
    };
    void tick();

    stopRef.current = () => {
      cancelled = true;
      ctrl.abort();
      if (timer) clearTimeout(timer);
    };
    return () => stopRef.current();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [orderId, intervalMs]);

  const setOrder = useCallback((o: Order) => {
    setOrderState(o);
    if (isTerminal(o.status)) {
      stopRef.current();
      setStopped(true);
    }
  }, []);

  return { order, error, stopped, setOrder };
}
