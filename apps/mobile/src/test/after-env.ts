/* eslint-disable @typescript-eslint/no-require-imports -- loaded lazily so a test file's own jest.mock calls still apply */

// No test inherits the previous one's API client or server-clock offset. Required lazily: a
// top-level import here would load '@/api' before the test file's jest.mock registrations.
afterEach(() => {
  const api = require('@/api') as typeof import('@/api');
  api.setApiClient(undefined);
  api.serverClock.reset();
});
