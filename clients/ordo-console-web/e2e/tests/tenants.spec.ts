/**
 * The Tenants screen in Chromium (docs/plan-console.md 4.4, step 5): the
 * list with status, usage and a tenant held closed, create, pause and
 * resume, each change with the owner's touch at the deployment relayed by
 * the console; and owner succession: the open cases with their objections,
 * a succession opened from a typed id, an objection resolved, a matured case
 * completed. The door's tenant ops are a stand-in over memory
 * (crates/ordo-console/tests/support/tenants_door.rs).
 */

import { expect, test } from '@playwright/test';
import { fits, person, recoverCode, savedAndContinue, shownCodes, linkAsNamedOwner } from './support';

test('list tenants, create one, pause and resume', async ({ browser }) => {
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

  // The list.
  await page.getByRole('link', { name: 'Tenants' }).click();
  await expect(page.getByRole('heading', { name: 'Tenants', level: 1 })).toBeVisible();
  const table = page.getByRole('region', { name: 'Tenants, newest first' });
  const acme = table.getByRole('row', { name: /Acme/ });
  await expect(acme).toContainText('Active');
  await expect(acme).toContainText('42 writes');
  await expect(table.getByRole('row', { name: /Half built/ })).toContainText('Being set up');
  await expect(table.getByRole('row', { name: /Held Ltd/ })).toContainText('Held closed: the chain moved backwards');
  await expect(page.getByRole('alert')).toContainText('1 tenant is held closed');

  // Create: the deployment asks for the owner's touch there.
  await page.getByLabel('Name', { exact: true }).fill('New Co');
  await page.getByRole('button', { name: 'Create tenant' }).click();
  const prompt = page.getByRole('dialog', { name: 'Touch your passkey for the deployment' });
  await expect(prompt).toBeVisible();
  await prompt.getByRole('button', { name: 'Use my passkey' }).click();
  await expect(page.getByRole('status')).toContainText('Created New Co.');
  await expect(table.getByRole('row', { name: /New Co/ })).toContainText('you: owner');

  // Pause with a reason, then resume. The touch still covers both.
  await page.getByRole('button', { name: 'Pause Acme' }).click();
  const pause = page.getByRole('dialog', { name: 'Pause Acme' });
  await pause.getByLabel('Reason').selectOption('billing');
  await pause.getByRole('button', { name: 'Pause tenant' }).click();
  await expect(page.getByRole('status')).toContainText('Paused Acme.');
  await expect(acme).toContainText('Paused');
  await expect(acme).toContainText('billing');
  await page.getByRole('button', { name: 'Resume Acme' }).click();
  await expect(page.getByRole('status')).toContainText('Resumed Acme.');
  await expect(acme).toContainText('Active');

  // Owner succession: the open cases, with a paused one and its objection.
  const cases = page.getByRole('list', { name: 'Owner succession' });
  const matured = cases.getByRole('listitem').filter({ hasText: 'rec_matured' }).first();
  const paused = cases.getByRole('listitem').filter({ hasText: 'rec_paused' }).first();
  await expect(matured).toContainText('Ready to complete');
  await expect(paused).toContainText('Paused by an objection');
  await expect(paused).toContainText('waiting for your decision');

  // Open one from a typed id: no list of the tenant's members is shown.
  await page.getByLabel('Tenant id').fill('spc_theirs');
  await page.getByLabel("Successor's id").fill('prn_new');
  await page.getByLabel('Who has left').fill('prn_gone');
  await page.getByLabel('Evidence').selectOption('contract-holder');
  await page.getByLabel('Reference (optional)').fill('MSA-3');
  await page.getByRole('button', { name: 'Open succession' }).click();
  await expect(page.getByRole('status')).toContainText('Opened succession rec_new');
  await expect(cases.getByRole('listitem').filter({ hasText: 'rec_new' }).first()).toContainText('Matures');
  await expect(cases.getByRole('listitem').filter({ hasText: 'rec_new' }).first()).toContainText('Has left: prn_gone');

  // Resolve the objection: resume, with a reason from the list.
  await page.getByRole('button', { name: 'Resolve the objection to rec_paused' }).click();
  const resolveDialog = page.getByRole('dialog', { name: 'Resolve the objection to rec_paused' });
  await resolveDialog.getByLabel('Resume').check();
  await resolveDialog.getByLabel('Reason').selectOption('evidence-rechecked');
  await resolveDialog.getByRole('button', { name: 'Resume succession' }).click();
  await expect(page.getByRole('status')).toContainText('Resumed rec_paused.');
  await expect(paused).toContainText('resolved, resume (evidence-rechecked)');

  // Complete the matured one; the departed owner's grants end by default.
  await page.getByRole('button', { name: 'Complete rec_matured' }).click();
  const completeDialog = page.getByRole('dialog', { name: 'Complete rec_matured' });
  await expect(completeDialog.getByLabel(/End the grants to apps and delegations of prn_gone/)).toBeChecked();
  await completeDialog.getByRole('button', { name: 'Complete succession' }).click();
  await expect(page.getByRole('status')).toContainText('prn_bob now owns spc_theirs.');
  await expect(page.getByRole('status')).toContainText('Ended 1 delegation(s) and 1 grant(s)');
  await expect(cases.getByRole('listitem').filter({ hasText: 'rec_matured' })).toHaveCount(0);

  // Delete waits for the id typed back.
  await page.getByRole('button', { name: 'Delete New Co' }).click();
  const del = page.getByRole('dialog', { name: 'Delete New Co' });
  await expect(del.getByRole('button', { name: 'Delete for good' })).toBeDisabled();
  await del.getByRole('button', { name: 'Cancel' }).click();

  // The screen fits a narrow window: one card a tenant.
  await page.setViewportSize({ width: 390, height: 844 });
  await fits(page);

  // Unlink, so the next spec links the door with its own passkey. The
  // touch given above still counts.
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.getByRole('link', { name: 'Deployment' }).click();
  await page.getByRole('button', { name: 'Unlink' }).click();
  await page.getByRole('button', { name: 'Yes, unlink' }).click();
  await expect(page.getByRole('status')).toContainText('Unlinked.');

  expect(problems).toEqual([]);
  await context.close();
});
