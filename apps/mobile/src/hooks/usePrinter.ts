import { useCallback, useEffect, useState } from 'react';

import { getApiClient, type Printer } from '@/api';

export type PrinterState =
  | { phase: 'loading' }
  | { phase: 'ready'; printer: Printer }
  | { phase: 'error'; error: unknown };

type Settled = {
  key: string;
  result: Exclude<PrinterState, { phase: 'loading' }>;
};

export function usePrinter(printerId: string): {
  state: PrinterState;
  reload: () => void;
} {
  const [settled, setSettled] = useState<Settled | null>(null);
  const [nonce, setNonce] = useState(0);
  const key = `${printerId}:${nonce}`;

  useEffect(() => {
    const ctrl = new AbortController();
    getApiClient()
      .getPrinter(printerId, ctrl.signal)
      .then(
        (printer) =>
          !ctrl.signal.aborted &&
          setSettled({ key, result: { phase: 'ready', printer } }),
        (error: unknown) =>
          !ctrl.signal.aborted &&
          setSettled({ key, result: { phase: 'error', error } }),
      );
    return () => ctrl.abort();
  }, [key, printerId]);

  const reload = useCallback(() => setNonce((n) => n + 1), []);
  // A result for an older key (id changed, reload pressed) counts as loading.
  const state: PrinterState =
    settled?.key === key ? settled.result : { phase: 'loading' };
  return { state, reload };
}
