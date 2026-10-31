/**
 * The Usage screen in Chromium (docs/plan-console.md 4.9 and 4.11, step 7):
 * usage for the whole deployment and for one tenant, a quota changed with
 * the owner's touch at the deployment relayed by the console, and the slow
 * ops table sorted. The door's answers are stand-ins over memory
 * (crates/ordo-console/tests/support/usage_door.rs).
 */

import { expect, test } from '@playwright/test';
import { fits, person, recoverCode, savedAndContinue, shownCodes, linkAsNamedOwner } from './support';

test('view usage, edit a quota, sort the slow ops', async ({ browser }) => {
  const { context, page, device, problems } = await person(browser);
  await page.goto('/');
  await page.getByRole('button', { name: 'Have an invite code?' }).click();
  await page.getByLabel('Code').fill(recoverCode());
  await page.getByRole('button', { name: 'Continue with a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'Your new passkey is ready' })).toBeVisible();
  await shownCodes(page);
  await savedAndContinue(page);

  // Link this console to the deployment with a passkey there.
  await page.getByRole('link', { name: 'Deployment' }).click();
  await linkAsNamedOwner(page, device);

  // The whole deployment: 6 periods of 10, 5 and 1 requests.
  await page.getByRole('link', { name: 'Usage' }).click();
  await expect(page.getByRole('heading', { name: 'Usage', level: 1 })).toBeVisible();
  await expect(page.getByRole('img', { name: /Requests per period, 6 periods/ })).toBeVisible();
  const by = page.getByRole('region', { name: 'Usage by tenant' });
  await expect(by.getByRole('row', { name: /Acme/ })).toContainText('60');
  await expect(by.getByRole('row', { name: /Globex/ })).toContainText('30');
  await expect(page.getByRole('note').filter({ hasText: 'not counted' })).toContainText('Held Ltd');

  // One tenant.
  await page.getByLabel('Whose').selectOption({ label: 'Globex' });
  await expect(page.getByRole('img', { name: /busiest had 5,/ })).toBeVisible();
  await expect(by).toHaveCount(0);
  await page.getByLabel('Range').selectOption({ label: 'Last 7 days' });
  await expect(page.getByRole('img', { name: /6 periods/ })).toBeVisible();

  // A quota: edit Acme's, which asks for the owner's touch at the deployment.
  const quotas = page.getByRole('region', { name: 'Quotas' });
  await expect(quotas.getByRole('row', { name: /Acme/ })).toContainText('100');
  await page.getByRole('button', { name: 'Edit the requests quota of Acme' }).click();
  const edit = page.getByRole('dialog', { name: 'Quota for Acme' });
  await edit.getByLabel('Limit').fill('250');
  await edit.getByLabel('Per').selectOption({ label: 'A day' });
  await edit.getByRole('button', { name: 'Save quota' }).click();
  const prompt = page.getByRole('dialog', { name: 'Touch your passkey for the deployment' });
  await expect(prompt).toBeVisible();
  await prompt.getByRole('button', { name: 'Use my passkey' }).click();
  await expect(page.getByRole('status')).toContainText('Saved: Acme, requests up to 250 in a day.');
  const acme = quotas.getByRole('row', { name: /Acme/ });
  await expect(acme).toContainText('250');
  await expect(acme).toContainText('a day');

  // A new one, still inside the same touch.
  await page.getByRole('button', { name: 'Set a quota' }).click();
  const fresh = page.getByRole('dialog', { name: 'Set a quota' });
  await fresh.getByLabel('Tenant').selectOption({ label: 'Globex' });
  await fresh.getByLabel('Counts').selectOption({ label: 'Rows returned' });
  await fresh.getByLabel('Limit').fill('9000');
  await fresh.getByRole('button', { name: 'Save quota' }).click();
  await expect(page.getByRole('status')).toContainText('Saved: Globex, rows returned up to 9,000');
  await expect(quotas.getByRole('row', { name: /Globex/ })).toContainText('Rows returned');

  // Slow ops: slowest mean first, then sorted by calls.
  const slow = page.getByRole('region', { name: 'Slow ops' });
  await expect(page.getByText(/From one node, node-a, since it started/)).toBeVisible();
  const first = slow.getByRole('row').nth(1);
  await expect(first).toContainText('searchNotes');
  await expect(first).toContainText('bounded-host');
  await expect(first).toContainText('200 ms');
  await slow.getByRole('button', { name: 'Calls' }).click();
  await expect(slow.getByRole('columnheader', { name: 'Calls' })).toHaveAttribute('aria-sort', 'descending');
  await expect(slow.getByRole('row').nth(1)).toContainText('getProfile');
  await slow.getByRole('button', { name: 'Calls' }).click();
  await expect(slow.getByRole('row').nth(1)).toContainText('searchNotes');
  await expect(slow.getByRole('row', { name: /listTasks/ })).toContainText('6 (5.0%)');

  // The screen fits a narrow window: one card a row.
  await page.setViewportSize({ width: 390, height: 844 });
  await fits(page);

  // Unlink, so a later spec links the door with its own passkey.
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.getByRole('link', { name: 'Deployment' }).click();
  await page.getByRole('button', { name: 'Unlink' }).click();
  await page.getByRole('button', { name: 'Yes, unlink' }).click();
  await expect(page.getByRole('status')).toContainText('Unlinked.');

  expect(problems).toEqual([]);
  await context.close();
});
