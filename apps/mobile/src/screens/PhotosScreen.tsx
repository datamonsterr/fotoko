import { ActivityIndicator, StyleSheet, Text, View } from 'react-native';
import { useRouter } from 'expo-router';

import { Button } from '@/components/Button';
import { ErrorBox } from '@/components/ErrorBox';
import { PhotoRow } from '@/components/PhotoRow';
import { Screen } from '@/components/Screen';
import { colors, space } from '@/components/theme';
import { usePhotoPicker } from '@/hooks/usePhotoPicker';
import { usePrinter } from '@/hooks/usePrinter';
import { useI18n } from '@/i18n';
import { MAX_PHOTOS } from '@/lib/limits';
import { formatVnd, totalCopies, totalPriceVnd } from '@/lib/price';
import { usePhotoDraft } from '@/state/draft';

export function PhotosScreen({ printerId }: { printerId: string }) {
  const { t } = useI18n();
  const router = useRouter();
  const { photos, dispatch } = usePhotoDraft();
  const { state, reload } = usePrinter(printerId);
  const { pick, picking, notices } = usePhotoPicker();

  if (state.phase === 'loading') {
    return (
      <Screen testID="picker-screen">
        <ActivityIndicator
          testID="picker-loading"
          accessibilityLabel={t('printer.loading')}
        />
      </Screen>
    );
  }
  if (state.phase === 'error') {
    return (
      <Screen testID="picker-screen">
        <ErrorBox error={state.error} testID="picker-error" onRetry={reload} />
      </Screen>
    );
  }

  const price = state.printer.pricePerPrintVnd;
  const total = totalPriceVnd(photos, price);
  const count = totalCopies(photos);
  return (
    <Screen testID="picker-screen">
      <Button
        variant="secondary"
        label={t('picker.add')}
        testID="picker-add"
        disabled={photos.length >= MAX_PHOTOS || picking}
        loading={picking}
        onPress={() => void pick()}
      />
      {notices.map((n, i) => (
        <Text
          key={n}
          testID={`picker-notice-${i}`}
          accessibilityRole="alert"
          style={styles.notice}
        >
          {n}
        </Text>
      ))}
      {photos.length === 0 ? (
        <Text testID="picker-empty" style={styles.muted}>
          {t('picker.empty', { max: MAX_PHOTOS })}
        </Text>
      ) : (
        photos.map((p, i) => (
          <PhotoRow
            key={p.id}
            photo={p}
            index={i}
            onCopies={(copies) =>
              dispatch({ type: 'setCopies', id: p.id, copies })
            }
            onRemove={() => dispatch({ type: 'remove', id: p.id })}
          />
        ))
      )}
      <View style={styles.total} accessibilityRole="summary">
        <Text style={styles.muted}>
          {t('picker.total')} ({t('picker.totalCopies', { count })})
        </Text>
        <Text testID="picker-total" style={styles.price}>
          {formatVnd(total)}
        </Text>
      </View>
      <Button
        label={t('picker.continue')}
        testID="picker-continue"
        disabled={photos.length === 0}
        onPress={() =>
          router.push({
            pathname: '/p/[printerId]/upload',
            params: { printerId },
          })
        }
      />
    </Screen>
  );
}

const styles = StyleSheet.create({
  muted: { color: colors.muted, fontSize: 15 },
  notice: { color: colors.danger, fontSize: 14 },
  total: {
    gap: space.xs,
    padding: space.md,
    backgroundColor: colors.surface,
    borderRadius: 10,
  },
  price: { fontSize: 24, fontWeight: '700', color: colors.text },
});
