import { defineConfig } from '@playwright/test';

// scripts/e2e/sim-up.sh starts the stack and `run.sh` exports its addresses (env.sh).
const baseURL = process.env.FOTOKO_E2E_WEB_URL ?? 'http://127.0.0.1:18081';

export default defineConfig({
  testDir: 'tests',
  outputDir: 'test-results',
  // One stack, one browser: the tests share the printers' output directories.
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  // No retries: a flaky stack should fail loudly; the report and sim logs show why.
  retries: 0,
  timeout: 90_000,
  expect: { timeout: 15_000 },
  reporter: [
    ['list'],
    ['html', { outputFolder: 'playwright-report', open: 'never' }],
  ],
  use: {
    baseURL,
    locale: 'en-US',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    // CI installs the matching browser with `playwright install --with-deps chromium`. Locally
    // FOTOKO_E2E_CHROMIUM may point at a Chromium whose revision differs from Playwright's.
    launchOptions: {
      executablePath: process.env.FOTOKO_E2E_CHROMIUM || undefined,
    },
  },
  projects: [{ name: 'chromium', use: { browserName: 'chromium' } }],
});
