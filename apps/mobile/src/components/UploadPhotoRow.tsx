import { StyleSheet, Text, View } from 'react-native';

import type { PhotoUploadState } from '@/hooks/useCheckoutFlow';
import { useI18n } from '@/i18n';

import { Button } from './Button';
import { colors, space } from './theme';

export function UploadPhotoRow({
  photo,
  index,
  retryDisabled,
  onRetry,
}: {
  photo: PhotoUploadState;
  index: number;
  /** True while any upload or checkout step is running: a retry then would race it. */
  retryDisabled: boolean;
  onRetry: () => void;
}) {
  const { t } = useI18n();
  const n = index + 1;
  const percent = Math.round(photo.progress * 100);
  const label =
    photo.status === 'pending'
      ? t('upload.pending')
      : photo.status === 'uploading'
        ? t('upload.uploading', { percent })
        : photo.status === 'done'
          ? t('upload.done')
          : t('upload.failed');
  return (
    <View testID={`upload-photo-${index}`} style={styles.row}>
      <Text style={styles.name}>{t('upload.photo', { n })}</Text>
      <Text
        testID={`upload-photo-status-${index}`}
        accessibilityRole="progressbar"
        accessibilityValue={{ min: 0, max: 100, now: percent }}
        style={photo.status === 'error' ? styles.error : styles.status}
      >
        {label}
      </Text>
      {photo.status === 'error' ? (
        <Button
          variant="secondary"
          label={t('upload.retry')}
          accessibilityLabel={`${t('upload.retry')} ${t('upload.photo', { n })}`}
          testID={`upload-photo-retry-${index}`}
          disabled={retryDisabled}
          onPress={onRetry}
        />
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
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
