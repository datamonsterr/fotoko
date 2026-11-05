import { render, screen } from '@testing-library/react-native';
import { useLocalSearchParams } from 'expo-router';

import HomeRoute from './index';
import RootLayout from './_layout';
import OrderRoute from './orders/[orderId]';
import PhotosRoute from './p/[printerId]/photos';
import PrinterRoute from './p/[printerId]/index';
import UploadRoute from './p/[printerId]/upload';
import ScanRoute from './scan';

/* eslint-disable @typescript-eslint/no-require-imports -- jest.mock factories load lazily */
function mockRenderStub(text: string) {
  const { Text } = require('react-native');
  return <Text testID="stub">{text}</Text>;
}
jest.mock('@/screens/PrinterScreen', () => ({
  PrinterScreen: (p: { printerId: string }) =>
    mockRenderStub(`printer:${p.printerId}`),
}));
jest.mock('@/screens/PhotosScreen', () => ({
  PhotosScreen: (p: { printerId: string }) =>
    mockRenderStub(`photos:${p.printerId}`),
}));
jest.mock('@/screens/UploadScreen', () => ({
  UploadScreen: (p: { printerId: string }) =>
    mockRenderStub(`upload:${p.printerId}`),
}));
jest.mock('@/screens/OrderScreen', () => ({
  OrderScreen: (p: { orderId: string }) => mockRenderStub(`order:${p.orderId}`),
}));

describe('routes', () => {
  it('param routes pass the id to their screen', async () => {
    (useLocalSearchParams as jest.Mock).mockReturnValue({
      printerId: 'stand-7',
      orderId: 'ord-9',
    });
    const cases: [() => React.ReactElement, string][] = [
      [PrinterRoute, 'printer:stand-7'],
      [PhotosRoute, 'photos:stand-7'],
      [UploadRoute, 'upload:stand-7'],
      [OrderRoute, 'order:ord-9'],
    ];
    for (const [Route, id] of cases) {
      const { unmount } = await render(<Route />);
      expect(screen.getByTestId('stub')).toHaveTextContent(id);
      await unmount();
    }
  });

  it('home and scan routes render their screens', async () => {
    await render(<HomeRoute />);
    expect(screen.getByTestId('home-screen')).toBeOnTheScreen();
    await render(<ScanRoute />);
    expect(screen.getByTestId('scan-screen')).toBeOnTheScreen();
  });

  it('root layout mounts providers and the stack', async () => {
    await render(<RootLayout />);
    expect(screen.toJSON()).toBeNull();
  });
});
