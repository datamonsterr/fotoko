import type { OrderStatus } from '@/api/types';

import { canCancel, isTerminal, timeline } from './status';

const all: OrderStatus[] = [
  'awaiting_upload',
  'awaiting_payment',
  'paid',
  'printing',
  'completed',
  'failed',
  'cancelled',
  'expired',
];

describe('status helpers', () => {
  it('terminal statuses', () => {
    expect(all.filter(isTerminal)).toEqual([
      'completed',
      'failed',
      'cancelled',
      'expired',
    ]);
  });

  it('cancel only before paid', () => {
    expect(all.filter(canCancel)).toEqual([
      'awaiting_upload',
      'awaiting_payment',
    ]);
  });
});

describe('timeline', () => {
  const states = (s: OrderStatus) => timeline(s).map((t) => t.state);

  it('marks the current step with earlier ones done', () => {
    expect(states('awaiting_upload')).toEqual([
      'current',
      'pending',
      'pending',
      'pending',
      'pending',
    ]);
    expect(states('paid')).toEqual([
      'done',
      'done',
      'current',
      'pending',
      'pending',
    ]);
    expect(states('printing')).toEqual([
      'done',
      'done',
      'done',
      'current',
      'pending',
    ]);
  });

  it('completed is all done', () => {
    expect(states('completed')).toEqual([
      'done',
      'done',
      'done',
      'done',
      'done',
    ]);
  });

  it('failed stops after paid', () => {
    expect(states('failed')).toEqual([
      'done',
      'done',
      'done',
      'pending',
      'pending',
    ]);
  });

  it.each(['cancelled', 'expired'] as const)(
    '%s leaves everything pending',
    (s) => {
      expect(states(s)).toEqual([
        'pending',
        'pending',
        'pending',
        'pending',
        'pending',
      ]);
    },
  );
});
