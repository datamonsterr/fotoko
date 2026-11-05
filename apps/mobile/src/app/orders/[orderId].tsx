import { useLocalSearchParams } from 'expo-router';

import { OrderScreen } from '@/screens/OrderScreen';

export default function OrderRoute() {
  const { orderId } = useLocalSearchParams<{ orderId: string }>();
  // key: a new order id gets fresh polling state, never the previous order's.
  return <OrderScreen key={orderId} orderId={orderId} />;
}
