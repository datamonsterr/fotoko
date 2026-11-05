import { Platform } from 'react-native';
import * as WebBrowser from 'expo-web-browser';

import { openCheckoutUrl } from './checkout';

describe('openCheckoutUrl', () => {
  const original = Platform.OS;
  afterEach(() => {
    Platform.OS = original;
  });

  it('opens the in-app browser on native', async () => {
    Platform.OS = 'ios';
    await openCheckoutUrl('https://pay.example/x');
    expect(WebBrowser.openBrowserAsync).toHaveBeenCalledWith(
      'https://pay.example/x',
    );
  });

  it('navigates the same tab on web', async () => {
    Platform.OS = 'web';
    const assign = jest.fn();
    const saved = Object.getOwnPropertyDescriptor(globalThis, 'location');
    Object.defineProperty(globalThis, 'location', {
      value: { assign },
      configurable: true,
    });
    await openCheckoutUrl('https://pay.example/y');
    expect(assign).toHaveBeenCalledWith('https://pay.example/y');
    if (saved) Object.defineProperty(globalThis, 'location', saved);
    else delete (globalThis as { location?: unknown }).location;
  });
});
