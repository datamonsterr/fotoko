import { act, renderHook, waitFor } from '@testing-library/react-native';

import { ApiError, setApiClient, type ApiClient } from '@/api';
import { printer } from '@/test/fixtures';

import { usePrinter } from './usePrinter';

describe('usePrinter', () => {
  it('loads then returns the printer', async () => {
    let resolve: (p: typeof printer) => void = () => {};
    const getPrinter = jest.fn(
      () => new Promise<typeof printer>((r) => (resolve = r)),
    );
    setApiClient({ getPrinter } as unknown as ApiClient);
    const { result } = await renderHook(() => usePrinter('stand-001'));
    expect(result.current.state.phase).toBe('loading');
    await act(async () => resolve(printer));
    await waitFor(() =>
      expect(result.current.state).toEqual({ phase: 'ready', printer }),
    );
    expect(getPrinter).toHaveBeenCalledWith(
      'stand-001',
      expect.any(AbortSignal),
    );
  });

  it('exposes errors and reload refetches (showing loading meanwhile)', async () => {
    const err = new ApiError('network', 'down');
    let resolve: (p: typeof printer) => void = () => {};
    const getPrinter = jest
      .fn()
      .mockRejectedValueOnce(err)
      .mockImplementationOnce(() => new Promise((r) => (resolve = r)));
    setApiClient({ getPrinter } as unknown as ApiClient);
    const { result } = await renderHook(() => usePrinter('p'));
    await waitFor(() =>
      expect(result.current.state).toEqual({ phase: 'error', error: err }),
    );
    await act(async () => result.current.reload());
    expect(result.current.state.phase).toBe('loading');
    await act(async () => resolve(printer));
    await waitFor(() => expect(result.current.state.phase).toBe('ready'));
    expect(getPrinter).toHaveBeenCalledTimes(2);
  });

  it('refetches when the id changes and ignores aborted responses', async () => {
    let resolveFirst: (p: typeof printer) => void = () => {};
    const getPrinter = jest
      .fn()
      .mockImplementationOnce(() => new Promise((r) => (resolveFirst = r)))
      .mockResolvedValueOnce({ ...printer, printerId: 'b' });
    setApiClient({ getPrinter } as unknown as ApiClient);
    const { result, rerender } = await renderHook(
      ({ id }: { id: string }) => usePrinter(id),
      { initialProps: { id: 'a' } },
    );
    await rerender({ id: 'b' });
    await waitFor(() =>
      expect(result.current.state).toMatchObject({
        phase: 'ready',
        printer: { printerId: 'b' },
      }),
    );
    await act(async () => resolveFirst({ ...printer, printerId: 'a' }));
    expect(result.current.state).toMatchObject({ printer: { printerId: 'b' } });
  });

  it('ignores an aborted failure', async () => {
    let fail: (e: unknown) => void = () => {};
    const getPrinter = jest.fn(() => new Promise((_, rej) => (fail = rej)));
    setApiClient({ getPrinter } as unknown as ApiClient);
    const { result, unmount } = await renderHook(() => usePrinter('a'));
    await unmount();
    await act(async () => fail(new Error('late')));
    expect(result.current.state.phase).toBe('loading');
  });

  it('uses the app client by default', async () => {
    const getPrinter = jest.fn().mockResolvedValue(printer);
    setApiClient({ getPrinter } as unknown as ApiClient);
    const { result } = await renderHook(() => usePrinter('a'));
    await waitFor(() => expect(result.current.state.phase).toBe('ready'));
    setApiClient(undefined);
  });
});
