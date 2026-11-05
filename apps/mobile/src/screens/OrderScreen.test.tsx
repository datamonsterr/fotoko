import { Text } from 'react-native';
import { fireEvent, screen, waitFor } from '@testing-library/react-native';

import { ApiError, type ApiClient, type Order, type OrderStatus } from '@/api';
import { order } from '@/test/fixtures';
import { usePhotoDraft } from '@/state/draft';
import { draftPhoto, renderApp, router } from '@/test/render';

import { OrderScreen } from './OrderScreen';

beforeEach(() => jest.clearAllMocks());

const at = (status: OrderStatus, extra: Partial<Order> = {}): Order => ({
  ...order,
  status,
  ...extra,
});
const clientWith =
  (api: Partial<Record<'getOrder' | 'cancelOrder', jest.Mock>>) => () =>
    api as unknown as ApiClient;

describe('OrderScreen', () => {
  it('loading', async () => {
    await renderApp(
      <OrderScreen
        orderId="ord-1"
        client={clientWith({ getOrder: jest.fn(() => new Promise(() => {})) })}
      />,
    );
    expect(screen.getByTestId('order-loading')).toBeOnTheScreen();
  });

  it('unknown order: error with a way home', async () => {
    const getOrder = jest
      .fn()
      .mockRejectedValue(new ApiError('not_found', 'x', 404));
    await renderApp(
      <OrderScreen orderId="nope" client={clientWith({ getOrder })} />,
    );
    await waitFor(() =>
      expect(screen.getByTestId('order-error')).toHaveTextContent(
        /Không tìm thấy/,
      ),
    );
    await fireEvent.press(screen.getByTestId('order-home'));
    expect(router.replace).toHaveBeenCalledWith('/');
  });

  it('transient failure before the first response keeps the spinner path out of the way', async () => {
    const getOrder = jest.fn().mockRejectedValue(new ApiError('network', 'x'));
    await renderApp(
      <OrderScreen
        orderId="o"
        client={clientWith({ getOrder })}
        intervalMs={5}
      />,
    );
    await waitFor(() =>
      expect(screen.getByTestId('order-error')).toHaveTextContent(
        /Không có kết nối mạng/,
      ),
    );
    expect(screen.queryByTestId('order-home')).toBeNull();
  });

  it('awaiting payment: shows status, amount, reference, timeline and cancel', async () => {
    const getOrder = jest.fn().mockResolvedValue(at('awaiting_payment'));
    await renderApp(
      <OrderScreen orderId="ord-1" client={clientWith({ getOrder })} />,
    );
    await waitFor(() =>
      expect(screen.getByTestId('order-status')).toHaveTextContent(
        'Chờ thanh toán',
      ),
    );
    expect(screen.getByTestId('order-amount')).toHaveTextContent(
      'Số tiền: 20.000 ₫',
    );
    expect(screen.getByTestId('order-reference')).toHaveTextContent(
      'Mã đơn: ord-1',
    );
    expect(screen.getByTestId('order-step-awaiting_payment')).toBeOnTheScreen();
    expect(screen.getByTestId('order-cancel')).toBeOnTheScreen();
    expect(screen.queryByTestId('order-done')).toBeNull();
    expect(screen.queryByTestId('order-again')).toBeNull();
  });

  it.each(['paid', 'printing'] as const)(
    '%s: cannot cancel',
    async (status) => {
      const getOrder = jest.fn().mockResolvedValue(at(status));
      await renderApp(
        <OrderScreen orderId="ord-1" client={clientWith({ getOrder })} />,
      );
      await waitFor(() =>
        expect(screen.getByTestId('order-status')).toBeOnTheScreen(),
      );
      expect(screen.queryByTestId('order-cancel')).toBeNull();
    },
  );

  it('cancel: calls the API, shows the cancelled result and stops', async () => {
    const getOrder = jest.fn().mockResolvedValue(at('awaiting_payment'));
    const cancelOrder = jest.fn().mockResolvedValue(at('cancelled'));
    await renderApp(
      <OrderScreen
        orderId="ord-1"
        client={clientWith({ getOrder, cancelOrder })}
      />,
    );
    await waitFor(() =>
      expect(screen.getByTestId('order-cancel')).toBeOnTheScreen(),
    );
    await fireEvent.press(screen.getByTestId('order-cancel'));
    await waitFor(() =>
      expect(screen.getByTestId('order-cancelled')).toHaveTextContent(
        'Đơn hàng đã hủy',
      ),
    );
    expect(cancelOrder).toHaveBeenCalledWith('ord-1');
    expect(screen.queryByTestId('order-cancel')).toBeNull();
    expect(screen.getByTestId('order-again')).toBeOnTheScreen();
  });

  it('cancel failure (conflict: already paid) is shown and the button stays', async () => {
    const getOrder = jest.fn().mockResolvedValue(at('awaiting_payment'));
    const cancelOrder = jest
      .fn()
      .mockRejectedValue(new ApiError('conflict', 'x'));
    await renderApp(
      <OrderScreen
        orderId="ord-1"
        client={clientWith({ getOrder, cancelOrder })}
      />,
    );
    await waitFor(() =>
      expect(screen.getByTestId('order-cancel')).toBeOnTheScreen(),
    );
    await fireEvent.press(screen.getByTestId('order-cancel'));
    await waitFor(() =>
      expect(screen.getByTestId('order-cancel-error')).toHaveTextContent(
        /Trạng thái đơn hàng đã thay đổi/,
      ),
    );
    expect(screen.getByTestId('order-cancel')).toBeEnabled();
  });

  it('completed: done screen, all steps done, actions', async () => {
    const getOrder = jest.fn().mockResolvedValue(at('completed'));
    await renderApp(
      <OrderScreen orderId="ord-1" client={clientWith({ getOrder })} />,
    );
    await waitFor(() =>
      expect(screen.getByTestId('order-done')).toHaveTextContent(/Đã in xong!/),
    );
    expect(screen.getByTestId('order-done')).toHaveTextContent(
      /Lấy ảnh của bạn tại máy in/,
    );
    expect(screen.queryByTestId('order-cancel')).toBeNull();
    await fireEvent.press(screen.getByTestId('order-again'));
    expect(router.replace).toHaveBeenCalledWith({
      pathname: '/p/[printerId]',
      params: { printerId: 'stand-001' },
    });
    await fireEvent.press(screen.getByTestId('order-home'));
    expect(router.replace).toHaveBeenLastCalledWith('/');
  });

  it('failed: shows the server error message, or a default', async () => {
    const getOrder = jest
      .fn()
      .mockResolvedValue(
        at('failed', { error: { code: 'paper_out', message: 'Hết giấy' } }),
      );
    await renderApp(
      <OrderScreen orderId="ord-1" client={clientWith({ getOrder })} />,
    );
    await waitFor(() =>
      expect(screen.getByTestId('order-failed')).toHaveTextContent(
        /In không thành công/,
      ),
    );
    expect(screen.getByTestId('order-failed')).toHaveTextContent(/Hết giấy/);
  });

  it('failed without a message uses the generic text', async () => {
    const getOrder = jest
      .fn()
      .mockResolvedValue(at('failed', { error: { code: 'x', message: '' } }));
    await renderApp(
      <OrderScreen orderId="ord-1" client={clientWith({ getOrder })} />,
    );
    await waitFor(() =>
      expect(screen.getByTestId('order-failed')).toHaveTextContent(/hoàn tiền/),
    );
  });

  it('expired', async () => {
    const getOrder = jest.fn().mockResolvedValue(at('expired'));
    await renderApp(
      <OrderScreen orderId="ord-1" client={clientWith({ getOrder })} />,
      { locale: 'en' },
    );
    await waitFor(() =>
      expect(screen.getByTestId('order-expired')).toHaveTextContent(
        'Order expired',
      ),
    );
  });

  it('polls until the status changes', async () => {
    const getOrder = jest
      .fn()
      .mockResolvedValueOnce(at('paid'))
      .mockResolvedValue(at('completed'));
    await renderApp(
      <OrderScreen
        orderId="ord-1"
        client={clientWith({ getOrder })}
        intervalMs={5}
      />,
    );
    await waitFor(() =>
      expect(screen.getByTestId('order-done')).toBeOnTheScreen(),
    );
    expect(getOrder).toHaveBeenCalledTimes(2);
  });

  it('a failed refresh after a good response shows the error but keeps the order', async () => {
    const getOrder = jest
      .fn()
      .mockResolvedValueOnce(at('paid'))
      .mockRejectedValue(new ApiError('network', 'x'));
    await renderApp(
      <OrderScreen
        orderId="ord-1"
        client={clientWith({ getOrder })}
        intervalMs={5}
      />,
    );
    await waitFor(() =>
      expect(screen.getByTestId('order-error')).toBeOnTheScreen(),
    );
    expect(screen.getByTestId('order-status')).toHaveTextContent(
      'Đã thanh toán',
    );
  });

  describe('draft', () => {
    const Probe = () => {
      const { photos } = usePhotoDraft();
      return <Text testID="draft-count">{String(photos.length)}</Text>;
    };
    const renderWith = async (status: OrderStatus) => {
      const getOrder = jest.fn().mockResolvedValue(at(status));
      await renderApp(
        <>
          <OrderScreen orderId="ord-1" client={clientWith({ getOrder })} />
          <Probe />
        </>,
        { photos: [draftPhoto(0), draftPhoto(1)] },
      );
    };

    it.each(['awaiting_payment', 'paid', 'completed', 'cancelled'] as const)(
      'is cleared once the order is %s',
      async (status) => {
        await renderWith(status);
        await waitFor(() =>
          expect(screen.getByTestId('draft-count')).toHaveTextContent('0'),
        );
      },
    );

    it('is kept while the order is still awaiting_upload', async () => {
      await renderWith('awaiting_upload');
      await waitFor(() =>
        expect(screen.getByTestId('order-status')).toBeOnTheScreen(),
      );
      expect(screen.getByTestId('draft-count')).toHaveTextContent('2');
    });

    it('is kept when the order cannot be loaded', async () => {
      const getOrder = jest
        .fn()
        .mockRejectedValue(new ApiError('not_found', 'x', 404));
      await renderApp(
        <>
          <OrderScreen orderId="x" client={clientWith({ getOrder })} />
          <Probe />
        </>,
        { photos: [draftPhoto(0)] },
      );
      await waitFor(() =>
        expect(screen.getByTestId('order-error')).toBeOnTheScreen(),
      );
      expect(screen.getByTestId('draft-count')).toHaveTextContent('1');
    });
  });
});
