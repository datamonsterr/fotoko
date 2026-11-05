import { clampCopies, formatVnd, totalCopies, totalPriceVnd } from './price';

describe('clampCopies', () => {
  it.each([
    [0, 1],
    [-3, 1],
    [1, 1],
    [3, 3],
    [5, 5],
    [6, 5],
    [2.9, 2],
    [NaN, 1],
    [Infinity, 1],
  ])('%p -> %p', (input, expected) => {
    expect(clampCopies(input)).toBe(expected);
  });
});

describe('price', () => {
  it('sums copies', () => {
    expect(totalCopies([])).toBe(0);
    expect(totalCopies([{ copies: 2 }, { copies: 5 }])).toBe(7);
  });

  it('is sum(copies) * price', () => {
    expect(totalPriceVnd([], 10000)).toBe(0);
    expect(totalPriceVnd([{ copies: 2 }, { copies: 3 }], 10000)).toBe(50000);
  });
});

describe('formatVnd', () => {
  it.each([
    [0, '0 ₫'],
    [999, '999 ₫'],
    [1000, '1.000 ₫'],
    [10000, '10.000 ₫'],
    [1234567, '1.234.567 ₫'],
    [-2500, '-2.500 ₫'],
    [1499.6, '1.500 ₫'],
  ])('%p -> %p', (n, s) => {
    expect(formatVnd(n)).toBe(s);
  });
});
