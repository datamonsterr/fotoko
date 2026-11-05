import { Platform } from 'react-native';
import { fireEvent, screen, waitFor } from '@testing-library/react-native';

import { ApiError, type ApiClient } from '@/api';
import type { UploadArgs } from '@/api/upload';
import { createdOrder } from '@/test/fixtures';
import { draftPhoto, renderApp, router } from '@/test/render';

import { UploadScreen } from './UploadScreen';

beforeEach(() => jest.clearAllMocks());

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
    openCheckout?: jest.Mock;
  } = {},
) {
  const createOrder =
    opts.createOrder ?? jest.fn().mockResolvedValue(createdOrder(n));
  const checkout = opts.checkout ?? jest.fn().mockResolvedValue(checkoutRes);
  const openCheckout =
    opts.openCheckout ?? jest.fn().mockResolvedValue(undefined);
  const upload = jest.fn(opts.upload ?? (async () => {}));
  const photos = Array.from({ length: n }, (_, i) => draftPhoto(i));
  const ui = (
    <UploadScreen
      printerId="stand-001"
      client={() => ({ createOrder, checkout }) as unknown as ApiClient}
      upload={upload}
      openCheckout={openCheckout}
    />
  );
  return { ui, photos, createOrder, checkout, openCheckout, upload };
}

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
    expect(s.upload).toHaveBeenCalledTimes(2);
    expect(s.openCheckout).toHaveBeenCalledWith('https://pay.example/c');
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
    const { act } = jest.requireActual(
      '@testing-library/react-native',
    ) as typeof import('@testing-library/react-native');
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
    expect(s.checkout).not.toHaveBeenCalled();
    fail = false;
    await fireEvent.press(screen.getByTestId('upload-photo-retry-0'));
    await waitFor(() => expect(router.replace).toHaveBeenCalled());
    expect(s.checkout).toHaveBeenCalledTimes(1);
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
    expect(s.upload).toHaveBeenCalledTimes(1);
  });

  it('shows the checkout phase while the payment page opens', async () => {
    const s = setup(1, { openCheckout: jest.fn(() => new Promise(() => {})) });
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

  it('uses the default checkout opener (web/native module) without crashing', async () => {
    const WebBrowser = jest.requireMock('expo-web-browser') as {
      openBrowserAsync: jest.Mock;
    };
    const createOrder = jest.fn().mockResolvedValue(createdOrder(1));
    const checkout = jest.fn().mockResolvedValue(checkoutRes);
    await renderApp(
      <UploadScreen
        printerId="p"
        client={() => ({ createOrder, checkout }) as unknown as ApiClient}
        upload={async () => {}}
      />,
      { photos: [draftPhoto(0)] },
    );
    await waitFor(() =>
      expect(WebBrowser.openBrowserAsync).toHaveBeenCalledWith(
        'https://pay.example/c',
      ),
    );
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
        expect(s.openCheckout).toHaveBeenCalledWith('https://pay.example/c'),
      );
      await waitFor(() =>
        expect(screen.getByTestId('upload-phase')).toHaveTextContent(
          'Đang mở trang thanh toán...',
        ),
      );
      expect(router.replace).not.toHaveBeenCalled();
    });
  });

  it('native: the draft is not cleared by the upload screen itself', async () => {
    const s = setup(1);
    await renderApp(s.ui, { photos: s.photos });
    await waitFor(() => expect(router.replace).toHaveBeenCalled());
    expect(screen.getByTestId('upload-photo-0')).toBeOnTheScreen();
  });
});
