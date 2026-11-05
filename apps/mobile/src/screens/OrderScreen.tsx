import { useEffect, useState } from 'react';
import { ActivityIndicator, StyleSheet, Text, View } from 'react-native';
import { useRouter } from 'expo-router';

import { ApiError, getApiClient } from '@/api';
import { Button } from '@/components/Button';
import { ErrorBox } from '@/components/ErrorBox';
import { OrderOutcome } from '@/components/OrderOutcome';
import { Screen } from '@/components/Screen';
import { StatusTimeline } from '@/components/StatusTimeline';
import { colors, space } from '@/components/theme';
import { useOrderPolling } from '@/hooks/useOrderPolling';
import { useI18n } from '@/i18n';
import { formatVnd } from '@/lib/price';
import { canCancel } from '@/lib/status';
import { usePhotoDraft } from '@/state/draft';

export function OrderScreen({ orderId }: { orderId: string }) {
  const { t } = useI18n();
  const router = useRouter();
  const { order, error, stopped, setOrder, retry } = useOrderPolling(orderId);
  const { dispatch } = usePhotoDraft();
  const [cancelling, setCancelling] = useState(false);
  const [cancelError, setCancelError] = useState<unknown>(null);

  // Once the order is past upload the photos are on the server; a failed payOS load keeps them.
  const loadedStatus = order?.status;
  useEffect(() => {
    if (loadedStatus && loadedStatus !== 'awaiting_upload') {
      dispatch({ type: 'clear' });
    }
  }, [loadedStatus, dispatch]);

  const cancel = async () => {
    setCancelling(true);
    setCancelError(null);
    try {
      setOrder(await getApiClient().cancelOrder(orderId));
    } catch (e) {
      setCancelError(e);
    } finally {
      setCancelling(false);
    }
  };

  // Polling gave up on a session problem: let the user try again instead of a dead end.
  const canRetry =
    stopped && error instanceof ApiError && error.code === 'auth';

  if (!order) {
    return (
      <Screen testID="order-screen">
        {error ? (
          <ErrorBox
            error={error}
            testID="order-error"
            onRetry={canRetry ? retry : undefined}
            retryTestID="order-retry"
          />
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
      <OrderOutcome order={order} />
      <StatusTimeline status={status} />
      {error ? (
        <ErrorBox
          error={error}
          testID="order-error"
          onRetry={canRetry ? retry : undefined}
          retryTestID="order-retry"
        />
      ) : null}
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
  actions: { gap: space.sm },
});
