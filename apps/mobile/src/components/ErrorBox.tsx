import { StyleSheet, Text, View } from 'react-native';

import { useI18n } from '@/i18n';
import { errorMessageKey } from '@/lib/errorMessage';

import { Button } from './Button';
import { colors, space } from './theme';

export function ErrorBox({
  error,
  testID,
  onRetry,
  retryLabel,
  retryTestID,
}: {
  error: unknown;
  testID: string;
  onRetry?: () => void;
  retryLabel?: string;
  retryTestID?: string;
}) {
  const { t } = useI18n();
  return (
    <View style={styles.box} testID={testID} accessibilityRole="alert">
      <Text style={styles.text}>{t(errorMessageKey(error))}</Text>
      {onRetry ? (
        <Button
          variant="secondary"
          label={retryLabel ?? t('common.retry')}
          onPress={onRetry}
          testID={retryTestID ?? `${testID}-retry`}
        />
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  box: {
    gap: space.sm,
    padding: space.md,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: colors.danger,
  },
  text: { color: colors.danger, fontSize: 15 },
});
