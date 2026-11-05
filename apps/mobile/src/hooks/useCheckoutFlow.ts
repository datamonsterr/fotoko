import { useEffect, useState, useSyncExternalStore } from 'react';

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

export type CheckoutFlowHandle = FlowState & {
  start: () => void;
  retryPhoto: (index: number) => void;
  restart: () => void;
};

/**
 * React binding for CheckoutFlow. The photo list is captured at first render: the order is
 * created from that snapshot, so later edits to the draft do not change it. Deps are kept
 * current. Unmounting aborts in-flight uploads and cancels any pending checkout hand-over.
 */
export function useCheckoutFlow(
  printerId: string,
  draft: readonly DraftPhoto[],
  deps: CheckoutFlowDeps,
): CheckoutFlowHandle {
  const [flow] = useState(() => new CheckoutFlow(printerId, draft, deps));
  useEffect(() => {
    flow.setDeps(deps);
  });
  // arm() first: StrictMode re-runs this effect on the same instance after the cleanup.
  useEffect(() => {
    flow.arm();
    return flow.dispose;
  }, [flow]);
  const state = useSyncExternalStore(
    flow.subscribe,
    flow.getState,
    flow.getState,
  );
  return {
    ...state,
    start: flow.start,
    retryPhoto: flow.retryPhoto,
    restart: flow.restart,
  };
}
