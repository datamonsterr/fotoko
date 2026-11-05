import { useLocalSearchParams } from 'expo-router';

import { UploadScreen } from '@/screens/UploadScreen';

export default function UploadRoute() {
  const { printerId } = useLocalSearchParams<{ printerId: string }>();
  return <UploadScreen printerId={printerId} />;
}
