/**
 * The console on a phone: a narrow touch screen. Runs after the desktop
 * flows (it needs the owner they made), and comes in the way a deployment
 * owner who lost every device would: `ordo console recover`.
 */

import { expect, test } from '@playwright/test';
import { fits, passkeyDevice, recoverCode, savedAndContinue, shownCodes, signOut, watch } from './support';

test('the console fits a phone: sign-in, a recover code, codes, Health, Runtime and Owners', async ({ page }) => {
  await passkeyDevice(page);
  const problems = watch(page);

  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
  await fits(page);
  await page.getByRole('button', { name: /Use a recovery code/ }).click();
  await fits(page);
  await page.getByRole('button', { name: 'Back to sign in' }).click();

  await page.getByRole('button', { name: 'Have an invite code?' }).click();
  await fits(page);
  await page.getByLabel('Code').fill(recoverCode());
  await page.getByRole('button', { name: 'Continue with a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'Your new passkey is ready' })).toBeVisible();
  await shownCodes(page);
  await fits(page);
  await savedAndContinue(page);
  await fits(page);

  await expect(page.getByRole('heading', { name: 'Health' })).toBeVisible();
  await expect(page.locator('.state [data-state]')).toBeVisible();
  await fits(page);

  // Runtime, with the revisions, settings and secret the runtime flows made.
  await page.getByRole('link', { name: 'Runtime' }).click();
  await expect(page.getByRole('heading', { name: 'Runtime', level: 1 })).toBeVisible();
  await expect(page.getByRole('list', { name: 'Revisions' }).getByRole('listitem').first()).toBeVisible();
  await page.getByText(/^Last log lines/).click();
  await page.getByLabel('Setting name', { exact: true }).fill('ORDO_A_RATHER_LONG_SETTING_NAME_FOR_A_NARROW_SCREEN');
  await page.getByLabel('Value', { exact: true }).fill('a-long-value-'.repeat(8));
  await expect(page.getByText(/^Add ORDO_A_RATHER_LONG/)).toBeVisible();
  await fits(page);
  await page.locator('li[data-revision="1"]').getByRole('button', { name: 'Roll back to this' }).click();
  await fits(page);
  await expect(page.getByRole('button', { name: 'Apply' })).toBeVisible();
  await page.getByRole('button', { name: 'Apply' }).scrollIntoViewIfNeeded();
  await expect(page.getByRole('button', { name: 'Apply' })).toBeInViewport({ ratio: 1 });

  await page.getByRole('link', { name: 'Owners' }).click();
  await expect(page.getByRole('heading', { name: 'Owners' })).toBeVisible();
  await fits(page);
  await expect(page.getByRole('button', { name: 'Make an invite code' })).toBeInViewport({ ratio: 1 });
  await expect(page.getByRole('button', { name: 'Sign out' })).toBeInViewport({ ratio: 1 });
  await signOut(page);

  expect(problems).toEqual([]);
});
