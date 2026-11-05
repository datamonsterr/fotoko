import { fireEvent, screen } from '@testing-library/react-native';

import { renderApp, router } from '@/test/render';

import { HomeScreen } from './HomeScreen';

beforeEach(() => jest.clearAllMocks());

describe('HomeScreen', () => {
  it('opens the scanner', async () => {
    await renderApp(<HomeScreen />);
    await fireEvent.press(screen.getByTestId('home-scan-button'));
    expect(router.push).toHaveBeenCalledWith('/scan');
  });

  it('opens a typed printer id (trimmed) from the button and from submit', async () => {
    await renderApp(<HomeScreen />);
    await fireEvent.changeText(
      screen.getByTestId('home-printer-input'),
      '  stand-001 ',
    );
    await fireEvent.press(screen.getByTestId('home-printer-go'));
    expect(router.push).toHaveBeenLastCalledWith({
      pathname: '/p/[printerId]',
      params: { printerId: 'stand-001' },
    });
    await fireEvent(screen.getByTestId('home-printer-input'), 'submitEditing');
    expect(router.push).toHaveBeenCalledTimes(2);
  });

  it.each(['', '   ', 'a/b', 'a+b', 'a#b'])(
    'rejects invalid id %p and clears the error on edit',
    async (bad) => {
      await renderApp(<HomeScreen />);
      expect(screen.queryByTestId('home-printer-error')).toBeNull();
      await fireEvent.changeText(screen.getByTestId('home-printer-input'), bad);
      await fireEvent.press(screen.getByTestId('home-printer-go'));
      expect(screen.getByTestId('home-printer-error')).toHaveTextContent(
        'Mã máy in không hợp lệ.',
      );
      expect(router.push).not.toHaveBeenCalled();
      await fireEvent.changeText(
        screen.getByTestId('home-printer-input'),
        'ok',
      );
      expect(screen.queryByTestId('home-printer-error')).toBeNull();
    },
  );

  it('clears a previous error after a valid submit', async () => {
    await renderApp(<HomeScreen />);
    await fireEvent.press(screen.getByTestId('home-printer-go'));
    expect(screen.getByTestId('home-printer-error')).toBeOnTheScreen();
    await fireEvent.changeText(screen.getByTestId('home-printer-input'), 'x');
    await fireEvent.press(screen.getByTestId('home-printer-go'));
    expect(screen.queryByTestId('home-printer-error')).toBeNull();
  });

  it('toggles the language vi <-> en', async () => {
    await renderApp(<HomeScreen />);
    expect(screen.getByText('In ảnh tại chỗ')).toBeOnTheScreen();
    await fireEvent.press(screen.getByTestId('home-language-toggle'));
    expect(screen.getByText('On-the-spot photo printing')).toBeOnTheScreen();
    await fireEvent.press(screen.getByTestId('home-language-toggle'));
    expect(screen.getByText('In ảnh tại chỗ')).toBeOnTheScreen();
  });
});
