import { putPhoto, UPLOAD_TIMEOUT_MS } from './upload';

class FakeXhr {
  status = 200;
  headers: Record<string, string> = {};
  upload: { onprogress: ((e: unknown) => void) | null } = { onprogress: null };
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  ontimeout: (() => void) | null = null;
  onabort: (() => void) | null = null;
  timeout = 0;
  aborted = false;
  abort() {
    this.aborted = true;
    this.onabort?.();
  }
  opened: [string, string] | null = null;
  sent: unknown;
  hang = false;
  outcome: 'load' | 'error' | 'timeout' = 'load';
  progress: { loaded: number; total: number; lengthComputable: boolean }[] = [];
  open(m: string, u: string) {
    this.opened = [m, u];
  }
  setRequestHeader(k: string, v: string) {
    this.headers[k] = v;
  }
  send(body: unknown) {
    this.sent = body;
    if (this.hang) return;
    this.progress.forEach((p) => this.upload.onprogress?.(p));
    if (this.outcome === 'load') this.onload?.();
    else if (this.outcome === 'error') this.onerror?.();
    else this.ontimeout?.();
  }
}

const blob = { size: 3 } as Blob;
const args = (extra = {}) => ({
  url: 'https://s3.example/put',
  headers: { 'x-amz-meta-a': '1' },
  uri: 'file:///a.jpg',
  contentType: 'image/jpeg',
  ...extra,
});

function run(xhr: FakeXhr, extra = {}, readBody = async () => blob) {
  return putPhoto(args(extra), {
    readBody,
    createXhr: () => xhr as unknown as XMLHttpRequest,
  });
}

describe('putPhoto', () => {
  it('PUTs the blob with presigned headers, no auth, adding content-type when absent', async () => {
    const xhr = new FakeXhr();
    await run(xhr);
    expect(xhr.opened).toEqual(['PUT', 'https://s3.example/put']);
    expect(xhr.headers).toEqual({
      'x-amz-meta-a': '1',
      'content-type': 'image/jpeg',
    });
    expect(xhr.sent).toBe(blob);
  });

  it('keeps a presigned content-type of any case', async () => {
    const xhr = new FakeXhr();
    await run(xhr, { headers: { 'Content-Type': 'image/png' } });
    expect(xhr.headers).toEqual({ 'Content-Type': 'image/png' });
  });

  it('reports progress fractions and completes at 1', async () => {
    const xhr = new FakeXhr();
    xhr.progress = [
      { loaded: 1, total: 4, lengthComputable: true },
      { loaded: 2, total: 0, lengthComputable: true },
      { loaded: 3, total: 4, lengthComputable: false },
    ];
    const seen: number[] = [];
    await run(xhr, { onProgress: (f: number) => seen.push(f) });
    expect(seen).toEqual([0.25, 1]);
  });

  it('works without a progress callback', async () => {
    const xhr = new FakeXhr();
    xhr.progress = [{ loaded: 1, total: 2, lengthComputable: true }];
    await expect(run(xhr)).resolves.toBeUndefined();
  });

  it('rejects upload_failed on a non-2xx status', async () => {
    const xhr = new FakeXhr();
    xhr.status = 403;
    await expect(run(xhr)).rejects.toMatchObject({
      code: 'upload_failed',
      status: 403,
    });
  });

  it.each(['error', 'timeout'] as const)(
    'rejects network on %s',
    async (outcome) => {
      const xhr = new FakeXhr();
      xhr.outcome = outcome;
      await expect(run(xhr)).rejects.toMatchObject({ code: 'network' });
    },
  );

  it('rejects network when the photo cannot be read', async () => {
    await expect(
      run(new FakeXhr(), {}, async () => {
        throw new Error('gone');
      }),
    ).rejects.toMatchObject({ code: 'network', message: 'gone' });
    await expect(
      run(new FakeXhr(), {}, async () => {
        throw 'x';
      }),
    ).rejects.toMatchObject({ code: 'network', message: 'cannot read photo' });
  });

  it('defaults: reads via fetch and uses global XMLHttpRequest', async () => {
    const fetchSpy = jest
      .spyOn(globalThis, 'fetch')
      .mockResolvedValue({ blob: async () => blob } as unknown as Response);
    const xhr = new FakeXhr();
    const Original = globalThis.XMLHttpRequest;
    globalThis.XMLHttpRequest = function () {
      return xhr;
    } as unknown as typeof XMLHttpRequest;
    await putPhoto(args());
    expect(fetchSpy).toHaveBeenCalledWith('file:///a.jpg');
    expect(xhr.sent).toBe(blob);
    globalThis.XMLHttpRequest = Original;
    fetchSpy.mockRestore();
  });

  it('sets a 90 s timeout', async () => {
    const xhr = new FakeXhr();
    await run(xhr);
    expect(xhr.timeout).toBe(90_000);
    expect(UPLOAD_TIMEOUT_MS).toBe(90_000);
  });

  it('aborts the XHR when the signal fires and rejects aborted', async () => {
    const xhr = new FakeXhr();
    xhr.hang = true;
    const ctrl = new AbortController();
    const p = run(xhr, { signal: ctrl.signal });
    await new Promise((r) => setTimeout(r, 0));
    ctrl.abort();
    await expect(p).rejects.toMatchObject({ code: 'aborted' });
    expect(xhr.aborted).toBe(true);
  });

  it('rejects aborted without touching the network when already aborted', async () => {
    const xhr = new FakeXhr();
    const ctrl = new AbortController();
    ctrl.abort();
    await expect(run(xhr, { signal: ctrl.signal })).rejects.toMatchObject({
      code: 'aborted',
    });
    expect(xhr.opened).toBeNull();
  });

  it('stops listening to the signal once finished', async () => {
    const xhr = new FakeXhr();
    const ctrl = new AbortController();
    await run(xhr, { signal: ctrl.signal });
    ctrl.abort();
    expect(xhr.aborted).toBe(false);
  });
});
