import { useLocalSearchParams } from 'expo-router';

import { PrinterScreen } from '@/screens/PrinterScreen';

export default function PrinterRoute() {
  const { printerId } = useLocalSearchParams<{ printerId: string }>();
  return <PrinterScreen printerId={printerId} />;
}
