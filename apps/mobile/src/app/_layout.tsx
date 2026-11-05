import { Stack } from 'expo-router';
import { StatusBar } from 'expo-status-bar';

import { I18nProvider, useI18n } from '@/i18n';
import { PhotoDraftProvider } from '@/state/draft';

function Navigator() {
  const { t } = useI18n();
  return (
    <Stack screenOptions={{ headerTitle: t('app.name') }}>
      <Stack.Screen name="index" options={{ headerShown: false }} />
      <Stack.Screen name="scan" options={{ title: t('scan.title') }} />
      <Stack.Screen
        name="p/[printerId]/photos"
        options={{ title: t('picker.title') }}
      />
      <Stack.Screen
        name="p/[printerId]/upload"
        options={{ title: t('upload.title') }}
      />
      <Stack.Screen
        name="orders/[orderId]"
        options={{ title: t('order.title'), headerBackVisible: false }}
      />
    </Stack>
  );
}

export default function RootLayout() {
  return (
    <I18nProvider>
      <PhotoDraftProvider>
        <StatusBar style="dark" />
        <Navigator />
      </PhotoDraftProvider>
    </I18nProvider>
  );
}
