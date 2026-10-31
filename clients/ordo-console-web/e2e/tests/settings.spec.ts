/**
 * The settings screens in Chromium (docs/plan-console.md 4.6 and 4.10,
 * step 6): each setting as the running deployment took it, its source and
 * default, and a change through a runtime revision with drift shown after.
 *
 * The door's std.deployment.settings is ordo-host's own answer over the
 * environment the stand-in host started with
 * (crates/ordo-console/tests/support/settings_door.rs). The stack stores
 * guest sign-in `off`, as std.deployment.setGuest would, so a revision that
 * sets ORDO_AUTH_GUEST=on is not what the deployment does: drift.
 */

import { expect, test } from '@playwright/test';
import { fits, person, recoverCode, savedAndContinue, shownCodes, stack, linkAsNamedOwner } from './support';

test('view the settings, change one through a revision, see the drift', async ({ browser }) => {
  const { context, page, device, problems } = await person(browser);
  await page.goto('/');
  await page.getByRole('button', { name: 'Have an invite code?' }).click();
  await page.getByLabel('Code').fill(recoverCode());
  await page.getByRole('button', { name: 'Continue with a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'Your new passkey is ready' })).toBeVisible();
  await shownCodes(page);
  await savedAndContinue(page);

  // Link this console to the deployment with a passkey there; `tenants`
  // unlinked when it finished.
  await page.getByRole('link', { name: 'Deployment' }).click();
  await linkAsNamedOwner(page, device);

  // Sign-in settings: the stored guest setting, with who stored it.
  await page.getByRole('link', { name: 'Sign-in', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Sign-in settings', level: 1 })).toBeVisible();
  const guest = page.locator('tr[data-setting="ORDO_AUTH_GUEST"]');
  await expect(guest).toContainText('off');
  await expect(guest).toContainText('Stored by std.deployment.setGuest');
  await expect(guest).toContainText('Stored by an op');
  const digits = page.locator('tr[data-setting="ORDO_AUTH_EMAIL_CODE_DIGITS"]');
  await expect(digits).toContainText('Default');
  await expect(digits).toContainText('Takes 6 to 10');

  // Every setting: where the deployment listens, from its environment.
  await page.getByRole('link', { name: 'Settings', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Every setting', level: 1 })).toBeVisible();
  await page.getByLabel('Find a setting').fill('ORDO_HOST_ADDR');
  const addr = page.locator('tr[data-setting="ORDO_HOST_ADDR"]');
  await expect(addr).toContainText(new URL(stack().host).host);
  await expect(addr).toContainText('Environment');
  await expect(addr).not.toHaveAttribute('data-drift');

  // Change guest sign-in through a new revision. The deployment restarts
  // with it, and the stored value still wins: drift.
  await page.getByLabel('Find a setting').fill('ORDO_AUTH_GUEST');
  await page.getByRole('button', { name: 'Change ORDO_AUTH_GUEST', exact: true }).click();
  const form = page.getByRole('form', { name: 'Change ORDO_AUTH_GUEST', exact: true });
  await form.getByLabel('New value for ORDO_AUTH_GUEST').fill('on');
  await form.getByLabel(/Why/).fill('e2e: guests');
  await expect(form).toContainText('set ORDO_AUTH_GUEST to "on"');
  await expect(form).toContainText('only seeds it on a first boot');
  await form.getByRole('button', { name: 'Apply and restart' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'is live and ready' })).toContainText(
    'with the new ORDO_AUTH_GUEST',
    { timeout: 30_000 },
  );
  await expect(guest).toHaveAttribute('data-drift', 'stored');
  await expect(guest).toContainText('Stored value wins');
  await expect(guest).toContainText('on');
  await expect(page.getByText(/1 setting differs/)).toBeVisible();

  // The screen fits a narrow window: one card a setting.
  await page.setViewportSize({ width: 390, height: 844 });
  await fits(page);

  // Unlink, so `usage` links the door with its own passkey.
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.getByRole('link', { name: 'Deployment' }).click();
  await page.getByRole('button', { name: 'Unlink' }).click();
  await page.getByRole('button', { name: 'Yes, unlink' }).click();
  // Nothing here wrote at the deployment yet, so unlinking asks for the
  // owner's touch there.
  await page
    .getByRole('dialog', { name: 'Touch your passkey for the deployment' })
    .getByRole('button', { name: 'Use my passkey' })
    .click();
  await expect(page.getByRole('status')).toContainText('Unlinked.');

  expect(problems).toEqual([]);
  await context.close();
});
