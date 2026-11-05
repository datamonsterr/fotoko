import { ApiError, serverClock, setApiClient, type ApiClient } from '@/api';
import type { UploadArgs } from '@/api/upload';
import type { DraftPhoto } from '@/lib/photos';
import { createdOrder, farFuture } from '@/test/fixtures';

import { CheckoutFlow, EXPIRY_MARGIN_MS } from './checkoutFlow';

const draft = (n: number): DraftPhoto[] =>
  Array.from({ length: n }, (_, i) => ({
    id: `d${i}`,
    uri: `file:///${i}.jpg`,
    contentType: i % 2 ? 'image/png' : 'image/jpeg',
    sizeBytes: 100 + i,
    copies: i + 1,
  }));

const checkoutRes = {
  orderId: 'ord-1',
  status: 'awaiting_payment' as const,
  orderCode: 5,
  checkoutUrl: 'https://pay.example/c',
  qrCode: 'q',
};

function setup(
  n: number,
  overrides: {
    upload?: (a: UploadArgs) => Promise<void>;
    createOrder?: jest.Mock;
    checkout?: jest.Mock;
    onCheckout?: jest.Mock;
    now?: () => number;
  } = {},
) {
  const createOrder =
    overrides.createOrder ?? jest.fn().mockResolvedValue(createdOrder(n));
  const checkout =
    overrides.checkout ?? jest.fn().mockResolvedValue(checkoutRes);
  const upload = jest.fn(overrides.upload ?? (async () => {}));
  const onCheckout = overrides.onCheckout ?? jest.fn();
  const flow = new CheckoutFlow('stand-001', draft(n), {
    client: () => ({ createOrder, checkout }) as unknown as ApiClient,
    upload,
    onCheckout,
    now: overrides.now,
  });
  const settle = () => new Promise<void>((r) => setTimeout(r, 0));
  return { flow, createOrder, checkout, upload, onCheckout, settle };
}

describe('CheckoutFlow happy path', () => {
  it('creates the order, uploads every photo, checks out, hands over the URL', async () => {
    const { flow, createOrder, upload, checkout, onCheckout, settle } =
      setup(2);
    expect(flow.getState()).toMatchObject({ phase: 'idle', orderId: null });
    flow.start();
    await settle();
    expect(createOrder).toHaveBeenCalledWith({
      printerId: 'stand-001',
      photos: [
        { contentType: 'image/jpeg', sizeBytes: 100, copies: 1 },
        { contentType: 'image/png', sizeBytes: 101, copies: 2 },
      ],
    });
    expect(upload).toHaveBeenCalledWith(
      expect.objectContaining({
        url: 'https://s3.example/put/1',
        uri: 'file:///1.jpg',
        contentType: 'image/png',
        headers: {},
      }),
    );
    expect(checkout).toHaveBeenCalledWith('ord-1');
    expect(onCheckout).toHaveBeenCalledWith('https://pay.example/c', 'ord-1');
    expect(flow.getState()).toMatchObject({
      phase: 'redirected',
      orderId: 'ord-1',
    });
    expect(flow.getState().photos.map((p) => p.status)).toEqual([
      'done',
      'done',
    ]);
  });

  it('notifies subscribers and stops after unsubscribe', async () => {
    const { flow, settle } = setup(1);
    const seen = jest.fn();
    const off = flow.subscribe(seen);
    flow.start();
    await settle();
    expect(seen).toHaveBeenCalled();
    const n = seen.mock.calls.length;
    off();
    flow.retryPhoto(0);
    await settle();
    expect(seen.mock.calls.length).toBe(n);
  });

  it('reports upload progress', async () => {
    let report: (f: number) => void = () => {};
    let finish: () => void = () => {};
    const { flow, settle } = setup(1, {
      upload: (a) => {
        report = a.onProgress as (f: number) => void;
        return new Promise<void>((r) => (finish = r));
      },
    });
    flow.start();
    await settle();
    expect(flow.getState().phase).toBe('uploading');
    report(0.4);
    expect(flow.getState().photos[0]).toEqual({
      status: 'uploading',
      progress: 0.4,
    });
    finish();
    await settle();
    expect(flow.getState().photos[0]).toEqual({ status: 'done', progress: 1 });
  });

  it('uploads at most 3 photos at a time', async () => {
    let active = 0;
    let peak = 0;
    const { flow, settle, upload } = setup(5, {
      upload: async () => {
        active++;
        peak = Math.max(peak, active);
        await new Promise((r) => setTimeout(r, 5));
        active--;
      },
    });
    flow.start();
    await new Promise((r) => setTimeout(r, 40));
    await settle();
    expect(upload).toHaveBeenCalledTimes(5);
    expect(peak).toBe(3);
  });

  it('start is ignored while running and with an empty draft', async () => {
    const { flow, createOrder, settle } = setup(1);
    flow.start();
    flow.start();
    await settle();
    expect(createOrder).toHaveBeenCalledTimes(1);

    const empty = setup(0);
    empty.flow.start();
    await empty.settle();
    expect(empty.createOrder).not.toHaveBeenCalled();
    expect(empty.flow.getState().phase).toBe('idle');
  });

  it('uses the app client by default', async () => {
    const createOrder = jest.fn().mockResolvedValue(createdOrder(1));
    const checkout = jest.fn().mockResolvedValue(checkoutRes);
    setApiClient({ createOrder, checkout } as unknown as ApiClient);
    const onCheckout = jest.fn();
    const flow = new CheckoutFlow('p', draft(1), {
      upload: async () => {},
      onCheckout,
    });
    flow.start();
    await new Promise((r) => setTimeout(r, 0));
    expect(onCheckout).toHaveBeenCalled();
    setApiClient(undefined);
  });
});

describe('CheckoutFlow failures and retry', () => {
  it('a failed photo stops before checkout; retryPhoto uploads just that photo then checks out', async () => {
    let fail = true;
    const { flow, upload, checkout, onCheckout, settle } = setup(3, {
      upload: async (a) => {
        if (a.url.endsWith('/1') && fail) throw new ApiError('network', 'down');
      },
    });
    flow.start();
    await settle();
    expect(flow.getState().phase).toBe('error');
    expect(flow.getState().error).toBeNull();
    expect(flow.getState().photos.map((p) => p.status)).toEqual([
      'done',
      'error',
      'done',
    ]);
    expect(flow.getState().photos[1]?.error).toMatchObject({ code: 'network' });
    expect(checkout).not.toHaveBeenCalled();

    fail = false;
    upload.mockClear();
    flow.retryPhoto(1);
    await settle();
    expect(upload).toHaveBeenCalledTimes(1);
    expect(checkout).toHaveBeenCalledTimes(1);
    expect(onCheckout).toHaveBeenCalledTimes(1);
  });

  it('a photo that fails again stays in error without checkout', async () => {
    const { flow, checkout, settle } = setup(1, {
      upload: async () => {
        throw new ApiError('upload_failed', 'x', 403);
      },
    });
    flow.start();
    await settle();
    flow.retryPhoto(0);
    await settle();
    expect(flow.getState().phase).toBe('error');
    expect(checkout).not.toHaveBeenCalled();
  });

  it('start() after photo failures retries only the failed ones', async () => {
    let calls = 0;
    const { flow, createOrder, upload, settle } = setup(2, {
      upload: async (a) => {
        calls++;
        if (a.url.endsWith('/0') && calls === 1) throw new Error('flaky');
      },
    });
    flow.start();
    await settle();
    upload.mockClear();
    flow.start();
    await settle();
    expect(createOrder).toHaveBeenCalledTimes(1);
    expect(upload).toHaveBeenCalledTimes(1);
    expect(flow.getState().phase).toBe('redirected');
  });

  it('retryPhoto of an unknown index is a no-op that re-evaluates', async () => {
    const { flow, upload, settle } = setup(1, { upload: async () => {} });
    flow.retryPhoto(7);
    await settle();
    expect(upload).not.toHaveBeenCalled();
    expect(flow.getState().phase).toBe('error'); // no order yet
  });

  it('retryPhoto is ignored while busy', async () => {
    let release: () => void = () => {};
    const { flow, upload, settle } = setup(1, {
      upload: () => new Promise<void>((r) => (release = r)),
    });
    flow.start();
    await settle();
    flow.retryPhoto(0);
    await settle();
    expect(upload).toHaveBeenCalledTimes(1);
    release();
    await settle();
  });

  it('a failed create surfaces the error and start() retries it', async () => {
    const createOrder = jest
      .fn()
      .mockRejectedValueOnce(new ApiError('validation', 'printer offline'))
      .mockResolvedValueOnce(createdOrder(1));
    const { flow, upload, settle } = setup(1, { createOrder });
    flow.start();
    await settle();
    expect(flow.getState()).toMatchObject({ phase: 'error', orderId: null });
    expect(flow.getState().error).toMatchObject({ code: 'validation' });
    expect(upload).not.toHaveBeenCalled();
    flow.start();
    await settle();
    expect(flow.getState().phase).toBe('redirected');
    expect(flow.getState().error).toBeNull();
  });

  it('a failed checkout keeps the order and uploads; start() retries only checkout', async () => {
    const checkout = jest
      .fn()
      .mockRejectedValueOnce(new ApiError('conflict', 'photo not uploaded'))
      .mockResolvedValueOnce(checkoutRes);
    const { flow, createOrder, upload, onCheckout, settle } = setup(2, {
      checkout,
    });
    flow.start();
    await settle();
    expect(flow.getState().phase).toBe('error');
    expect(flow.getState().error).toMatchObject({ code: 'conflict' });
    upload.mockClear();
    flow.start();
    await settle();
    expect(createOrder).toHaveBeenCalledTimes(1);
    expect(upload).not.toHaveBeenCalled();
    expect(checkout).toHaveBeenCalledTimes(2);
    expect(onCheckout).toHaveBeenCalledTimes(1);
  });

  it('an onCheckout failure is reported as an error', async () => {
    const onCheckout = jest.fn().mockRejectedValue(new Error('browser failed'));
    const { flow, settle } = setup(1, { onCheckout });
    flow.start();
    await settle();
    expect(flow.getState().phase).toBe('error');
    expect(flow.getState().error).toMatchObject({ message: 'browser failed' });
  });

  it('does not upload to an expired URL', async () => {
    const createOrder = jest.fn().mockResolvedValue({
      ...createdOrder(1),
      uploads: [
        { ...createdOrder(1).uploads[0], expiresAt: '2000-01-01T00:00:00Z' },
      ],
    });
    const { flow, upload, settle } = setup(1, { createOrder });
    flow.start();
    await settle();
    expect(upload).not.toHaveBeenCalled();
    expect(flow.getState().photos[0]?.status).toBe('error');
  });

  it('honours an injected clock for expiry', async () => {
    const { flow, upload, settle } = setup(1, {
      now: () => Date.parse(farFuture) + 1,
    });
    flow.start();
    await settle();
    expect(upload).not.toHaveBeenCalled();
  });

  it('tolerates a server that returns fewer upload slots than photos', async () => {
    const createOrder = jest.fn().mockResolvedValue({
      ...createdOrder(2),
      uploads: createdOrder(1).uploads,
    });
    const { flow, upload, checkout, settle } = setup(2, { createOrder });
    flow.start();
    await settle();
    expect(upload).toHaveBeenCalledTimes(1);
    expect(flow.getState().photos[1]).toMatchObject({
      status: 'error',
      error: { code: 'invalid_response' },
    });
    expect(checkout).not.toHaveBeenCalled();
  });
});

describe('CheckoutFlow lifecycle', () => {
  it('unmount mid-upload aborts the upload and never checks out or calls onCheckout', async () => {
    let signal: AbortSignal | undefined;
    let fail: (e: unknown) => void = () => {};
    const { flow, checkout, onCheckout, settle } = setup(1, {
      upload: (a) => {
        signal = a.signal;
        return new Promise<void>((_, rej) => (fail = rej));
      },
    });
    flow.start();
    await settle();
    expect(signal?.aborted).toBe(false);
    const before = flow.getState();
    flow.dispose();
    expect(signal?.aborted).toBe(true);
    fail(new ApiError('aborted', 'upload aborted'));
    await settle();
    expect(checkout).not.toHaveBeenCalled();
    expect(onCheckout).not.toHaveBeenCalled();
    expect(flow.getState()).toBe(before);
  });

  it('a successful upload finishing after dispose does not check out', async () => {
    let finish: () => void = () => {};
    const { flow, checkout, onCheckout, settle } = setup(1, {
      upload: () => new Promise<void>((r) => (finish = r)),
    });
    flow.start();
    await settle();
    flow.dispose();
    finish();
    await settle();
    expect(checkout).not.toHaveBeenCalled();
    expect(onCheckout).not.toHaveBeenCalled();
  });

  it('dispose while checkout is in flight: onCheckout is not called', async () => {
    let release: (v: typeof checkoutRes) => void = () => {};
    const checkout = jest.fn(
      () => new Promise<typeof checkoutRes>((r) => (release = r)),
    );
    const { flow, onCheckout, settle } = setup(1, { checkout });
    flow.start();
    await settle();
    flow.dispose();
    release(checkoutRes);
    await settle();
    expect(onCheckout).not.toHaveBeenCalled();
    expect(flow.getState().phase).toBe('checkingOut');
  });

  it('start after dispose is ignored until arm()', async () => {
    const { flow, createOrder, settle } = setup(1);
    flow.dispose();
    flow.start();
    await settle();
    expect(createOrder).not.toHaveBeenCalled();
    flow.arm();
    flow.start();
    await settle();
    expect(createOrder).toHaveBeenCalledTimes(1);
  });

  it('StrictMode: dispose + arm + start reuses the in-flight order and finishes once', async () => {
    let created: (v: ReturnType<typeof createdOrder>) => void = () => {};
    const createOrder = jest.fn(
      () => new Promise<ReturnType<typeof createdOrder>>((r) => (created = r)),
    );
    const { flow, upload, checkout, onCheckout, settle } = setup(1, {
      createOrder,
    });
    flow.start();
    flow.dispose();
    flow.arm();
    flow.start();
    await settle();
    created(createdOrder(1));
    await settle();
    expect(createOrder).toHaveBeenCalledTimes(1);
    expect(upload).toHaveBeenCalledTimes(1);
    expect(checkout).toHaveBeenCalledTimes(1);
    expect(onCheckout).toHaveBeenCalledTimes(1);
  });

  it('arm is a no-op when not disposed', async () => {
    const { flow, createOrder, settle } = setup(1);
    flow.arm();
    flow.start();
    await settle();
    expect(createOrder).toHaveBeenCalledTimes(1);
  });

  it('setDeps swaps the callbacks used later', async () => {
    const { flow, settle } = setup(1);
    const other = jest.fn();
    flow.setDeps({
      client: () =>
        ({
          createOrder: jest.fn().mockResolvedValue(createdOrder(1)),
          checkout: jest.fn().mockResolvedValue(checkoutRes),
        }) as unknown as ApiClient,
      upload: async () => {},
      onCheckout: other,
    });
    flow.start();
    await settle();
    expect(other).toHaveBeenCalled();
  });
});

describe('CheckoutFlow expiry and restart', () => {
  afterEach(() => serverClock.reset());

  it('an expired upload URL puts the flow in the expired phase (not error)', async () => {
    const createOrder = jest.fn().mockResolvedValue({
      ...createdOrder(1),
      uploads: [
        { ...createdOrder(1).uploads[0], expiresAt: '2000-01-01T00:00:00Z' },
      ],
    });
    const { flow, checkout, settle } = setup(1, { createOrder });
    flow.start();
    await settle();
    expect(flow.getState().phase).toBe('expired');
    expect(flow.getState().photos[0]?.error).toMatchObject({ code: 'expired' });
    expect(checkout).not.toHaveBeenCalled();
  });

  it('applies a safety margin', async () => {
    const t = Date.parse('2030-01-01T00:00:00Z');
    const createOrder = jest.fn().mockResolvedValue({
      ...createdOrder(1),
      uploads: [
        {
          ...createdOrder(1).uploads[0],
          expiresAt: new Date(t + EXPIRY_MARGIN_MS - 1).toISOString(),
        },
      ],
      expiresAt: new Date(t + 900_000).toISOString(),
    });
    const { flow, upload, settle } = setup(1, { createOrder, now: () => t });
    flow.start();
    await settle();
    expect(upload).not.toHaveBeenCalled();
    expect(flow.getState().phase).toBe('expired');
  });

  it('the order expiring before checkout is expired too', async () => {
    let t = Date.parse('2030-01-01T00:00:00Z');
    const createOrder = jest.fn().mockResolvedValue({
      ...createdOrder(1),
      expiresAt: new Date(t + 60_000).toISOString(),
      uploads: [
        {
          ...createdOrder(1).uploads[0],
          expiresAt: new Date(t + 900_000).toISOString(),
        },
      ],
    });
    const { flow, checkout, settle } = setup(1, {
      createOrder,
      upload: async () => {
        t += 45_000;
      },
      now: () => t,
    });
    flow.start();
    await settle();
    expect(flow.getState().phase).toBe('expired');
    expect(checkout).not.toHaveBeenCalled();
  });

  it('uses the server clock offset by default', async () => {
    // Device clock is 1 h behind the server: a URL valid for 30 min of device time is expired.
    serverClock.observe(new Date(Date.now() + 3_600_000).toUTCString());
    const createOrder = jest.fn().mockResolvedValue({
      ...createdOrder(1),
      expiresAt: new Date(Date.now() + 1_800_000).toISOString(),
      uploads: [
        {
          ...createdOrder(1).uploads[0],
          expiresAt: new Date(Date.now() + 1_800_000).toISOString(),
        },
      ],
    });
    const { flow, upload, settle } = setup(1, { createOrder });
    flow.start();
    await settle();
    expect(upload).not.toHaveBeenCalled();
    expect(flow.getState().phase).toBe('expired');
  });

  it('restart cancels the old order best-effort and creates a new one with the same photos', async () => {
    const createOrder = jest
      .fn()
      .mockResolvedValueOnce({
        ...createdOrder(1),
        uploads: [
          { ...createdOrder(1).uploads[0], expiresAt: '2000-01-01T00:00:00Z' },
        ],
      })
      .mockResolvedValueOnce({ ...createdOrder(1), orderId: 'ord-2' });
    const cancelOrder = jest
      .fn()
      .mockRejectedValue(new ApiError('conflict', 'x'));
    const checkout = jest
      .fn()
      .mockResolvedValue({ ...checkoutRes, orderId: 'ord-2' });
    const upload = jest.fn(async () => {});
    const onCheckout = jest.fn();
    const flow = new CheckoutFlow('stand-001', draft(1), {
      client: () =>
        ({ createOrder, cancelOrder, checkout }) as unknown as ApiClient,
      upload,
      onCheckout,
    });
    flow.start();
    await new Promise((r) => setTimeout(r, 0));
    expect(flow.getState().phase).toBe('expired');
    flow.restart();
    await new Promise((r) => setTimeout(r, 0));
    expect(cancelOrder).toHaveBeenCalledWith('ord-1');
    expect(createOrder).toHaveBeenCalledTimes(2);
    expect(createOrder.mock.calls[1]).toEqual(createOrder.mock.calls[0]);
    expect(onCheckout).toHaveBeenCalledWith('https://pay.example/c', 'ord-2');
    expect(flow.getState().orderId).toBe('ord-2');
  });

  it('restart without an old order just starts', async () => {
    const createOrder = jest
      .fn()
      .mockRejectedValueOnce(new ApiError('network', 'x'))
      .mockResolvedValueOnce(createdOrder(1));
    const { flow, settle } = setup(1, { createOrder });
    flow.start();
    await settle();
    flow.restart();
    await settle();
    expect(flow.getState().phase).toBe('redirected');
  });
});
