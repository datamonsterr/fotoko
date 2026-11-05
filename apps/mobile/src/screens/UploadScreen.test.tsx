import { BackHandler, Platform } from 'react-native';
import { act, fireEvent, screen, waitFor } from '@testing-library/react-native';

import { ApiError, putPhoto, setApiClient, type ApiClient } from '@/api';
import type { UploadArgs } from '@/api/upload';
import { openCheckoutUrl } from '@/lib/checkout';
import { createdOrder } from '@/test/fixtures';
import { draftPhoto, renderApp, router } from '@/test/render';

import { UploadScreen } from './UploadScreen';

jest.mock('@/lib/checkout', () => ({ openCheckoutUrl: jest.fn() }));
jest.mock('@/api/upload', () => ({
  ...jest.requireActual('@/api/upload'),
  putPhoto: jest.fn(),
}));

const stackScreen = (
  jest.requireMock('expo-router') as { __stackScreen: jest.Mock }
).__stackScreen;

const put = putPhoto as jest.Mock;
const openCheckout = openCheckoutUrl as jest.Mock;

beforeEach(() => {
  jest.clearAllMocks();
  put.mockResolvedValue(undefined);
  openCheckout.mockResolvedValue(undefined);
});

const checkoutRes = {
  orderId: 'ord-1',
  status: 'awaiting_payment',
  orderCode: 1,
  checkoutUrl: 'https://pay.example/c',
  qrCode: 'q',
};

function setup(
  n: number,
  opts: {
    upload?: (a: UploadArgs) => Promise<void>;
    checkout?: jest.Mock;
    createOrder?: jest.Mock;
    cancelOrder?: jest.Mock;
  } = {},
) {
  const createOrder =
    opts.createOrder ?? jest.fn().mockResolvedValue(createdOrder(n));
  const checkout = opts.checkout ?? jest.fn().mockResolvedValue(checkoutRes);
  const cancelOrder = opts.cancelOrder ?? jest.fn().mockResolvedValue({});
  setApiClient({ createOrder, checkout, cancelOrder } as unknown as ApiClient);
  if (opts.upload) put.mockImplementation(opts.upload);
  const photos = Array.from({ length: n }, (_, i) => draftPhoto(i));
  return {
    ui: <UploadScreen printerId="stand-001" />,
    photos,
    createOrder,
    checkout,
    cancelOrder,
  };
}

const lastOptions = () =>
  (stackScreen.mock.calls.at(-1) as unknown as [{ options: object }])[0]
    .options;

describe('UploadScreen', () => {
  it('happy path: creates, uploads, checks out, opens payOS, then shows the order', async () => {
    const s = setup(2);
    await renderApp(s.ui, { photos: s.photos });
    await waitFor(() =>
      expect(router.replace).toHaveBeenCalledWith({
        pathname: '/orders/[orderId]',
        params: { orderId: 'ord-1' },
      }),
    );
    expect(s.createOrder).toHaveBeenCalledTimes(1);
    expect(put).toHaveBeenCalledTimes(2);
    expect(openCheckout).toHaveBeenCalledWith('https://pay.example/c');
    expect(screen.getByTestId('upload-photo-status-1')).toHaveTextContent(
      'Đã tải lên',
    );
  });

  it('shows per-photo progress with a progressbar role', async () => {
    let report: (f: number) => void = () => {};
    const s = setup(1, {
      upload: (a) => {
        report = a.onProgress as (f: number) => void;
        return new Promise(() => {});
      },
    });
    await renderApp(s.ui, { photos: s.photos });
    await waitFor(() =>
      expect(screen.getByTestId('upload-photo-status-0')).toHaveTextContent(
        'Đang tải lên 0%',
      ),
    );
    await act(async () => report(0.5));
    expect(screen.getByTestId('upload-photo-status-0')).toHaveTextContent(
      'Đang tải lên 50%',
    );
    expect(
      screen.getByTestId('upload-photo-status-0').props.accessibilityValue,
    ).toEqual({ min: 0, max: 100, now: 50 });
  });

  it('pending photos are labelled while the order is being created', async () => {
    const s = setup(1, { createOrder: jest.fn(() => new Promise(() => {})) });
    await renderApp(s.ui, { photos: s.photos });
    await waitFor(() =>
      expect(screen.getByTestId('upload-phase')).toHaveTextContent(
        'Đang tạo đơn hàng...',
      ),
    );
    expect(screen.getByTestId('upload-photo-status-0')).toHaveTextContent(
      'Đang chờ',
    );
    expect(screen.getByTestId('upload-busy')).toBeOnTheScreen();
  });

  it('a failed photo shows an error and a per-photo retry; retry finishes the checkout', async () => {
    let fail = true;
    const s = setup(2, {
      upload: async (a) => {
        if (fail && a.url.endsWith('/0')) throw new ApiError('network', 'x');
      },
    });
    await renderApp(s.ui, { photos: s.photos });
    await waitFor(() =>
      expect(screen.getByTestId('upload-photo-retry-0')).toBeOnTheScreen(),
    );
    expect(screen.getByTestId('upload-photo-status-0')).toHaveTextContent(
      'Tải lên thất bại',
    );
    expect(screen.queryByTestId('upload-photo-retry-1')).toBeNull();
    expect(screen.getByTestId('upload-photo-retry-0')).toBeEnabled();
    expect(s.checkout).not.toHaveBeenCalled();
    fail = false;
    await fireEvent.press(screen.getByTestId('upload-photo-retry-0'));
    await waitFor(() => expect(router.replace).toHaveBeenCalled());
    expect(s.checkout).toHaveBeenCalledTimes(1);
  });

  it('retry is disabled while another photo is still uploading', async () => {
    let release: () => void = () => {};
    const s = setup(2, {
      upload: (a) =>
        a.url.endsWith('/0')
          ? Promise.reject(new ApiError('network', 'x'))
          : new Promise<void>((r) => (release = r)),
    });
    await renderApp(s.ui, { photos: s.photos });
    await waitFor(() =>
      expect(screen.getByTestId('upload-photo-retry-0')).toBeOnTheScreen(),
    );
    expect(screen.getByTestId('upload-photo-retry-0')).toBeDisabled();
    await act(async () => release());
    await waitFor(() =>
      expect(screen.getByTestId('upload-photo-retry-0')).toBeEnabled(),
    );
  });

  it('a failed order creation shows the mapped error; retry resumes', async () => {
    const createOrder = jest
      .fn()
      .mockRejectedValueOnce(new ApiError('validation', 'printer not online'))
      .mockResolvedValueOnce(createdOrder(1));
    const s = setup(1, { createOrder });
    await renderApp(s.ui, { photos: s.photos });
    await waitFor(() =>
      expect(screen.getByTestId('upload-error')).toHaveTextContent(
        /Dữ liệu không hợp lệ/,
      ),
    );
    await fireEvent.press(screen.getByTestId('upload-retry'));
    await waitFor(() => expect(router.replace).toHaveBeenCalled());
    expect(createOrder).toHaveBeenCalledTimes(2);
  });

  it('a conflict at checkout is shown and can be retried without re-uploading', async () => {
    const checkout = jest
      .fn()
      .mockRejectedValueOnce(new ApiError('conflict', 'x'))
      .mockResolvedValueOnce(checkoutRes);
    const s = setup(1, { checkout });
    await renderApp(s.ui, { photos: s.photos });
    await waitFor(() =>
      expect(screen.getByTestId('upload-error')).toHaveTextContent(
        /Trạng thái đơn hàng đã thay đổi/,
      ),
    );
    await fireEvent.press(screen.getByTestId('upload-retry'));
    await waitFor(() => expect(router.replace).toHaveBeenCalled());
    expect(put).toHaveBeenCalledTimes(1);
  });

  it('shows the checkout phase while the payment page opens', async () => {
    openCheckout.mockReturnValue(new Promise(() => {}));
    const s = setup(1);
    await renderApp(s.ui, { photos: s.photos });
    await waitFor(() =>
      expect(screen.getByTestId('upload-phase')).toHaveTextContent(
        'Đang mở trang thanh toán...',
      ),
    );
    expect(router.replace).not.toHaveBeenCalled();
  });

  it('an empty draft does nothing and offers a way back', async () => {
    const s = setup(0);
    await renderApp(s.ui, { photos: [] });
    expect(s.createOrder).not.toHaveBeenCalled();
    await fireEvent.press(screen.getByTestId('upload-back'));
    expect(router.back).toHaveBeenCalled();
  });

  describe('expired order', () => {
    const expiredOrder = () => ({
      ...createdOrder(1),
      uploads: [
        { ...createdOrder(1).uploads[0], expiresAt: '2000-01-01T00:00:00Z' },
      ],
    });

    it('shows a distinct message and Start over, which cancels the old order and re-creates', async () => {
      const createOrder = jest
        .fn()
        .mockResolvedValueOnce(expiredOrder())
        .mockResolvedValueOnce({ ...createdOrder(1), orderId: 'ord-2' });
      const checkout = jest
        .fn()
        .mockResolvedValue({ ...checkoutRes, orderId: 'ord-2' });
      const s = setup(1, { createOrder, checkout });
      await renderApp(s.ui, { photos: s.photos });
      await waitFor(() =>
        expect(screen.getByTestId('upload-expired')).toHaveTextContent(
          /hết hạn/,
        ),
      );
      expect(put).not.toHaveBeenCalled();
      expect(screen.queryByTestId('upload-error')).toBeNull();
      // a per-photo retry cannot help an expired order: it is disabled, Start over is the way
      expect(screen.getByTestId('upload-photo-retry-0')).toBeDisabled();
      await fireEvent.press(screen.getByTestId('upload-restart'));
      await waitFor(() =>
        expect(router.replace).toHaveBeenCalledWith({
          pathname: '/orders/[orderId]',
          params: { orderId: 'ord-2' },
        }),
      );
      expect(s.cancelOrder).toHaveBeenCalledWith('ord-1');
      expect(createOrder).toHaveBeenCalledTimes(2);
    });
  });

  describe('leaving mid-flight', () => {
    it('blocks the back gesture, header back and hardware back while working', async () => {
      let handler: () => boolean = () => false;
      const remove = jest.fn();
      const spy = jest
        .spyOn(BackHandler, 'addEventListener')
        .mockImplementation(((_: string, h: () => boolean) => {
          handler = h;
          return { remove };
        }) as never);
      const s = setup(1, { upload: () => new Promise(() => {}) });
      await renderApp(s.ui, { photos: s.photos });
      await waitFor(() =>
        expect(screen.getByTestId('upload-photo-status-0')).toHaveTextContent(
          /Đang tải lên/,
        ),
      );
      expect(lastOptions()).toEqual({
        gestureEnabled: false,
        headerBackVisible: false,
      });
      expect(handler()).toBe(true);
      spy.mockRestore();
    });

    it('allows leaving again after a failure, and releases the hardware back', async () => {
      const remove = jest.fn();
      const spy = jest
        .spyOn(BackHandler, 'addEventListener')
        .mockImplementation((() => ({ remove })) as never);
      const s = setup(1, {
        upload: async () => {
          throw new ApiError('network', 'x');
        },
      });
      await renderApp(s.ui, { photos: s.photos });
      await waitFor(() =>
        expect(screen.getByTestId('upload-photo-retry-0')).toBeOnTheScreen(),
      );
      expect(lastOptions()).toEqual({
        gestureEnabled: true,
        headerBackVisible: true,
      });
      expect(remove).toHaveBeenCalled();
      spy.mockRestore();
    });

    it('unmounting mid-upload aborts it and never checks out or opens payOS', async () => {
      let signal: AbortSignal | undefined;
      let finish: () => void = () => {};
      const s = setup(1, {
        upload: (a) => {
          signal = a.signal;
          return new Promise<void>((r) => (finish = r));
        },
      });
      const { unmount } = await renderApp(s.ui, { photos: s.photos });
      await waitFor(() => expect(put).toHaveBeenCalled());
      await unmount();
      expect(signal?.aborted).toBe(true);
      finish();
      await act(async () => {
        await new Promise((r) => setTimeout(r, 10));
      });
      expect(s.checkout).not.toHaveBeenCalled();
      expect(openCheckout).not.toHaveBeenCalled();
      expect(router.replace).not.toHaveBeenCalled();
    });
  });

  describe('web', () => {
    const original = Platform.OS;
    beforeEach(() => {
      Platform.OS = 'web';
    });
    afterEach(() => {
      Platform.OS = original;
    });

    it('opens payOS and stays: no router.replace (payOS returnUrl brings the user back)', async () => {
      const s = setup(1);
      await renderApp(s.ui, { photos: s.photos });
      await waitFor(() =>
        expect(openCheckout).toHaveBeenCalledWith('https://pay.example/c'),
      );
      await waitFor(() =>
        expect(screen.getByTestId('upload-phase')).toHaveTextContent(
          'Đang mở trang thanh toán...',
        ),
      );
      expect(router.replace).not.toHaveBeenCalled();
    });
  });
});
