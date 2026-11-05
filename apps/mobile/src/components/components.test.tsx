import { fireEvent, render, screen } from '@testing-library/react-native';

import { ApiError } from '@/api';
import { draftPhoto, renderApp } from '@/test/render';

import { Button } from './Button';
import { ErrorBox } from './ErrorBox';
import { PhotoRow } from './PhotoRow';
import { StatusTimeline } from './StatusTimeline';

describe('Button', () => {
  it('is an accessible button that fires onPress', async () => {
    const onPress = jest.fn();
    await render(<Button label="Go" testID="b" onPress={onPress} />);
    const b = screen.getByRole('button', { name: 'Go' });
    expect(b).toBeEnabled();
    await fireEvent.press(screen.getByTestId('b'));
    expect(onPress).toHaveBeenCalledTimes(1);
  });

  it('accessibilityLabel overrides the visible label', async () => {
    await render(
      <Button
        label="+"
        accessibilityLabel="More"
        testID="b"
        onPress={() => {}}
      />,
    );
    expect(screen.getByRole('button', { name: 'More' })).toBeOnTheScreen();
  });

  it.each(['primary', 'secondary', 'danger'] as const)(
    'renders the %s variant',
    async (variant) => {
      await render(
        <Button label="x" variant={variant} testID="b" onPress={() => {}} />,
      );
      expect(screen.getByTestId('b')).toBeOnTheScreen();
    },
  );

  it('disabled and loading do not fire and expose state', async () => {
    const onPress = jest.fn();
    const { rerender } = await render(
      <Button label="Go" testID="b" disabled onPress={onPress} />,
    );
    await fireEvent.press(screen.getByTestId('b'));
    expect(screen.getByTestId('b')).toBeDisabled();
    await rerender(<Button label="Go" testID="b" loading onPress={onPress} />);
    await fireEvent.press(screen.getByTestId('b'));
    expect(onPress).not.toHaveBeenCalled();
    expect(screen.getByTestId('b')).toBeBusy();
    expect(screen.queryByText('Go')).toBeNull();
  });

  it('loading indicator on a primary and a non-primary button', async () => {
    const { rerender } = await render(
      <Button
        label="Go"
        variant="secondary"
        loading
        testID="b"
        onPress={() => {}}
      />,
    );
    expect(screen.getByTestId('b')).toBeBusy();
    await rerender(<Button label="Go" loading testID="b" onPress={() => {}} />);
    expect(screen.getByTestId('b')).toBeBusy();
  });
});

describe('ErrorBox', () => {
  it('shows the localized message as an alert, without retry by default', async () => {
    await renderApp(
      <ErrorBox error={new ApiError('conflict', 'x')} testID="e" />,
    );
    expect(screen.getByTestId('e')).toHaveTextContent(
      'Trạng thái đơn hàng đã thay đổi, vui lòng thử lại.',
    );
    expect(screen.getByTestId('e').props.accessibilityRole).toBe('alert');
    expect(screen.queryByTestId('e-retry')).toBeNull();
  });

  it('retry button with default and custom ids/labels', async () => {
    const onRetry = jest.fn();
    const { rerender } = await renderApp(
      <ErrorBox error={new Error('x')} testID="e" onRetry={onRetry} />,
    );
    await fireEvent.press(screen.getByTestId('e-retry'));
    expect(onRetry).toHaveBeenCalledTimes(1);
    expect(screen.getByText('Thử lại')).toBeOnTheScreen();
    await rerender(
      <ErrorBox
        error={null}
        testID="e"
        onRetry={onRetry}
        retryLabel="Again"
        retryTestID="again"
      />,
    );
    expect(screen.getByTestId('again')).toBeOnTheScreen();
  });
});

describe('PhotoRow', () => {
  const setup = async (copies: number) => {
    const onCopies = jest.fn();
    const onRemove = jest.fn();
    await renderApp(
      <PhotoRow
        photo={draftPhoto(0, copies)}
        index={0}
        onCopies={onCopies}
        onRemove={onRemove}
      />,
    );
    return { onCopies, onRemove };
  };

  it('shows copies, steps by one, and removes', async () => {
    const { onCopies, onRemove } = await setup(2);
    expect(screen.getByTestId('picker-copies-0')).toHaveTextContent(
      'Số bản in: 2',
    );
    expect(screen.getByLabelText('Ảnh 1')).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole('button', { name: 'Thêm một bản' }));
    expect(onCopies).toHaveBeenLastCalledWith(3);
    await fireEvent.press(screen.getByRole('button', { name: 'Bớt một bản' }));
    expect(onCopies).toHaveBeenLastCalledWith(1);
    await fireEvent.press(screen.getByRole('button', { name: 'Xóa ảnh 1' }));
    expect(onRemove).toHaveBeenCalledTimes(1);
  });

  it('cannot go below 1', async () => {
    await setup(1);
    expect(screen.getByTestId('picker-copies-dec-0')).toBeDisabled();
    expect(screen.getByTestId('picker-copies-inc-0')).toBeEnabled();
  });

  it('cannot go above 5', async () => {
    await setup(5);
    expect(screen.getByTestId('picker-copies-inc-0')).toBeDisabled();
    expect(screen.getByTestId('picker-copies-dec-0')).toBeEnabled();
  });
});

describe('StatusTimeline', () => {
  it('lists the five steps, marking the current one', async () => {
    await renderApp(<StatusTimeline status="paid" />);
    expect(screen.getByTestId('order-step-paid')).toHaveTextContent(
      /^●\s*Đã thanh toán$/,
    );
    expect(screen.getByTestId('order-step-awaiting_payment')).toHaveTextContent(
      /^✓/,
    );
    expect(screen.getByTestId('order-step-printing')).toHaveTextContent(/^○/);
    expect(screen.getByTestId('order-step-completed')).toHaveTextContent(/^○/);
  });

  it('completed shows every step done', async () => {
    await renderApp(<StatusTimeline status="completed" />);
    expect(screen.getByTestId('order-step-completed')).toHaveTextContent(/^✓/);
  });
});
