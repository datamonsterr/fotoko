import { fireEvent, screen, waitFor } from '@testing-library/react-native';

import { ApiError, type ApiClient, type Printer } from '@/api';
import { printer } from '@/test/fixtures';
import { renderApp, router } from '@/test/render';

import { PrinterScreen } from './PrinterScreen';

beforeEach(() => jest.clearAllMocks());

const clientWith = (getPrinter: jest.Mock) => () =>
  ({ getPrinter }) as unknown as ApiClient;

describe('PrinterScreen', () => {
  it('shows a spinner while loading', async () => {
    await renderApp(
      <PrinterScreen
        printerId="p"
        client={clientWith(jest.fn(() => new Promise(() => {})))}
      />,
    );
    expect(screen.getByTestId('printer-loading')).toBeOnTheScreen();
  });

  it('online: shows name, VND price, media, and the pick CTA', async () => {
    await renderApp(
      <PrinterScreen
        printerId="stand-001"
        client={clientWith(jest.fn().mockResolvedValue(printer))}
      />,
    );
    await waitFor(() =>
      expect(screen.getByTestId('printer-name')).toHaveTextContent('Stand 001'),
    );
    expect(screen.getByTestId('printer-price')).toHaveTextContent('10.000 ₫');
    expect(screen.getByText('Khổ giấy: 4x6')).toBeOnTheScreen();
    expect(screen.getByTestId('printer-status')).toHaveTextContent('Sẵn sàng');
    await fireEvent.press(screen.getByTestId('printer-pick-button'));
    expect(router.push).toHaveBeenCalledWith({
      pathname: '/p/[printerId]/photos',
      params: { printerId: 'stand-001' },
    });
  });

  it.each([
    ['offline', 'Máy in đang ngoại tuyến'],
    ['busy', 'Máy in đang bận, vui lòng thử lại sau ít phút.'],
    ['error', 'Máy in đang gặp lỗi, vui lòng báo nhân viên.'],
  ] as const)('%s: no CTA, explains, can re-check', async (status, text) => {
    const getPrinter = jest
      .fn()
      .mockResolvedValue({ ...printer, status } satisfies Printer);
    await renderApp(
      <PrinterScreen printerId="p" client={clientWith(getPrinter)} />,
    );
    await waitFor(() =>
      expect(screen.getByTestId('printer-status')).toHaveTextContent(text),
    );
    expect(screen.queryByTestId('printer-pick-button')).toBeNull();
    await fireEvent.press(screen.getByTestId('printer-retry'));
    await waitFor(() => expect(getPrinter).toHaveBeenCalledTimes(2));
  });

  it('English copy', async () => {
    await renderApp(
      <PrinterScreen
        printerId="p"
        client={clientWith(jest.fn().mockResolvedValue(printer))}
      />,
      { locale: 'en' },
    );
    await waitFor(() =>
      expect(screen.getByText('Choose photos to print')).toBeOnTheScreen(),
    );
  });

  it('not found', async () => {
    const getPrinter = jest
      .fn()
      .mockRejectedValue(new ApiError('not_found', 'x', 404));
    await renderApp(
      <PrinterScreen printerId="p" client={clientWith(getPrinter)} />,
    );
    await waitFor(() =>
      expect(screen.getByTestId('printer-not-found')).toHaveTextContent(
        'Không tìm thấy máy in này.',
      ),
    );
    await fireEvent.press(screen.getByTestId('printer-home'));
    expect(router.replace).toHaveBeenCalledWith('/');
  });

  it('other errors show the mapped message with retry', async () => {
    const getPrinter = jest
      .fn()
      .mockRejectedValueOnce(new ApiError('network', 'x'))
      .mockResolvedValueOnce(printer);
    await renderApp(
      <PrinterScreen printerId="p" client={clientWith(getPrinter)} />,
    );
    await waitFor(() =>
      expect(screen.getByTestId('printer-error')).toHaveTextContent(
        /Không có kết nối mạng/,
      ),
    );
    await fireEvent.press(screen.getByTestId('printer-retry'));
    await waitFor(() =>
      expect(screen.getByTestId('printer-name')).toBeOnTheScreen(),
    );
  });

  it('a non-API error is not treated as not-found', async () => {
    const getPrinter = jest.fn().mockRejectedValue(new Error('boom'));
    await renderApp(
      <PrinterScreen printerId="p" client={clientWith(getPrinter)} />,
    );
    await waitFor(() =>
      expect(screen.getByTestId('printer-error')).toHaveTextContent(
        /Đã có lỗi xảy ra/,
      ),
    );
  });
});
