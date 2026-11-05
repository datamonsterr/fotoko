import { Platform } from 'react-native';
import * as SecureStore from 'expo-secure-store';

import type { KeyValueStore } from './types';

const memory = new Map<string, string>();

/**
 * Persistent store: SecureStore on native, localStorage on web. Falls back to memory when the
 * backing store throws (private windows, blocked storage), so the app still works for the session.
 */
export const persistentStore: KeyValueStore = {
  async get(key) {
    try {
      if (Platform.OS === 'web') return globalThis.localStorage.getItem(key);
      return await SecureStore.getItemAsync(key);
    } catch {
      return memory.get(key) ?? null;
    }
  },
  async set(key, value) {
    memory.set(key, value);
    try {
      if (Platform.OS === 'web') globalThis.localStorage.setItem(key, value);
      else await SecureStore.setItemAsync(key, value);
    } catch {
      // memory copy already saved
    }
  },
};
