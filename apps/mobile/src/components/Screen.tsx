import type { ReactNode } from 'react';
import { ScrollView, StyleSheet, View } from 'react-native';

import { colors, space } from './theme';

export function Screen({
  children,
  testID,
}: {
  children: ReactNode;
  testID: string;
}) {
  return (
    <ScrollView
      testID={testID}
      style={styles.scroll}
      contentContainerStyle={styles.content}
      keyboardShouldPersistTaps="handled"
    >
      <View style={styles.inner}>{children}</View>
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  scroll: { flex: 1, backgroundColor: colors.bg },
  content: { flexGrow: 1, padding: space.md, alignItems: 'center' },
  inner: { width: '100%', maxWidth: 480, gap: space.md },
});
