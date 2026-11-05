import { useLocalSearchParams } from 'expo-router';

import { PhotosScreen } from '@/screens/PhotosScreen';

export default function PhotosRoute() {
  const { printerId } = useLocalSearchParams<{ printerId: string }>();
  return <PhotosScreen printerId={printerId} />;
}
