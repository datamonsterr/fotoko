import { Text } from 'react-native';
import { act, render, screen } from '@testing-library/react-native';
import * as Localization from 'expo-localization';

import { en } from './en';
import { detectLocale, I18nProvider, translate, useI18n } from './index';
import { vi } from './vi';

jest.mock('expo-localization', () => ({
  getLocales: jest.fn(() => [{ languageCode: 'vi' }]),
}));

describe('dictionaries', () => {
  it('en has exactly the keys of vi', async () => {
    expect(Object.keys(en).sort()).toEqual(Object.keys(vi).sort());
  });
  it('no empty strings, and placeholders match', async () => {
    for (const key of Object.keys(vi) as (keyof typeof vi)[]) {
      expect(en[key]).not.toBe('');
      const names = (s: string) => (s.match(/\{\w+\}/g) ?? []).sort();
      expect(names(en[key])).toEqual(names(vi[key]));
    }
  });
});

describe('translate / detectLocale', () => {
  it('interpolates and leaves unknown placeholders', async () => {
    expect(translate('en', 'picker.empty', { max: 10 })).toBe(
      'No photos yet. Pick up to 10.',
    );
    expect(translate('vi', 'picker.empty', {})).toContain('{max}');
    expect(translate('vi', 'home.scan')).toBe('Quét mã QR');
  });
  it('Vietnamese default, English only for en', async () => {
    expect(detectLocale('en')).toBe('en');
    expect(detectLocale('vi')).toBe('vi');
    expect(detectLocale('fr')).toBe('vi');
    expect(detectLocale(null)).toBe('vi');
    expect(detectLocale(undefined)).toBe('vi');
  });
});

function Probe() {
  const { t, locale, setLocale } = useI18n();
  return (
    <Text testID="p" onPress={() => setLocale('en')}>
      {locale}:{t('home.scan')}
    </Text>
  );
}

describe('I18nProvider', () => {
  it('uses the device language and can switch', async () => {
    await render(
      <I18nProvider>
        <Probe />
      </I18nProvider>,
    );
    expect(screen.getByTestId('p')).toHaveTextContent('vi:Quét mã QR');
    await act(async () => screen.getByTestId('p').props.onPress());
    expect(screen.getByTestId('p')).toHaveTextContent('en:Scan QR code');
  });

  it('device English and explicit initial locale', async () => {
    (Localization.getLocales as jest.Mock).mockReturnValueOnce([
      { languageCode: 'en' },
    ]);
    const { unmount } = await render(
      <I18nProvider>
        <Probe />
      </I18nProvider>,
    );
    expect(screen.getByTestId('p')).toHaveTextContent(/^en:/);
    await unmount();
    (Localization.getLocales as jest.Mock).mockReturnValueOnce([]);
    await render(
      <I18nProvider>
        <Probe />
      </I18nProvider>,
    );
    expect(screen.getByTestId('p')).toHaveTextContent(/^vi:/);
  });

  it('initialLocale wins', async () => {
    await render(
      <I18nProvider initialLocale="en">
        <Probe />
      </I18nProvider>,
    );
    expect(screen.getByTestId('p')).toHaveTextContent(/^en:/);
  });

  it('works without a provider (Vietnamese)', async () => {
    await render(<Probe />);
    expect(screen.getByTestId('p')).toHaveTextContent('vi:Quét mã QR');
    await act(async () => screen.getByTestId('p').props.onPress());
    expect(screen.getByTestId('p')).toHaveTextContent('vi:Quét mã QR');
  });
});
