import { ApiError } from '@/api';
import type { MessageKey } from '@/i18n';

/** Maps any thrown value to a localized message key; unknown server codes become `error.unknown`. */
export function errorMessageKey(e: unknown): MessageKey {
  if (e instanceof ApiError && e.isKnown)
    return `error.${e.code}` as MessageKey;
  return 'error.unknown';
}
