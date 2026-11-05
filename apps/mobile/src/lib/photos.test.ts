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
  const base = {
    existingCount: 0,
    newId: () => `id-${++n}`,
    readSize: jest.fn().mockResolvedValue(4096),
  };

  it('converts supported assets with 1 copy', async () => {
    const r = await toDraftPhotos(
      [{ uri: 'a.jpg', mimeType: 'image/jpeg', fileSize: 1000 }],
      base,
    );
    expect(r.photos).toEqual([
      {
        id: expect.any(String),
        uri: 'a.jpg',
        contentType: 'image/jpeg',
        sizeBytes: 1000,
        copies: 1,
      },
    ]);
  });

  it('reads the size when the picker gives none', async () => {
    const r = await toDraftPhotos(
      [{ uri: 'a.png', mimeType: 'image/png' }],
      base,
    );
    expect(base.readSize).toHaveBeenCalledWith('a.png');
    expect(r.photos[0]?.sizeBytes).toBe(4096);
  });

  it('counts unsupported, too big, empty and over-limit assets', async () => {
    const assets = [
      { uri: 'a.heic', mimeType: 'image/heic', fileSize: 10 },
      {
        uri: 'big.jpg',
        mimeType: 'image/jpeg',
        fileSize: 15 * 1024 * 1024 + 1,
      },
      { uri: 'empty.jpg', mimeType: 'image/jpeg', fileSize: 0 },
      { uri: 'ok.jpg', mimeType: 'image/jpeg', fileSize: 15 * 1024 * 1024 },
      { uri: 'late.jpg', mimeType: 'image/jpeg', fileSize: 1 },
    ];
    const r = await toDraftPhotos(assets, { ...base, existingCount: 9 });
    expect(r).toMatchObject({ tooBig: 1, unsupported: 2, overLimit: 1 });
    expect(r.photos.map((p) => p.uri)).toEqual(['ok.jpg']);
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
