import { ActivityIndicator, Pressable, StyleSheet, Text } from 'react-native';

import { colors, space } from './theme';

export interface ButtonProps {
  label: string;
  onPress: () => void;
  testID: string;
  variant?: 'primary' | 'secondary' | 'danger';
  disabled?: boolean;
  loading?: boolean;
  /** Overrides the visible label for screen readers. */
  accessibilityLabel?: string;
}

export function Button({
  label,
  onPress,
  testID,
  variant = 'primary',
  disabled = false,
  loading = false,
  accessibilityLabel,
}: ButtonProps) {
  const inactive = disabled || loading;
  return (
    <Pressable
      testID={testID}
      accessibilityRole="button"
      accessibilityLabel={accessibilityLabel ?? label}
      accessibilityState={{ disabled: inactive, busy: loading }}
      disabled={inactive}
      onPress={onPress}
      style={[styles.base, styles[variant], inactive && styles.disabled]}
    >
      {loading ? (
        <ActivityIndicator
          color={variant === 'primary' ? colors.onPrimary : colors.text}
        />
      ) : (
        <Text
          style={[
            styles.label,
            variant === 'primary' && styles.labelPrimary,
            variant === 'danger' && styles.labelDanger,
          ]}
        >
          {label}
        </Text>
      )}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  base: {
    minHeight: 48,
    paddingHorizontal: space.md,
    borderRadius: 10,
    alignItems: 'center',
    justifyContent: 'center',
  },
  primary: { backgroundColor: colors.primary },
  secondary: {
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.border,
  },
  danger: {
    backgroundColor: colors.bg,
    borderWidth: 1,
    borderColor: colors.danger,
  },
  disabled: { opacity: 0.5 },
  label: { fontSize: 16, fontWeight: '600', color: colors.text },
  labelPrimary: { color: colors.onPrimary },
  labelDanger: { color: colors.danger },
});
