import { useState, useSyncExternalStore } from 'react';

import type { DraftPhoto } from '@/lib/photos';

import {
  CheckoutFlow,
  type CheckoutFlowDeps,
  type FlowState,
} from './checkoutFlow';

export type {
  CheckoutFlowDeps,
  FlowPhase,
  FlowState,
  PhotoUploadState,
} from './checkoutFlow';

/**
 * React binding for CheckoutFlow. The photo list and deps are captured at first render: the
 * order is created from that snapshot, so later edits to the draft do not change it.
 */
export function useCheckoutFlow(
  printerId: string,
  draft: readonly DraftPhoto[],
  deps: CheckoutFlowDeps,
): FlowState & { start: () => void; retryPhoto: (index: number) => void } {
  const [flow] = useState(() => new CheckoutFlow(printerId, draft, deps));
  const state = useSyncExternalStore(
    flow.subscribe,
    flow.getState,
    flow.getState,
  );
  return { ...state, start: flow.start, retryPhoto: flow.retryPhoto };
}
