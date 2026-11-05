import type { OrderStatus } from '@/api/types';

export const TERMINAL_STATUSES: readonly OrderStatus[] = [
  'completed',
  'failed',
  'cancelled',
  'expired',
];

export function isTerminal(status: OrderStatus): boolean {
  return TERMINAL_STATUSES.includes(status);
}

/** Cancel is allowed only before `paid`. */
export function canCancel(status: OrderStatus): boolean {
  return status === 'awaiting_upload' || status === 'awaiting_payment';
}

export const TIMELINE_STEPS = [
  'awaiting_upload',
  'awaiting_payment',
  'paid',
  'printing',
  'completed',
] as const;
export type TimelineStep = (typeof TIMELINE_STEPS)[number];
export type StepState = 'done' | 'current' | 'pending';

/**
 * Happy-path timeline. `failed` happens after payment: the steps up to `paid` are done and
 * `printing` is left pending. `cancelled`/`expired` only occur before payment and leave every
 * step pending (the screen shows a banner instead).
 */
export function timeline(
  status: OrderStatus,
): { step: TimelineStep; state: StepState }[] {
  let doneUpTo: number; // index of the last done step
  let current = -1;
  if (status === 'completed') doneUpTo = TIMELINE_STEPS.length - 1;
  else if (status === 'failed') doneUpTo = TIMELINE_STEPS.indexOf('paid');
  else if (status === 'cancelled' || status === 'expired') doneUpTo = -1;
  else {
    current = TIMELINE_STEPS.indexOf(status);
    doneUpTo = current - 1;
  }
  return TIMELINE_STEPS.map((step, i) => ({
    step,
    state: i <= doneUpTo ? 'done' : i === current ? 'current' : 'pending',
  }));
}
