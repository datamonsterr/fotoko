import {
  createContext,
  useContext,
  useMemo,
  useReducer,
  type ReactNode,
} from 'react';

import { MAX_PHOTOS } from '@/lib/limits';
import { clampCopies } from '@/lib/price';
import type { DraftPhoto } from '@/lib/photos';

export type DraftAction =
  | { type: 'add'; photos: DraftPhoto[] }
  | { type: 'remove'; id: string }
  | { type: 'setCopies'; id: string; copies: number }
  | { type: 'clear' };

export function draftReducer(
  state: DraftPhoto[],
  action: DraftAction,
): DraftPhoto[] {
  switch (action.type) {
    case 'add':
      return [...state, ...action.photos].slice(0, MAX_PHOTOS);
    case 'remove':
      return state.filter((p) => p.id !== action.id);
    case 'setCopies':
      return state.map((p) =>
        p.id === action.id ? { ...p, copies: clampCopies(action.copies) } : p,
      );
    case 'clear':
      return [];
  }
}

interface DraftValue {
  photos: DraftPhoto[];
  dispatch: (a: DraftAction) => void;
}

const DraftContext = createContext<DraftValue | null>(null);

export function PhotoDraftProvider({
  children,
  initial = [],
}: {
  children: ReactNode;
  initial?: DraftPhoto[];
}) {
  const [photos, dispatch] = useReducer(draftReducer, initial);
  const value = useMemo(() => ({ photos, dispatch }), [photos]);
  return (
    <DraftContext.Provider value={value}>{children}</DraftContext.Provider>
  );
}

export function usePhotoDraft(): DraftValue {
  const v = useContext(DraftContext);
  if (!v) throw new Error('usePhotoDraft needs <PhotoDraftProvider>');
  return v;
}
