import { KNOWN_ERROR_CODES, type KnownErrorCode } from './types';

/** Codes with no server response behind them. */
export type ClientErrorCode =
  | 'network'
  | 'invalid_response'
  | 'upload_failed'
  | 'auth'
  | 'aborted'
  | 'expired'
  | 'unknown';

export class ApiError extends Error {
  /** Server code (snake_case, may be one this app does not know) or a client code. */
  readonly code: string;
  readonly status: number;

  constructor(code: string, message: string, status = 0) {
    super(message);
    this.name = 'ApiError';
    this.code = code;
    this.status = status;
  }

  get isKnown(): boolean {
    return (
      (KNOWN_ERROR_CODES as readonly string[]).includes(this.code) ||
      this.code === 'network' ||
      this.code === 'invalid_response' ||
      this.code === 'upload_failed' ||
      this.code === 'auth' ||
      this.code === 'expired'
    );
  }

  is(code: KnownErrorCode | ClientErrorCode): boolean {
    return this.code === code;
  }
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null;
}

/** Turns a non-2xx response body into an ApiError; tolerates non-JSON and odd shapes. */
export function parseErrorBody(status: number, text: string): ApiError {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    parsed = undefined;
  }
  const err = isRecord(parsed) ? parsed.error : undefined;
  if (isRecord(err) && typeof err.code === 'string' && err.code !== '') {
    return new ApiError(
      err.code,
      typeof err.message === 'string' ? err.message : '',
      status,
    );
  }
  // API Gateway's own IAM rejections are `{"message": "..."}`: no contract error code.
  if (status === 401 || status === 403) {
    const message =
      isRecord(parsed) && typeof parsed.message === 'string'
        ? parsed.message
        : text;
    return new ApiError('auth', message.slice(0, 200), status);
  }
  return new ApiError('unknown', text.slice(0, 200), status);
}
