import { MAX_COPIES, MIN_COPIES } from './limits';

export function clampCopies(n: number): number {
  if (!Number.isFinite(n)) return MIN_COPIES;
  return Math.min(MAX_COPIES, Math.max(MIN_COPIES, Math.trunc(n)));
}

export function totalCopies(photos: readonly { copies: number }[]): number {
  return photos.reduce((sum, p) => sum + p.copies, 0);
}

/** amountVnd = sum(copies) * pricePerPrintVnd (docs/contracts.md). */
export function totalPriceVnd(
  photos: readonly { copies: number }[],
  pricePerPrintVnd: number,
): number {
  return totalCopies(photos) * pricePerPrintVnd;
}

/** `10.000 ₫` style, dot thousands separator, independent of the runtime's ICU data. */
export function formatVnd(amount: number): string {
  const rounded = Math.round(amount);
  const sign = rounded < 0 ? '-' : '';
  const digits = String(Math.abs(rounded)).replace(
    /\B(?=(\d{3})+(?!\d))/g,
    '.',
  );
  return `${sign}${digits} ₫`;
}
