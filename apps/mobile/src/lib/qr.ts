const PRINTER_ID_FORBIDDEN = /[/+#]/;

/** Non-empty after trim, none of `/ + #` (they break the path, the topic, or the URL). */
export function isValidPrinterId(id: string): boolean {
  const t = id.trim();
  return (
    t.length > 0 &&
    !PRINTER_ID_FORBIDDEN.test(t) &&
    !/\s/.test(t) &&
    !t.includes('?')
  );
}

/**
 * Extracts the printer id from `https://<host>/p/<id>` or `fotoko://p/<id>`.
 * Returns null for anything else (other schemes, other paths, extra segments, bad ids).
 */
export function parsePrinterLink(raw: string): string | null {
  const text = raw.trim();
  const https = /^https:\/\/[^/?#\s]+\/p\/([^/?#]*)\/?(?:[?#].*)?$/i.exec(text);
  const app = /^fotoko:\/\/p\/([^/?#]*)\/?(?:[?#].*)?$/i.exec(text);
  const m = https ?? app;
  if (!m) return null;
  let id: string;
  try {
    id = decodeURIComponent(m[1] ?? '');
  } catch {
    return null;
  }
  return isValidPrinterId(id) ? id.trim() : null;
}
