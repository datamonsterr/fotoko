import path from 'node:path';

import { expect, test } from '@playwright/test';

import { printedPages, simEnv } from './sim';

const photo = path.join(import.meta.dirname, '..', 'fixtures', 'photo.jpg');

test('a customer scans, uploads, pays and the photo is printed', async ({
  page,
}) => {
  const payos = simEnv('FOTOKO_E2E_PAYOS_URL');
  const outDir = simEnv('FOTOKO_E2E_SIM_OUT_STAND_001');

  // The QR code opens the stand page.
  await page.goto('/p/stand-001');
  await expect(page.getByTestId('printer-name')).toBeVisible();
  await expect(page.getByTestId('printer-price')).toContainText(/10[.,]000/);
  await expect(page.getByTestId('printer-status')).toHaveText('Ready');
  await page.getByTestId('printer-pick-button').click();

  // expo-image-picker opens a hidden <input type="file"> on the web.
  const chooser = page.waitForEvent('filechooser');
  await page.getByTestId('picker-add').click();
  await (await chooser).setFiles(photo);
  await expect(page.getByTestId('picker-photo-0')).toBeVisible();
  await expect(page.getByTestId('picker-total')).toContainText(/10[.,]000/);

  // Two copies of the one photo.
  await page.getByTestId('picker-copies-inc-0').click();
  await expect(page.getByTestId('picker-copies-0')).toContainText('2');
  await expect(page.getByTestId('picker-total')).toContainText(/20[.,]000/);

  // Upload and checkout lead to the (mock) payOS page in the same tab.
  await page.getByTestId('picker-continue').click();
  await page.waitForURL(new RegExp(`^${payos}/web/\\d+$`));
  await expect(page.locator('#payos-amount')).toContainText('20000');
  await page.getByRole('button', { name: 'Pay' }).click();

  // payOS sends the customer back to the order, which turns completed once the Pi printed.
  await page.waitForURL(/\/orders\/[^/?]+/);
  const orderId = /\/orders\/([^/?]+)/.exec(page.url())?.[1];
  expect(orderId).toBeTruthy();
  await expect(page.getByTestId('order-done')).toBeVisible({ timeout: 30_000 });

  // The simulated printer wrote both copies, fitted to a landscape 6x4 inch page.
  const pages = printedPages(outDir, orderId ?? '');
  expect(pages).toHaveLength(2);
  for (const printed of pages) {
    expect({ width: printed.width, height: printed.height }).toEqual({
      width: 1800,
      height: 1200,
    });
  }
});
