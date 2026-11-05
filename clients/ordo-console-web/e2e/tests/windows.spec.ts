/**
 * The Maintenance windows screen and the Deploys screen's revoke in Chromium
 * (owner rulings 2026-11-04, docs/plan-versions.md 4, increment 2b): add a
 * one-off window and a repeating one, see the door's refusal as given, edit,
 * skip a range and remove the skip, delete with the in-page confirm; then
 * revoke a stored version with a reason and lift the mark. The door's ops
 * are `ordo-host`'s own over the stack's database
 * (crates/ordo-console/examples/e2e_door.rs).
 */

import { expect, test, type Page } from '@playwright/test';
import { fits, linkAsNamedOwner, person, recoverCode, savedAndContinue, shownCodes } from './support';

/** Take the touch at the deployment when it asks for one. */
async function touchIfAsked(page: Page): Promise<void> {
  const prompt = page.getByRole('dialog', { name: 'Touch your passkey for the deployment' });
  const use = prompt.getByRole('button', { name: 'Use my passkey' });
  try {
    await use.waitFor({ state: 'visible', timeout: 3000 });
  } catch {
    return;
  }
  await use.click();
  await expect(prompt).toBeHidden();
}

/** A `datetime-local` value `days` from now at `hour`, local time. */
function local(days: number, hour: number): string {
  const d = new Date();
  d.setDate(d.getDate() + days);
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(hour)}:00`;
}

test('add, refuse, edit, skip and delete maintenance windows; revoke a version and lift it', async ({ browser }) => {
  const { context, page, device, problems } = await person(browser);
  await page.goto('/');
  await page.getByRole('button', { name: 'Have an invite code?' }).click();
  await page.getByLabel('Code').fill(recoverCode());
  await page.getByRole('button', { name: 'Continue with a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'Your new passkey is ready' })).toBeVisible();
  await shownCodes(page);
  await savedAndContinue(page);
  await page.getByRole('link', { name: 'Deployment' }).click();
  await linkAsNamedOwner(page, device);

  await page.getByRole('link', { name: 'Maintenance' }).click();
  await expect(page.getByRole('heading', { name: 'Maintenance windows', level: 1 })).toBeVisible();
  await expect(page.getByText('No window is set.')).toBeVisible();

  // A one-off window. Its message is shown as written, never as markup.
  await page.getByLabel('Starts (your local time)').fill(local(1, 2));
  await page.getByLabel('Ends (your local time)').fill(local(1, 3));
  await page.getByLabel('Message').fill('Upgrade <b>not bold</b>');
  await page.getByRole('button', { name: 'Add window' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Added window');
  const upcoming = page.getByRole('list', { name: 'Upcoming windows' });
  await expect(upcoming.getByText('Upgrade <b>not bold</b>')).toBeVisible();

  // The door refuses an end before the start; its words show as given.
  await page.getByLabel('Starts (your local time)').fill(local(2, 5));
  await page.getByLabel('Ends (your local time)').fill(local(2, 4));
  await page.getByLabel('Message').fill('Backwards');
  await page.getByRole('button', { name: 'Add window' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('alert')).toContainText('ends after it starts');

  // A repeating window, then edited.
  await page.getByLabel('Repeats on a schedule').check();
  await page.getByLabel('Schedule (cron, 5 fields, UTC)').fill('0 3 * * 0');
  await page.getByLabel('Length (minutes)').fill('60');
  await page.getByLabel('Message').fill('Weekly');
  await page.getByRole('button', { name: 'Add window' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Added window');
  const windows = page.getByRole('list', { name: 'Windows', exact: true });
  await expect(windows.locator(':scope > li')).toHaveCount(2);
  const weekly = windows.getByRole('listitem').filter({ hasText: 'Repeats' });
  await weekly.getByRole('button', { name: 'Edit' }).click();
  await expect(page.getByRole('heading', { name: 'Edit window' })).toBeVisible();
  await page.getByLabel('Message').fill('Weekly, later');
  await page.getByRole('button', { name: 'Save window' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Saved window');
  await expect(weekly.getByText('Weekly, later')).toBeVisible();

  // Skip the next 8 days of it, then remove the skip.
  await weekly.getByRole('button', { name: 'Skip a range' }).click();
  await weekly.getByLabel('From', { exact: true }).fill(local(0, 0));
  await weekly.getByLabel('To', { exact: true }).fill(local(8, 0));
  await weekly.getByLabel('Reason (optional)').fill('Holiday');
  await page.getByRole('button', { name: 'Skip these times' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Skipping window');
  await expect(upcoming.getByText('skipped').first()).toBeVisible();
  await weekly.getByRole('button', { name: 'Remove skip' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Removed skip');

  // Times in UTC on request.
  await page.getByLabel(/Show times in UTC/).check();
  await expect(upcoming.getByText(/UTC/).first()).toBeVisible();

  // Delete both, each with the in-page confirm.
  for (let left = 2; left > 0; left--) {
    await windows.getByRole('button', { name: 'Delete' }).first().click();
    await page.getByRole('group', { name: /Confirm delete window/ }).getByRole('button', { name: 'Delete window' }).click();
    await touchIfAsked(page);
    await expect(page.getByRole('status')).toContainText('Deleted window');
  }
  await expect(page.getByText('No window is set.')).toBeVisible();

  // The screen fits a phone.
  await page.setViewportSize({ width: 390, height: 844 });
  await fits(page);
  await page.setViewportSize({ width: 1280, height: 800 });

  // Deploys: revoke the version that does not serve, with a reason, and lift it.
  await page.getByRole('link', { name: 'Deploys' }).click();
  const versions = page.getByRole('list', { name: 'Stored versions' });
  const second = versions.getByRole('listitem').filter({ hasText: '#2' });
  await expect(second).toBeVisible();
  await second.getByRole('button', { name: 'Revoke' }).click();
  await second.getByLabel('Reason').fill('bad build');
  await second.getByRole('button', { name: 'Revoke' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Revoked');
  await expect(second.getByText('revoked', { exact: true })).toBeVisible();
  await expect(second).toContainText('bad build');
  await second.getByRole('button', { name: 'Lift the revoke' }).click();
  await second.getByLabel('Reason').fill('fixed');
  await second.getByRole('button', { name: 'Lift the revoke' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Lifted the revoke');
  await expect(second.getByText('revoked', { exact: true })).toHaveCount(0);

  // Revoking the version that serves asks where to move.
  const serving = versions.getByRole('listitem').filter({ hasText: 'active (new tenants)' });
  await serving.getByRole('button', { name: 'Revoke' }).click();
  await expect(serving.getByLabel('It serves now. Move to')).toBeVisible();
  await serving.getByRole('button', { name: 'Cancel' }).click();
  await page.setViewportSize({ width: 390, height: 844 });
  await fits(page);

  // Unlink, so a later spec links the door with its own passkey.
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.getByRole('link', { name: 'Deployment' }).click();
  await page.getByRole('button', { name: 'Unlink' }).click();
  await page.getByRole('button', { name: 'Yes, unlink' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Unlinked.');

  expect(problems).toEqual([]);
  await context.close();
});
