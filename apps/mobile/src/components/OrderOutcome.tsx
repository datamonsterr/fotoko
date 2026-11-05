import { StyleSheet, Text, View } from 'react-native';

import type { Order } from '@/api/types';
import { useI18n, type MessageKey } from '@/i18n';

import { colors, space } from './theme';

interface Outcome {
  testID: string;
  title: MessageKey;
  /** Fallback body; a failed order prefers the server's own message. */
  body?: MessageKey;
  good: boolean;
}

const OUTCOMES: Partial<Record<Order['status'], Outcome>> = {
  completed: {
    testID: 'order-done',
    title: 'order.doneTitle',
    body: 'order.doneBody',
    good: true,
  },
  failed: {
    testID: 'order-failed',
    title: 'order.failedTitle',
    body: 'order.failedBody',
    good: false,
  },
  cancelled: {
    testID: 'order-cancelled',
    title: 'order.cancelledTitle',
    good: false,
  },
  expired: {
    testID: 'order-expired',
    title: 'order.expiredTitle',
    good: false,
  },
};

/** The result card of a terminal order; renders nothing for in-progress statuses. */
export function OrderOutcome({ order }: { order: Order }) {
  const { t } = useI18n();
  const outcome = OUTCOMES[order.status];
  if (!outcome) return null;
  const body =
    (order.status === 'failed' && order.error?.message) ||
    (outcome.body ? t(outcome.body) : '');
  return (
    <View testID={outcome.testID} accessibilityRole="alert" style={styles.card}>
      <Text style={[styles.title, outcome.good ? styles.good : styles.bad]}>
        {t(outcome.title)}
      </Text>
      {body ? <Text style={styles.body}>{body}</Text> : null}
    </View>
  );
}

const styles = StyleSheet.create({
  card: {
    gap: space.xs,
    padding: space.md,
    borderRadius: 10,
    backgroundColor: colors.surface,
  },
  title: { fontSize: 20, fontWeight: '700' },
  body: { fontSize: 15, color: colors.text },
  good: { color: colors.success },
  bad: { color: colors.danger },
});
