import { Text } from 'react-native';
import { act, render, renderHook } from '@testing-library/react-native';
import type { ReactNode } from 'react';

import type { DraftPhoto } from '@/lib/photos';

import { draftReducer, PhotoDraftProvider, usePhotoDraft } from './draft';

const photo = (id: string, copies = 1): DraftPhoto => ({
  id,
  uri: `${id}.jpg`,
  contentType: 'image/jpeg',
  sizeBytes: 10,
  copies,
});

describe('draftReducer', () => {
  it('adds, capped at 10 photos', async () => {
    const many = Array.from({ length: 12 }, (_, i) => photo(`p${i}`));
    expect(draftReducer([], { type: 'add', photos: many })).toHaveLength(10);
    expect(
      draftReducer([photo('a')], { type: 'add', photos: [photo('b')] }).map(
        (p) => p.id,
      ),
    ).toEqual(['a', 'b']);
  });

  it('removes by id', async () => {
    expect(
      draftReducer([photo('a'), photo('b')], { type: 'remove', id: 'a' }).map(
        (p) => p.id,
      ),
    ).toEqual(['b']);
  });

  it('sets copies clamped to 1..5, only for the target', async () => {
    const s = [photo('a'), photo('b')];
    expect(
      draftReducer(s, { type: 'setCopies', id: 'a', copies: 9 })[0]?.copies,
    ).toBe(5);
    expect(
      draftReducer(s, { type: 'setCopies', id: 'a', copies: 0 })[0]?.copies,
    ).toBe(1);
    expect(
      draftReducer(s, { type: 'setCopies', id: 'a', copies: 3 })[1]?.copies,
    ).toBe(1);
  });

  it('clears', async () => {
    expect(draftReducer([photo('a')], { type: 'clear' })).toEqual([]);
  });
});

describe('usePhotoDraft', () => {
  const wrapper = ({ children }: { children: ReactNode }) => (
    <PhotoDraftProvider>{children}</PhotoDraftProvider>
  );

  it('shares state through the provider', async () => {
    const { result } = await renderHook(() => usePhotoDraft(), { wrapper });
    await act(async () =>
      result.current.dispatch({ type: 'add', photos: [photo('a')] }),
    );
    expect(result.current.photos).toHaveLength(1);
  });

  it('throws without a provider', async () => {
    const spy = jest.spyOn(console, 'error').mockImplementation(() => {});
    function Bad() {
      usePhotoDraft();
      return <Text>x</Text>;
    }
    await expect(render(<Bad />)).rejects.toThrow(/PhotoDraftProvider/);
    spy.mockRestore();
  });
});
