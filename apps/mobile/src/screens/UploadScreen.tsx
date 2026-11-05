import { useEffect } from 'react';
import {
  ActivityIndicator,
  BackHandler,
  Platform,
  StyleSheet,
  Text,
} from 'react-native';
import { Stack, useRouter } from 'expo-router';

import { Button } from '@/components/Button';
import { ErrorBox } from '@/components/ErrorBox';
import { Screen } from '@/components/Screen';
import { UploadPhotoRow } from '@/components/UploadPhotoRow';
import { colors, space } from '@/components/theme';
import { useCheckoutFlow } from '@/hooks/useCheckoutFlow';
import { useI18n } from '@/i18n';
import { openCheckoutUrl } from '@/lib/checkout';
import { usePhotoDraft } from '@/state/draft';

export function UploadScreen({ printerId }: { printerId: string }) {
  const { t } = useI18n();
  const router = useRouter();
  const { photos: draft } = usePhotoDraft();
  const flow = useCheckoutFlow(printerId, draft, {
    onCheckout: async (url, orderId) => {
      await openCheckoutUrl(url);
      // Web leaves the page: payOS returns to /orders/{id}, and the order screen clears the draft.
      if (Platform.OS === 'web') return;
      router.replace({ pathname: '/orders/[orderId]', params: { orderId } });
    },
  });

  const { start } = flow;
  useEffect(() => {
    start();
  }, [start]);

  // Leaving mid-flight would abandon uploads (and could skip the payment hand-over).
  const working =
    flow.phase === 'creating' ||
    flow.phase === 'uploading' ||
    flow.phase === 'checkingOut' ||
    flow.phase === 'redirected';
  useEffect(() => {
    if (!working) return;
    const sub = BackHandler.addEventListener('hardwareBackPress', () => true);
    return () => sub.remove();
  }, [working]);

  const spinner =
    flow.phase === 'creating' ||
    flow.phase === 'checkingOut' ||
    flow.phase === 'redirected';
  const phaseKey =
    flow.phase === 'creating'
      ? 'upload.creating'
      : spinner
        ? 'upload.checkingOut'
        : 'upload.title';
  const flowFailed = flow.phase === 'error' && flow.error != null;

  return (
    <Screen testID="upload-screen">
      <Stack.Screen
        options={{ gestureEnabled: !working, headerBackVisible: !working }}
      />
      <Text
        testID="upload-phase"
        accessibilityRole="header"
        accessibilityLiveRegion="polite"
        style={styles.title}
      >
        {t(phaseKey)}
      </Text>
      {spinner ? (
        <ActivityIndicator
          testID="upload-busy"
          accessibilityLabel={t(phaseKey)}
        />
      ) : null}
      {flow.photos.map((p, i) => (
        <UploadPhotoRow
          key={i}
          photo={p}
          index={i}
          retryDisabled={working}
          onRetry={() => flow.retryPhoto(i)}
        />
      ))}
      {flow.phase === 'expired' ? (
        <>
          <Text
            testID="upload-expired"
            accessibilityRole="alert"
            style={styles.error}
          >
            {t('upload.expired')}
          </Text>
          <Button
            label={t('upload.restart')}
            testID="upload-restart"
            onPress={flow.restart}
          />
        </>
      ) : null}
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
  error: { color: colors.danger, fontSize: 15, marginBottom: space.xs },
});
