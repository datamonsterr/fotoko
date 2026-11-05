import { Image, StyleSheet, Text, View } from 'react-native';

import { useI18n } from '@/i18n';
import { MAX_COPIES, MIN_COPIES } from '@/lib/limits';
import type { DraftPhoto } from '@/lib/photos';

import { Button } from './Button';
import { colors, space } from './theme';

export function PhotoRow({
  photo,
  index,
  onCopies,
  onRemove,
}: {
  photo: DraftPhoto;
  index: number;
  onCopies: (copies: number) => void;
  onRemove: () => void;
}) {
  const { t } = useI18n();
  const n = index + 1;
  return (
    <View style={styles.row} testID={`picker-photo-${index}`}>
      <Image
        source={{ uri: photo.uri }}
        style={styles.thumb}
        accessibilityLabel={t('picker.photo', { n })}
      />
      <View style={styles.body}>
        <Text
          testID={`picker-copies-${index}`}
          accessibilityRole="text"
          style={styles.copies}
        >
          {t('picker.copies', { copies: photo.copies })}
        </Text>
        <View style={styles.stepper}>
          <Button
            variant="secondary"
            label="-"
            accessibilityLabel={t('picker.less')}
            testID={`picker-copies-dec-${index}`}
            disabled={photo.copies <= MIN_COPIES}
            onPress={() => onCopies(photo.copies - 1)}
          />
          <Button
            variant="secondary"
            label="+"
            accessibilityLabel={t('picker.more')}
            testID={`picker-copies-inc-${index}`}
            disabled={photo.copies >= MAX_COPIES}
            onPress={() => onCopies(photo.copies + 1)}
          />
        </View>
      </View>
      <Button
        variant="danger"
        label="✕"
        accessibilityLabel={t('picker.remove', { n })}
        testID={`picker-remove-${index}`}
        onPress={onRemove}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: space.sm,
    padding: space.sm,
    backgroundColor: colors.surface,
    borderRadius: 10,
  },
  thumb: {
    width: 72,
    height: 72,
    borderRadius: 8,
    backgroundColor: colors.border,
  },
  body: { flex: 1, gap: space.xs },
  copies: { fontSize: 15, color: colors.text },
  stepper: { flexDirection: 'row', gap: space.sm },
});
