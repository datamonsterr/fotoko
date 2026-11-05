import { Platform } from 'react-native';
import * as SecureStore from 'expo-secure-store';

import { persistentStore } from './storage';

describe('persistentStore', () => {
  const original = Platform.OS;
  afterEach(() => {
    Platform.OS = original;
    jest.restoreAllMocks();
  });

  it('uses SecureStore on native', async () => {
    Platform.OS = 'android';
    await persistentStore.set('k1', 'v');
    expect(SecureStore.setItemAsync).toHaveBeenCalledWith('k1', 'v');
    await expect(persistentStore.get('k1')).resolves.toBe('v');
    await expect(persistentStore.get('missing')).resolves.toBeNull();
  });

  it('falls back to memory when SecureStore throws', async () => {
    Platform.OS = 'ios';
    (SecureStore.setItemAsync as jest.Mock).mockRejectedValueOnce(
      new Error('locked'),
    );
    (SecureStore.getItemAsync as jest.Mock).mockRejectedValueOnce(
      new Error('locked'),
    );
    await persistentStore.set('k2', 'mem');
    await expect(persistentStore.get('k2')).resolves.toBe('mem');
    (SecureStore.getItemAsync as jest.Mock).mockRejectedValueOnce(
      new Error('locked'),
    );
    await expect(persistentStore.get('absent')).resolves.toBeNull();
  });

  it('uses localStorage on web and degrades to memory when it throws', async () => {
    Platform.OS = 'web';
    const data = new Map<string, string>();
    const ls = {
      getItem: (k: string) => data.get(k) ?? null,
      setItem: (k: string, v: string) => void data.set(k, v),
    };
    Object.defineProperty(globalThis, 'localStorage', {
      value: ls,
      configurable: true,
    });
    await persistentStore.set('w', '1');
    expect(data.get('w')).toBe('1');
    await expect(persistentStore.get('w')).resolves.toBe('1');

    ls.setItem = () => {
      throw new Error('blocked');
    };
    ls.getItem = () => {
      throw new Error('blocked');
    };
    await persistentStore.set('w2', '2');
    await expect(persistentStore.get('w2')).resolves.toBe('2');
    delete (globalThis as { localStorage?: unknown }).localStorage;
  });
});
