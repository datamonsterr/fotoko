import { useRef, useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import { CameraView, useCameraPermissions } from 'expo-camera';
import { useRouter } from 'expo-router';

import { Button } from '@/components/Button';
import { Screen } from '@/components/Screen';
import { colors, space } from '@/components/theme';
import { useI18n } from '@/i18n';
import { parsePrinterLink } from '@/lib/qr';

export function ScanScreen() {
  const { t } = useI18n();
  const router = useRouter();
  const [permission, requestPermission] = useCameraPermissions();
  const [rejected, setRejected] = useState(false);
  // onBarcodeScanned fires many times per second; handle one code per attempt.
  const handled = useRef(false);

  if (!permission?.granted) {
    return (
      <Screen testID="scan-screen">
        <Text style={styles.text}>{t('scan.permissionNeeded')}</Text>
        <Button
          label={t('scan.grant')}
          testID="scan-permission-grant"
          onPress={() => void requestPermission()}
        />
        <Button
          variant="secondary"
          label={t('scan.manual')}
          testID="scan-manual"
          onPress={() => router.replace('/')}
        />
      </Screen>
    );
  }

  const onScanned = ({ data }: { data: string }) => {
    if (handled.current) return;
    handled.current = true;
    const printerId = parsePrinterLink(data);
    if (printerId) {
      router.replace({ pathname: '/p/[printerId]', params: { printerId } });
    } else {
      setRejected(true);
    }
  };

  return (
    <Screen testID="scan-screen">
      <Text style={styles.text}>{t('scan.hint')}</Text>
      <View style={styles.camera}>
        <CameraView
          testID="scan-camera"
          style={StyleSheet.absoluteFill}
          facing="back"
          barcodeScannerSettings={{ barcodeTypes: ['qr'] }}
          onBarcodeScanned={rejected ? undefined : onScanned}
          accessibilityLabel={t('scan.hint')}
        />
      </View>
      {rejected ? (
        <View style={styles.rejected}>
          <Text
            testID="scan-error"
            accessibilityRole="alert"
            style={styles.error}
          >
            {t('scan.invalid')}
          </Text>
          <Button
            label={t('scan.again')}
            testID="scan-again"
            onPress={() => {
              handled.current = false;
              setRejected(false);
            }}
          />
        </View>
      ) : null}
    </Screen>
  );
}

const styles = StyleSheet.create({
  text: { fontSize: 16, color: colors.text },
  camera: {
    aspectRatio: 1,
    width: '100%',
    borderRadius: 12,
    overflow: 'hidden',
    backgroundColor: '#000',
  },
  rejected: { gap: space.sm },
  error: { color: colors.danger, fontSize: 15 },
});
