import { StyleSheet, Text, View } from 'react-native';

import type { OrderStatus } from '@/api/types';
import { useI18n } from '@/i18n';
import { timeline } from '@/lib/status';

import { colors, space } from './theme';

export function StatusTimeline({ status }: { status: OrderStatus }) {
  const { t } = useI18n();
  return (
    <View testID="order-timeline" accessibilityRole="list" style={styles.list}>
      {timeline(status).map(({ step, state }) => {
        const label = t(`order.status.${step}`);
        const mark = state === 'done' ? '✓' : state === 'current' ? '●' : '○';
        return (
          <View
            key={step}
            testID={`order-step-${step}`}
            accessible
            accessibilityRole="text"
            accessibilityLabel={label}
            accessibilityState={{
              checked: state === 'done',
              selected: state === 'current',
            }}
            style={styles.item}
          >
            <Text
              style={[styles.mark, state !== 'pending' && styles.markActive]}
            >
              {mark}
            </Text>
            <Text
              style={[
                styles.label,
                state === 'current' && styles.labelCurrent,
                state === 'pending' && styles.labelPending,
              ]}
            >
              {label}
            </Text>
          </View>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create({
  list: { gap: space.sm },
  item: { flexDirection: 'row', alignItems: 'center', gap: space.sm },
  mark: { width: 24, fontSize: 18, color: colors.muted },
  markActive: { color: colors.primary },
  label: { fontSize: 16, color: colors.text },
  labelCurrent: { fontWeight: '700' },
  labelPending: { color: colors.muted },
});
