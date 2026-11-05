import { fireEvent, screen } from '@testing-library/react-native';

import { renderApp, router } from '@/test/render';

import { ScanScreen } from './ScanScreen';

const cam = (
  jest.requireMock('expo-camera') as {
    __state: { granted: boolean; request: jest.Mock };
  }
).__state;

beforeEach(() => {
  jest.clearAllMocks();
  cam.granted = true;
});

const scan = (data: string) =>
  fireEvent(screen.getByTestId('scan-camera'), 'barcodeScanned', { data });
const onScanned = (data: string) =>
  screen.getByTestId('scan-camera').props.onBarcodeScanned({ data });

describe('ScanScreen', () => {
  it('asks for the camera permission first', async () => {
    cam.granted = false;
    await renderApp(<ScanScreen />);
    expect(screen.queryByTestId('scan-camera')).toBeNull();
    await fireEvent.press(screen.getByTestId('scan-permission-grant'));
    expect(cam.request).toHaveBeenCalled();
    await fireEvent.press(screen.getByTestId('scan-manual'));
    expect(router.replace).toHaveBeenCalledWith('/');
  });

  it('configures a QR-only scanner', async () => {
    await renderApp(<ScanScreen />);
    expect(
      screen.getByTestId('scan-camera').props.barcodeScannerSettings,
    ).toEqual({ barcodeTypes: ['qr'] });
  });

  it.each([
    ['https://fotoko.example/p/stand-001', 'stand-001'],
    ['fotoko://p/stand-009', 'stand-009'],
  ])('navigates for %s', async (data, id) => {
    await renderApp(<ScanScreen />);
    await scan(data);
    expect(router.replace).toHaveBeenCalledWith({
      pathname: '/p/[printerId]',
      params: { printerId: id },
    });
  });

  it('handles one code even if the camera repeats it', async () => {
    await renderApp(<ScanScreen />);
    await scan('fotoko://p/a');
    await scan('fotoko://p/a');
    expect(router.replace).toHaveBeenCalledTimes(1);
  });

  it.each([
    'https://evil.example/x',
    'http://fotoko.example/p/a',
    'hello',
    'fotoko://p/a%2Fb',
  ])('rejects %p and lets the user scan again', async (data) => {
    await renderApp(<ScanScreen />);
    await scan(data);
    expect(router.replace).not.toHaveBeenCalled();
    expect(screen.getByTestId('scan-error')).toHaveTextContent(
      'Đây không phải mã QR của máy in Fotoko.',
    );
    expect(
      screen.getByTestId('scan-camera').props.onBarcodeScanned,
    ).toBeUndefined();
    await fireEvent.press(screen.getByTestId('scan-again'));
    expect(screen.queryByTestId('scan-error')).toBeNull();
    await onScanned('fotoko://p/ok');
    expect(router.replace).toHaveBeenCalledWith({
      pathname: '/p/[printerId]',
      params: { printerId: 'ok' },
    });
  });
});
