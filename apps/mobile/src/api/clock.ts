/**
 * Server clock estimate from the HTTP `Date` header. Phones with a wrong clock would otherwise
 * SigV4-sign with a bad timestamp (API Gateway rejects skew over 5 min) and misjudge when a
 * presigned URL expires. On web the header is readable cross-origin only if the API sends
 * `Access-Control-Expose-Headers: Date` (see README); otherwise the offset stays 0.
 */
let offsetMs = 0;

export const serverClock = {
  /** Current time as the server sees it. */
  now(): number {
    return Date.now() + offsetMs;
  },
  offset(): number {
    return offsetMs;
  },
  /** Feed a `Date` response header; ignores missing or unparsable values. */
  observe(dateHeader: string | null | undefined): void {
    if (!dateHeader) return;
    const server = Date.parse(dateHeader);
    if (Number.isNaN(server)) return;
    offsetMs = server - Date.now();
  },
  reset(): void {
    offsetMs = 0;
  },
};
