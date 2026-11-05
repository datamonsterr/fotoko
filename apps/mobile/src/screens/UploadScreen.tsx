import { useEffect } from 'react';
import {
  ActivityIndicator,
  Platform,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { useRouter } from 'expo-router';

import type { ApiClient } from '@/api';
import type { UploadArgs } from '@/api/upload';
import { Button } from '@/components/Button';
import { ErrorBox } from '@/components/ErrorBox';
import { Screen } from '@/components/Screen';
import { colors, space } from '@/components/theme';
import { useCheckoutFlow } from '@/hooks/useCheckoutFlow';
import { useI18n } from '@/i18n';
import { openCheckoutUrl } from '@/lib/checkout';
import { usePhotoDraft } from '@/state/draft';

export function UploadScreen({
  printerId,
  client,
  upload,
  openCheckout = openCheckoutUrl,
}: {
  printerId: string;
  client?: () => ApiClient;
  upload?: (args: UploadArgs) => Promise<void>;
  openCheckout?: (url: string) => Promise<void>;
}) {
  const { t } = useI18n();
  const router = useRouter();
  const { photos: draft } = usePhotoDraft();
  const flow = useCheckoutFlow(printerId, draft, {
    client,
    upload,
    onCheckout: async (url, orderId) => {
      await openCheckout(url);
      // Web leaves the page: payOS returns to /orders/{id}, and the order screen clears the draft.
      if (Platform.OS === 'web') return;
      router.replace({ pathname: '/orders/[orderId]', params: { orderId } });
    },
  });

  const { start } = flow;
  useEffect(() => {
    start();
  }, [start]);

  const busy =
    flow.phase === 'creating' ||
    flow.phase === 'checkingOut' ||
    flow.phase === 'redirected';
  const phaseKey =
    flow.phase === 'creating'
      ? 'upload.creating'
      : busy
        ? 'upload.checkingOut'
        : 'upload.title';
  const flowFailed = flow.phase === 'error' && flow.error != null;

  return (
    <Screen testID="upload-screen">
      <Text
        testID="upload-phase"
        accessibilityRole="header"
        accessibilityLiveRegion="polite"
        style={styles.title}
      >
        {t(phaseKey)}
      </Text>
      {busy ? (
        <ActivityIndicator
          testID="upload-busy"
          accessibilityLabel={t(phaseKey)}
        />
      ) : null}
      {flow.photos.map((p, i) => {
        const n = i + 1;
        const percent = Math.round(p.progress * 100);
        return (
          <View key={i} testID={`upload-photo-${i}`} style={styles.row}>
            <Text style={styles.name}>{t('upload.photo', { n })}</Text>
            <Text
              testID={`upload-photo-status-${i}`}
              accessibilityRole="progressbar"
              accessibilityValue={{ min: 0, max: 100, now: percent }}
              style={p.status === 'error' ? styles.error : styles.status}
            >
              {p.status === 'pending'
                ? t('upload.pending')
                : p.status === 'uploading'
                  ? t('upload.uploading', { percent })
                  : p.status === 'done'
                    ? t('upload.done')
                    : t('upload.failed')}
            </Text>
            {p.status === 'error' ? (
              <Button
                variant="secondary"
                label={t('upload.retry')}
                accessibilityLabel={`${t('upload.retry')} ${t('upload.photo', { n })}`}
                testID={`upload-photo-retry-${i}`}
                disabled={busy}
                onPress={() => flow.retryPhoto(i)}
              />
            ) : null}
          </View>
        );
      })}
      {flowFailed ? (
        <ErrorBox
          error={flow.error}
          testID="upload-error"
          onRetry={flow.start}
          retryLabel={t('upload.retryOrder')}
          retryTestID="upload-retry"
        />
      ) : null}
      {flow.phase === 'idle' ? (
        <Button
          variant="secondary"
          label={t('upload.back')}
          testID="upload-back"
          onPress={() => router.back()}
        />
      ) : null}
    </Screen>
  );
}

const styles = StyleSheet.create({
  title: { fontSize: 22, fontWeight: '700', color: colors.text },
  row: {
    gap: space.xs,
    padding: space.sm,
    backgroundColor: colors.surface,
    borderRadius: 10,
  },
  name: { fontSize: 15, color: colors.text },
  status: { color: colors.muted },
  error: { color: colors.danger },
});
