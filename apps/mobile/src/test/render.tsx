import type { ReactElement } from 'react';
import { render } from '@testing-library/react-native';

import { I18nProvider, type Locale } from '@/i18n';
import type { DraftPhoto } from '@/lib/photos';
import { PhotoDraftProvider } from '@/state/draft';

export function renderApp(
  ui: ReactElement,
  opts: { locale?: Locale; photos?: DraftPhoto[] } = {},
) {
  return render(
    <I18nProvider initialLocale={opts.locale ?? 'vi'}>
      <PhotoDraftProvider initial={opts.photos}>{ui}</PhotoDraftProvider>
    </I18nProvider>,
  );
}

/** The mocked expo-router (see setup.ts): push/replace/back are jest.fn. */
export const router = (
  jest.requireMock('expo-router') as {
    __router: Record<'push' | 'replace' | 'back', jest.Mock>;
  }
).__router;

export const draftPhoto = (i: number, copies = 1): DraftPhoto => ({
  id: `d${i}`,
  uri: `file:///${i}.jpg`,
  contentType: 'image/jpeg',
  sizeBytes: 1000,
  copies,
});
