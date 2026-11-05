import { detectContentType, readBlobSize, toDraftPhotos } from './photos';

describe('detectContentType', () => {
  it.each([
    [{ uri: 'x', mimeType: 'image/jpeg' }, 'image/jpeg'],
    [{ uri: 'x', mimeType: 'IMAGE/JPG' }, 'image/jpeg'],
    [{ uri: 'x', mimeType: 'image/png' }, 'image/png'],
    [{ uri: 'x', mimeType: 'image/heic' }, null],
    [{ uri: 'file:///a/b.JPEG?x=1' }, 'image/jpeg'],
    [{ uri: 'file:///a/b.jpg' }, 'image/jpeg'],
    [{ uri: 'file:///a/b', fileName: 'c.png' }, 'image/png'],
    [{ uri: 'file:///a/b.gif' }, null],
    [{ uri: 'blob:abc' }, null],
  ])('%j -> %p', (asset, expected) => {
    expect(detectContentType(asset)).toBe(expected);
  });
});

describe('toDraftPhotos', () => {
  let n = 0;
  const sizes: Record<string, number> = {};
  const base = {
    existingCount: 0,
    newId: () => `id-${++n}`,
    readSize: jest.fn(async (uri: string) => sizes[uri] ?? 4096),
  };

  it('converts supported assets with 1 copy, sized from the real bytes', async () => {
    sizes['a.jpg'] = 1234;
    const r = await toDraftPhotos(
      // the picker's fileSize is ignored: it can differ from what is uploaded
      [{ uri: 'a.jpg', mimeType: 'image/jpeg', fileSize: 999_999 }],
      base,
    );
    expect(base.readSize).toHaveBeenCalledWith('a.jpg');
    expect(r.photos).toEqual([
      {
        id: expect.any(String),
        uri: 'a.jpg',
        contentType: 'image/jpeg',
        sizeBytes: 1234,
        copies: 1,
      },
    ]);
  });

  it('counts unsupported, too big, empty and over-limit assets', async () => {
    Object.assign(sizes, {
      'a.heic': 10,
      'big.jpg': 15 * 1024 * 1024 + 1,
      'empty.jpg': 0,
      'ok.jpg': 15 * 1024 * 1024,
      'late.jpg': 1,
    });
    const assets = [
      { uri: 'a.heic', mimeType: 'image/heic' },
      { uri: 'big.jpg', mimeType: 'image/jpeg' },
      { uri: 'empty.jpg', mimeType: 'image/jpeg' },
      { uri: 'ok.jpg', mimeType: 'image/jpeg' },
      { uri: 'late.jpg', mimeType: 'image/jpeg' },
    ];
    const r = await toDraftPhotos(assets, { ...base, existingCount: 9 });
    expect(r).toMatchObject({ tooBig: 1, unsupported: 2, overLimit: 1 });
    expect(r.photos.map((p) => p.uri)).toEqual(['ok.jpg']);
  });

  it('a read failure rejects (the picker hook turns it into a notice)', async () => {
    await expect(
      toDraftPhotos([{ uri: 'x.jpg', mimeType: 'image/jpeg' }], {
        ...base,
        readSize: async () => {
          throw new Error('unreadable');
        },
      }),
    ).rejects.toThrow('unreadable');
  });
});

describe('readBlobSize', () => {
  it('measures the fetched blob', async () => {
    const spy = jest.spyOn(globalThis, 'fetch').mockResolvedValue({
      blob: async () => ({ size: 77 }),
    } as unknown as Response);
    await expect(readBlobSize('file:///a')).resolves.toBe(77);
    spy.mockRestore();
  });
});
