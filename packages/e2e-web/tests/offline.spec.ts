import { readFileSync } from 'node:fs';

import { expect, test } from '@playwright/test';

import { simEnv } from './sim';

test('a printer whose Pi stopped shows as offline and cannot be used', async ({
  page,
}) => {
  const pidFile = simEnv('FOTOKO_E2E_PI_PID_STAND_002');

  // stand-002 starts online.
  await page.goto('/p/stand-002');
  await expect(page.getByTestId('printer-status')).toHaveText('Ready');
  await expect(page.getByTestId('printer-pick-button')).toBeVisible();

  // systemd stops the agent: it announces `offline` and the API marks the printer.
  process.kill(Number(readFileSync(pidFile, 'utf8').trim()), 'SIGTERM');

  await expect(async () => {
    await page.reload();
    await expect(page.getByTestId('printer-status')).toHaveText(
      'The printer is offline',
      {
        timeout: 3_000,
      },
    );
  }).toPass({ timeout: 20_000 });
  await expect(page.getByTestId('printer-pick-button')).toBeHidden();
  await expect(page.getByTestId('printer-retry')).toBeVisible();
});
