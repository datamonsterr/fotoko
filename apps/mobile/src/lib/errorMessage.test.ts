import { ApiError } from '@/api';

import { errorMessageKey } from './errorMessage';

describe('errorMessageKey', () => {
  it.each([
    'not_found',
    'validation',
    'forbidden',
    'conflict',
    'payment_provider',
    'internal',
    'network',
    'invalid_response',
    'upload_failed',
    'auth',
    'expired',
  ])('maps %s', (code) => {
    expect(errorMessageKey(new ApiError(code, 'm'))).toBe(`error.${code}`);
  });

  it('falls back for unknown server codes and non-API errors', () => {
    expect(errorMessageKey(new ApiError('brand_new_code', 'm'))).toBe(
      'error.unknown',
    );
    expect(errorMessageKey(new ApiError('unknown', 'm'))).toBe('error.unknown');
    expect(errorMessageKey(new Error('x'))).toBe('error.unknown');
    expect(errorMessageKey(null)).toBe('error.unknown');
  });
});
