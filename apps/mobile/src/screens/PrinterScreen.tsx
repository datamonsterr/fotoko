import { ActivityIndicator, StyleSheet, Text, View } from 'react-native';
import { useRouter } from 'expo-router';

import { ApiError } from '@/api';
import { Button } from '@/components/Button';
import { ErrorBox } from '@/components/ErrorBox';
import { Screen } from '@/components/Screen';
import { colors, space } from '@/components/theme';
import { usePrinter } from '@/hooks/usePrinter';
import { useI18n } from '@/i18n';
import { formatVnd } from '@/lib/price';

export function PrinterScreen({ printerId }: { printerId: string }) {
  const { t } = useI18n();
  const router = useRouter();
  const { state, reload } = usePrinter(printerId);

  if (state.phase === 'loading') {
    return (
      <Screen testID="printer-screen">
        <ActivityIndicator
          testID="printer-loading"
          accessibilityLabel={t('printer.loading')}
        />
      </Screen>
    );
  }

  if (state.phase === 'error') {
    const notFound =
      state.error instanceof ApiError && state.error.is('not_found');
    return (
      <Screen testID="printer-screen">
        {notFound ? (
          <Text
            testID="printer-not-found"
            accessibilityRole="alert"
            style={styles.error}
          >
            {t('printer.notFound')}
          </Text>
        ) : (
          <ErrorBox
            error={state.error}
            testID="printer-error"
            onRetry={reload}
            retryTestID="printer-retry"
          />
        )}
        <Button
          variant="secondary"
          label={t('order.home')}
          testID="printer-home"
          onPress={() => router.replace('/')}
        />
      </Screen>
    );
  }

  const { printer } = state;
  // A busy printer is still printing: jobs queue on the Pi, so orders are accepted.
  const bookable = printer.status === 'online' || printer.status === 'busy';
  return (
    <Screen testID="printer-screen">
      <Text
        accessibilityRole="header"
        testID="printer-name"
        style={styles.title}
      >
        {printer.name}
      </Text>
      <View style={styles.card}>
        <Text style={styles.muted}>{t('printer.price')}</Text>
        <Text testID="printer-price" style={styles.price}>
          {formatVnd(printer.pricePerPrintVnd)}
        </Text>
        <Text style={styles.muted}>
          {t('printer.media', { media: printer.media.join(', ') })}
        </Text>
      </View>
      <Text
        testID="printer-status"
        accessibilityRole={bookable ? 'text' : 'alert'}
        style={bookable ? styles.ok : styles.error}
      >
        {t(`printer.status.${printer.status}`)}
      </Text>
      {bookable ? (
        <Button
          label={t('printer.pick')}
          testID="printer-pick-button"
          onPress={() =>
            router.push({
              pathname: '/p/[printerId]/photos',
              params: { printerId: printer.printerId },
            })
          }
        />
      ) : (
        <Button
          variant="secondary"
          label={t('printer.refresh')}
          testID="printer-retry"
          onPress={reload}
        />
      )}
    </Screen>
  );
}

const styles = StyleSheet.create({
  title: { fontSize: 26, fontWeight: '700', color: colors.text },
  card: {
    gap: space.xs,
    padding: space.md,
    borderRadius: 10,
    backgroundColor: colors.surface,
  },
  muted: { color: colors.muted, fontSize: 15 },
  price: { fontSize: 24, fontWeight: '700', color: colors.text },
  ok: { color: colors.success, fontSize: 16 },
  error: { color: colors.danger, fontSize: 16 },
});
