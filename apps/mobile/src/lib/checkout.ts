import { Platform } from 'react-native';
import * as WebBrowser from 'expo-web-browser';

/**
 * Opens the payOS page. Web: navigate the same tab (payOS redirects back to /orders/{id}).
 * Native: in-app browser; resolves when the user closes it, and the caller then shows the order.
 */
export async function openCheckoutUrl(url: string): Promise<void> {
  if (Platform.OS === 'web') {
    globalThis.location.assign(url);
    return;
  }
  await WebBrowser.openBrowserAsync(url);
}
