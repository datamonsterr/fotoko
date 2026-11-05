import { useCallback, useEffect, useRef, useState } from 'react';
import { AppState } from 'react-native';

import { ApiError, getApiClient, type Order } from '@/api';
import { POLL_INTERVAL_MS } from '@/lib/limits';
import { isTerminal } from '@/lib/status';

export interface OrderPolling {
  order: Order | null;
  /** Last fetch error; cleared by the next success. */
  error: unknown;
  /** True once polling stopped: terminal status or an error that will not fix itself. */
  stopped: boolean;
  /** Replace the cached order (e.g. with the cancel response). */
  setOrder: (o: Order) => void;
  /** Start polling again after it stopped on an error. */
  retry: () => void;
}

/** Codes that will not fix themselves; stop polling on them. */
const PERMANENT = ['not_found', 'forbidden', 'validation'];
/** Failed polls back off up to this delay. */
export const MAX_BACKOFF_MS = 10_000;

// Anything that is not a known background state counts as active (jest's mock has no state).
const isActive = (s: unknown) =>
  typeof s !== 'string' || s === 'active' || s === 'unknown';

/**
 * Polls GET /v1/orders/{id} every `intervalMs`, waiting for each response before scheduling the
 * next (no overlap). Stops on a terminal status or a permanent error.
 *
 * - Failures back off exponentially (interval * 2^n, capped at 10 s); a success resets it.
 * - A gateway auth failure (`auth`) is tried once more (the client already refreshed its
 *   credentials and clock for that request); a second one in a row stops polling with the
 *   error shown, and `retry()` resumes it.
 * - Polling pauses while the app is in the background (or the web tab hidden) and polls
 *   immediately on return.
 *
 * State starts empty; mount one instance per order id (the route does, via `key`).
 */
export function useOrderPolling(
  orderId: string,
  options: { intervalMs?: number } = {},
): OrderPolling {
  const { intervalMs = POLL_INTERVAL_MS } = options;
  const [order, setOrderState] = useState<Order | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [stopped, setStopped] = useState(false);
  const [nonce, setNonce] = useState(0);
  const stopRef = useRef<() => void>(() => {});

  useEffect(() => {
    let cancelled = false;
    let done = false;
    let inflight = false;
    let paused = !isActive(AppState.currentState);
    let failures = 0;
    let authFailures = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const ctrl = new AbortController();

    const schedule = (delay: number) => {
      if (cancelled || done || paused) return;
      timer = setTimeout(() => void tick(), delay);
    };

    const tick = async () => {
      timer = undefined;
      if (cancelled || done || paused || inflight) return;
      inflight = true;
      let delay = intervalMs;
      try {
        const next = await getApiClient().getOrder(orderId, ctrl.signal);
        if (cancelled) return;
        failures = 0;
        authFailures = 0;
        setOrderState(next);
        setError(null);
        if (isTerminal(next.status)) {
          done = true;
          setStopped(true);
          return;
        }
      } catch (e) {
        if (cancelled) return;
        setError(e);
        failures++;
        delay = Math.min(intervalMs * 2 ** failures, MAX_BACKOFF_MS);
        const code = e instanceof ApiError ? e.code : '';
        if (code === 'auth') authFailures++;
        else authFailures = 0;
        if (PERMANENT.includes(code) || authFailures >= 2) {
          done = true;
          setStopped(true);
          return;
        }
      } finally {
        inflight = false;
      }
      schedule(delay);
    };

    const sub = AppState.addEventListener('change', (state) => {
      if (isActive(state)) {
        if (!paused) return;
        paused = false;
        if (timer === undefined) void tick();
      } else {
        paused = true;
        if (timer) clearTimeout(timer);
        timer = undefined;
      }
    });

    void tick();

    stopRef.current = () => {
      cancelled = true;
      ctrl.abort();
      sub.remove();
      if (timer) clearTimeout(timer);
    };
    return () => stopRef.current();
  }, [orderId, intervalMs, nonce]);

  const setOrder = useCallback((o: Order) => {
    setOrderState(o);
    if (isTerminal(o.status)) {
      stopRef.current();
      setStopped(true);
    }
  }, []);

  const retry = useCallback(() => {
    setError(null);
    setStopped(false);
    setNonce((n) => n + 1);
  }, []);

  return { order, error, stopped, setOrder, retry };
}
