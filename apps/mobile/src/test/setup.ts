/* eslint-disable @typescript-eslint/no-require-imports -- jest.mock factories load lazily */

// Native modules are absent under Jest; give each the narrow surface the app uses.
jest.mock('expo-secure-store', () => {
  const values = new Map<string, string>();
  return {
    __values: values,
    getItemAsync: jest.fn(async (k: string) => values.get(k) ?? null),
    setItemAsync: jest.fn(async (k: string, v: string) => {
      values.set(k, v);
    }),
  };
});

jest.mock('expo-web-browser', () => ({
  openBrowserAsync: jest.fn().mockResolvedValue({ type: 'dismiss' }),
}));

jest.mock('expo-image-picker', () => ({
  launchImageLibraryAsync: jest.fn(),
}));

jest.mock('expo-crypto', () => {
  let n = 0;
  return { randomUUID: jest.fn(() => `uuid-${++n}`) };
});

jest.mock('expo-camera', () => {
  const React = require('react');
  const { View } = require('react-native');
  const state = { granted: true, request: jest.fn() };
  return {
    __state: state,
    CameraView: (props: Record<string, unknown>) =>
      React.createElement(View, { ...props, testID: props.testID }),
    useCameraPermissions: () => [{ granted: state.granted }, state.request],
  };
});

jest.mock('expo-router', () => {
  const router = { push: jest.fn(), replace: jest.fn(), back: jest.fn() };
  const stackScreen = jest.fn(() => null);
  const Stack = Object.assign(
    ({ children }: { children?: unknown }) => children ?? null,
    { Screen: stackScreen },
  );
  return {
    __router: router,
    __stackScreen: stackScreen,
    useRouter: () => router,
    useLocalSearchParams: jest.fn(),
    Stack,
  };
});

// jest-expo's AppState mock returns no subscription; give it one.
jest.requireActual('react-native').AppState.addEventListener = jest.fn(() => ({
  remove: jest.fn(),
}));
