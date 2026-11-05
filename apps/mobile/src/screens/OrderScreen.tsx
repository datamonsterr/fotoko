import { useState } from 'react';
import { ActivityIndicator, StyleSheet, Text, View } from 'react-native';
import { useRouter } from 'expo-router';

import type { ApiClient } from '@/api';
import { getApiClient } from '@/api';
import { Button } from '@/components/Button';
import { ErrorBox } from '@/components/ErrorBox';
import { Screen } from '@/components/Screen';
import { StatusTimeline } from '@/components/StatusTimeline';
import { colors, space } from '@/components/theme';
import { useOrderPolling } from '@/hooks/useOrderPolling';
import { useI18n } from '@/i18n';
import { formatVnd } from '@/lib/price';
import { canCancel } from '@/lib/status';

export function OrderScreen({
  orderId,
  client = getApiClient,
  intervalMs,
}: {
  orderId: string;
  client?: () => ApiClient;
  intervalMs?: number;
}) {
  const { t } = useI18n();
  const router = useRouter();
  const { order, error, stopped, setOrder } = useOrderPolling(orderId, {
    client,
    intervalMs,
  });
  const [cancelling, setCancelling] = useState(false);
  const [cancelError, setCancelError] = useState<unknown>(null);

  const cancel = async () => {
    setCancelling(true);
    setCancelError(null);
    try {
      setOrder(await client().cancelOrder(orderId));
    } catch (e) {
      setCancelError(e);
    } finally {
      setCancelling(false);
    }
  };

  if (!order) {
    return (
      <Screen testID="order-screen">
        {error ? (
          <ErrorBox error={error} testID="order-error" />
        ) : (
          <ActivityIndicator
            testID="order-loading"
            accessibilityLabel={t('order.loading')}
          />
        )}
        {stopped ? (
          <Button
            variant="secondary"
            label={t('order.home')}
            testID="order-home"
            onPress={() => router.replace('/')}
          />
        ) : null}
      </Screen>
    );
  }

  const { status } = order;
  const outcome =
    status === 'completed'
      ? {
          testID: 'order-done',
          title: t('order.doneTitle'),
          body: t('order.doneBody'),
          tone: styles.success,
        }
      : status === 'failed'
        ? {
            testID: 'order-failed',
            title: t('order.failedTitle'),
            body: order.error?.message || t('order.failedBody'),
            tone: styles.danger,
          }
        : status === 'cancelled'
          ? {
              testID: 'order-cancelled',
              title: t('order.cancelledTitle'),
              body: '',
              tone: styles.danger,
            }
          : status === 'expired'
            ? {
                testID: 'order-expired',
                title: t('order.expiredTitle'),
                body: '',
                tone: styles.danger,
              }
            : null;

  return (
    <Screen testID="order-screen">
      <Text accessibilityRole="header" style={styles.title}>
        {t('order.title')}
      </Text>
      <Text testID="order-reference" style={styles.muted} selectable>
        {t('order.reference', { id: order.orderId })}
      </Text>
      <Text
        testID="order-status"
        accessibilityLiveRegion="polite"
        style={styles.status}
      >
        {t(`order.status.${status}`)}
      </Text>
      <Text testID="order-amount" style={styles.muted}>
        {t('order.amount')}: {formatVnd(order.amountVnd)}
      </Text>
      {outcome ? (
        <View
          testID={outcome.testID}
          accessibilityRole="alert"
          style={styles.outcome}
        >
          <Text style={[styles.outcomeTitle, outcome.tone]}>
            {outcome.title}
          </Text>
          {outcome.body ? (
            <Text style={styles.body}>{outcome.body}</Text>
          ) : null}
        </View>
      ) : null}
      <StatusTimeline status={status} />
      {error ? <ErrorBox error={error} testID="order-error" /> : null}
      {cancelError ? (
        <ErrorBox error={cancelError} testID="order-cancel-error" />
      ) : null}
      {canCancel(status) ? (
        <Button
          variant="danger"
          label={cancelling ? t('order.cancelling') : t('order.cancel')}
          testID="order-cancel"
          loading={cancelling}
          onPress={() => void cancel()}
        />
      ) : null}
      {stopped ? (
        <View style={styles.actions}>
          <Button
            label={t('order.again')}
            testID="order-again"
            onPress={() =>
              router.replace({
                pathname: '/p/[printerId]',
                params: { printerId: order.printerId },
              })
            }
          />
          <Button
            variant="secondary"
            label={t('order.home')}
            testID="order-home"
            onPress={() => router.replace('/')}
          />
        </View>
      ) : null}
    </Screen>
  );
}

const styles = StyleSheet.create({
  title: { fontSize: 26, fontWeight: '700', color: colors.text },
  muted: { color: colors.muted, fontSize: 15 },
  status: { fontSize: 20, fontWeight: '600', color: colors.text },
  outcome: {
    gap: space.xs,
    padding: space.md,
    borderRadius: 10,
    backgroundColor: colors.surface,
  },
  outcomeTitle: { fontSize: 20, fontWeight: '700' },
  body: { fontSize: 15, color: colors.text },
  success: { color: colors.success },
  danger: { color: colors.danger },
  actions: { gap: space.sm },
});
