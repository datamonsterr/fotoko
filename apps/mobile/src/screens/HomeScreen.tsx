import { useState } from 'react';
import { StyleSheet, Text, TextInput, View } from 'react-native';
import { useRouter } from 'expo-router';

import { Button } from '@/components/Button';
import { Screen } from '@/components/Screen';
import { colors, space } from '@/components/theme';
import { useI18n } from '@/i18n';
import { isValidPrinterId } from '@/lib/qr';

export function HomeScreen() {
  const { t, locale, setLocale } = useI18n();
  const router = useRouter();
  const [id, setId] = useState('');
  const [invalid, setInvalid] = useState(false);

  const go = () => {
    if (!isValidPrinterId(id)) {
      setInvalid(true);
      return;
    }
    setInvalid(false);
    router.push({
      pathname: '/p/[printerId]',
      params: { printerId: id.trim() },
    });
  };

  return (
    <Screen testID="home-screen">
      <Text accessibilityRole="header" style={styles.title}>
        {t('home.title')}
      </Text>
      <Text style={styles.subtitle}>{t('home.subtitle')}</Text>
      <Button
        label={t('home.scan')}
        testID="home-scan-button"
        onPress={() => router.push('/scan')}
      />
      <View style={styles.manual}>
        <Text style={styles.label}>{t('home.manualLabel')}</Text>
        <TextInput
          testID="home-printer-input"
          accessibilityLabel={t('home.manualLabel')}
          placeholder={t('home.manualPlaceholder')}
          value={id}
          onChangeText={(v) => {
            setId(v);
            setInvalid(false);
          }}
          onSubmitEditing={go}
          autoCapitalize="none"
          autoCorrect={false}
          returnKeyType="go"
          style={styles.input}
        />
        {invalid ? (
          <Text
            testID="home-printer-error"
            accessibilityRole="alert"
            style={styles.error}
          >
            {t('home.invalidId')}
          </Text>
        ) : null}
        <Button
          variant="secondary"
          label={t('home.manualGo')}
          testID="home-printer-go"
          onPress={go}
        />
      </View>
      <Button
        variant="secondary"
        label={t('home.language')}
        testID="home-language-toggle"
        onPress={() => setLocale(locale === 'vi' ? 'en' : 'vi')}
      />
    </Screen>
  );
}

const styles = StyleSheet.create({
  title: { fontSize: 28, fontWeight: '700', color: colors.text },
  subtitle: { fontSize: 16, color: colors.muted },
  manual: { gap: space.sm },
  label: { fontSize: 15, color: colors.text },
  input: {
    minHeight: 48,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: 10,
    paddingHorizontal: space.md,
    fontSize: 16,
    color: colors.text,
  },
  error: { color: colors.danger },
});
