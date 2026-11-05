import { fireEvent, screen, waitFor } from '@testing-library/react-native';
import * as ImagePicker from 'expo-image-picker';

import { ApiError, type ApiClient } from '@/api';
import { printer } from '@/test/fixtures';
import { draftPhoto, renderApp, router } from '@/test/render';

import { PhotosScreen } from './PhotosScreen';

const launch = ImagePicker.launchImageLibraryAsync as jest.Mock;

beforeEach(() => jest.clearAllMocks());

const client =
  (getPrinter = jest.fn().mockResolvedValue(printer)) =>
  () =>
    ({ getPrinter }) as unknown as ApiClient;
const asset = (name: string, extra = {}) => ({
  uri: `file:///${name}`,
  mimeType: 'image/jpeg',
  fileSize: 1000,
  ...extra,
});

async function renderReady(photos = [] as ReturnType<typeof draftPhoto>[]) {
  await renderApp(<PhotosScreen printerId="stand-001" client={client()} />, {
    photos,
  });
  await waitFor(() =>
    expect(screen.getByTestId('picker-add')).toBeOnTheScreen(),
  );
}

describe('PhotosScreen', () => {
  it('loading and error states', async () => {
    await renderApp(
      <PhotosScreen
        printerId="p"
        client={client(jest.fn(() => new Promise(() => {})))}
      />,
    );
    expect(screen.getByTestId('picker-loading')).toBeOnTheScreen();
  });

  it('error state retries', async () => {
    const getPrinter = jest
      .fn()
      .mockRejectedValueOnce(new ApiError('internal', 'x'))
      .mockResolvedValueOnce(printer);
    await renderApp(<PhotosScreen printerId="p" client={client(getPrinter)} />);
    await waitFor(() =>
      expect(screen.getByTestId('picker-error')).toBeOnTheScreen(),
    );
    await fireEvent.press(screen.getByTestId('picker-error-retry'));
    await waitFor(() =>
      expect(screen.getByTestId('picker-add')).toBeOnTheScreen(),
    );
  });

  it('empty: hint, total 0, continue disabled', async () => {
    await renderReady();
    expect(screen.getByTestId('picker-empty')).toHaveTextContent(
      'Chưa có ảnh nào. Chọn tối đa 10 ảnh.',
    );
    expect(screen.getByTestId('picker-total')).toHaveTextContent('0 ₫');
    expect(screen.getByTestId('picker-continue')).toBeDisabled();
  });

  it('adds picked photos, asking the picker for the remaining slots', async () => {
    launch.mockResolvedValue({
      canceled: false,
      assets: [asset('a.jpg'), asset('b.png', { mimeType: 'image/png' })],
    });
    await renderReady();
    await fireEvent.press(screen.getByTestId('picker-add'));
    await waitFor(() =>
      expect(screen.getByTestId('picker-photo-1')).toBeOnTheScreen(),
    );
    expect(launch).toHaveBeenCalledWith(
      expect.objectContaining({
        mediaTypes: ['images'],
        allowsMultipleSelection: true,
        selectionLimit: 10,
      }),
    );
    expect(screen.getByTestId('picker-total')).toHaveTextContent('20.000 ₫');
    expect(screen.queryByTestId('picker-empty')).toBeNull();
    expect(screen.getByTestId('picker-continue')).toBeEnabled();
  });

  it('cancelled picker changes nothing', async () => {
    launch.mockResolvedValue({ canceled: true, assets: null });
    await renderReady();
    await fireEvent.press(screen.getByTestId('picker-add'));
    await waitFor(() => expect(launch).toHaveBeenCalled());
    expect(screen.getByTestId('picker-empty')).toBeOnTheScreen();
  });

  it('live total follows copies and removal', async () => {
    await renderReady([draftPhoto(0, 1), draftPhoto(1, 2)]);
    expect(screen.getByTestId('picker-total')).toHaveTextContent('30.000 ₫');
    await fireEvent.press(screen.getByTestId('picker-copies-inc-0'));
    expect(screen.getByTestId('picker-copies-0')).toHaveTextContent(
      'Số bản in: 2',
    );
    expect(screen.getByTestId('picker-total')).toHaveTextContent('40.000 ₫');
    await fireEvent.press(screen.getByTestId('picker-copies-dec-1'));
    expect(screen.getByTestId('picker-total')).toHaveTextContent('30.000 ₫');
    await fireEvent.press(screen.getByTestId('picker-remove-0'));
    expect(screen.getByTestId('picker-total')).toHaveTextContent('10.000 ₫');
    expect(screen.getByText(/1 bản in/)).toBeOnTheScreen();
  });

  it('a pick that adds nothing new clears old notices', async () => {
    launch
      .mockResolvedValueOnce({
        canceled: false,
        assets: [asset('x.heic', { mimeType: 'image/heic' })],
      })
      .mockResolvedValueOnce({ canceled: false, assets: [] });
    await renderReady();
    await fireEvent.press(screen.getByTestId('picker-add'));
    await waitFor(() =>
      expect(screen.getByTestId('picker-notice-0')).toBeOnTheScreen(),
    );
    await fireEvent.press(screen.getByTestId('picker-add'));
    await waitFor(() =>
      expect(screen.queryByTestId('picker-notice-0')).toBeNull(),
    );
  });

  it('full order: add is disabled', async () => {
    await renderReady(Array.from({ length: 10 }, (_, i) => draftPhoto(i)));
    expect(screen.getByTestId('picker-add')).toBeDisabled();
    expect(launch).not.toHaveBeenCalled();
  });

  it('reports skipped photos: too big, unsupported, over the limit', async () => {
    launch.mockResolvedValue({
      canceled: false,
      assets: [
        asset('big.jpg', { fileSize: 16 * 1024 * 1024 }),
        asset('x.heic', { mimeType: 'image/heic' }),
        asset('ok.jpg'),
        asset('late.jpg'),
      ],
    });
    await renderReady(Array.from({ length: 9 }, (_, i) => draftPhoto(i)));
    await fireEvent.press(screen.getByTestId('picker-add'));
    await waitFor(() =>
      expect(screen.getByTestId('picker-notice-0')).toBeOnTheScreen(),
    );
    expect(launch).toHaveBeenCalledWith(
      expect.objectContaining({ selectionLimit: 1 }),
    );
    expect(screen.getByTestId('picker-notice-0')).toHaveTextContent(
      'Tối đa 10 ảnh mỗi đơn.',
    );
    expect(screen.getByTestId('picker-notice-1')).toHaveTextContent(
      /Ảnh quá lớn/,
    );
    expect(screen.getByTestId('picker-notice-2')).toHaveTextContent(
      /JPEG hoặc PNG/,
    );
    expect(screen.getByTestId('picker-add')).toBeDisabled();
  });

  it('continue goes to upload', async () => {
    await renderReady([draftPhoto(0)]);
    await fireEvent.press(screen.getByTestId('picker-continue'));
    expect(router.push).toHaveBeenCalledWith({
      pathname: '/p/[printerId]/upload',
      params: { printerId: 'stand-001' },
    });
  });
});
