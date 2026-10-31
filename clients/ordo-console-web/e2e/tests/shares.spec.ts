/**
 * The Shares and security screen in Chromium (docs/plan-console.md 4.7 and
 * 4.8, step 9): a tenant's links with their state, opens, devices and lock,
 * a link revoked with the owner's touch at the deployment relayed by the
 * console, and the security log from the deployment, the tenant and the
 * console. The door's answers are stand-ins over memory
 * (crates/ordo-console/tests/support/shares_door.rs).
 */

import { expect, test } from '@playwright/test';
import { fits, person, recoverCode, savedAndContinue, shownCodes, linkAsNamedOwner } from './support';

test('see a tenant links, revoke one, read the security log', async ({ browser }) => {
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

  // The deployment's own record shows on open, newest first.
  await page.getByRole('link', { name: 'Shares' }).click();
  await expect(page.getByRole('heading', { name: 'Shares and security', level: 1 })).toBeVisible();
  await page.getByRole('button', { name: 'Security log' }).click();
  const record = page.getByRole('region', { name: "The deployment's security record" });
  await expect(record.getByRole('row')).toHaveCount(6);
  await expect(record.getByRole('row').nth(1)).toContainText('quota-set');
  await page.getByLabel('Kind', { exact: true }).selectOption('deploy');
  await page.getByRole('button', { name: 'Show' }).click();
  await expect(record.getByRole('row')).toHaveCount(2);
  await expect(record.getByRole('row').nth(1)).toContainText('applied');

  // Links in a tenant the owner owns: a touch at the deployment first.
  await page.getByRole('button', { name: 'Links and shares' }).click();
  await page.getByLabel('Tenant', { exact: true }).selectOption({ label: 'Acme (spc_mine), you: owner' });
  await page.getByRole('button', { name: 'Show links and shares' }).click();
  const prompt = page.getByRole('dialog', { name: 'Touch your passkey for the deployment' });
  await expect(prompt).toBeVisible();
  await prompt.getByRole('button', { name: 'Use my passkey' }).click();
  const links = page.getByRole('region', { name: /Share links in spc_mine/ });
  const live = links.getByRole('row', { name: /Board for Ana/ });
  await expect(live).toContainText('Live');
  await expect(live).toContainText('Locked until');
  await expect(live.getByRole('cell').nth(2)).toHaveText('3');
  await expect(links.getByRole('row', { name: /One look/ })).toContainText('Used up');
  await expect(page.getByRole('region', { name: /Live shares in spc_mine/ })).toContainText('Gil');

  // Revoke it, inside the same touch.
  await page.getByRole('button', { name: 'Revoke Board for Ana' }).click();
  const confirm = page.getByRole('dialog', { name: 'Revoke this link?' });
  await confirm.getByRole('button', { name: 'Revoke' }).click();
  await expect(page.getByRole('status')).toContainText('The link "Board for Ana" is revoked.');
  await expect(live).toContainText('Revoked');
  await expect(page.getByRole('button', { name: 'Revoke Board for Ana' })).toHaveCount(0);

  // The tenant's own record now carries the revocation.
  await page.getByRole('button', { name: 'Security log' }).click();
  await page.getByLabel('Record', { exact: true }).selectOption({ label: 'One tenant' });
  await page.getByLabel('Tenant', { exact: true }).selectOption({ label: 'Acme (spc_mine), you: owner' });
  await page.getByRole('button', { name: 'Show' }).click();
  const theirs = page.getByRole('region', { name: "The tenant's security record" });
  await expect(theirs.getByRole('row', { name: /enrolment/ })).toBeVisible();

  // The console's own audit log names the revoke.
  await page.getByLabel('Record', { exact: true }).selectOption({ label: 'This console' });
  await page.getByLabel('Action starts with').fill('shares.');
  await page.getByRole('button', { name: 'Show' }).click();
  const audit = page.getByRole('region', { name: "This console's audit log" });
  await expect(audit.getByRole('row', { name: /shares\.revoke_link/ })).toContainText('ok');

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
