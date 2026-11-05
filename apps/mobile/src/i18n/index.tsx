import {
  createContext,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import { getLocales } from 'expo-localization';

import { en } from './en';
import { vi, type MessageKey } from './vi';

export type Locale = 'vi' | 'en';
export type { MessageKey };

const dictionaries: Record<Locale, Record<MessageKey, string>> = { vi, en };

/** Vietnamese is the default; English only when the device language is English. */
export function detectLocale(languageCode: string | null | undefined): Locale {
  return languageCode === 'en' ? 'en' : 'vi';
}

export function translate(
  locale: Locale,
  key: MessageKey,
  vars?: Record<string, string | number>,
): string {
  const text = dictionaries[locale][key] ?? vi[key];
  if (!vars) return text;
  return text.replace(/\{(\w+)\}/g, (m, name: string) =>
    name in vars ? String(vars[name]) : m,
  );
}

interface I18nValue {
  locale: Locale;
  setLocale: (l: Locale) => void;
  t: (key: MessageKey, vars?: Record<string, string | number>) => string;
}

const I18nContext = createContext<I18nValue>({
  locale: 'vi',
  setLocale: () => {},
  t: (key, vars) => translate('vi', key, vars),
});

export function I18nProvider({
  children,
  initialLocale,
}: {
  children: ReactNode;
  initialLocale?: Locale;
}) {
  const [locale, setLocale] = useState<Locale>(
    () => initialLocale ?? detectLocale(getLocales()[0]?.languageCode),
  );
  const value = useMemo<I18nValue>(
    () => ({
      locale,
      setLocale,
      t: (key, vars) => translate(locale, key, vars),
    }),
    [locale],
  );
  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n(): I18nValue {
  return useContext(I18nContext);
}
