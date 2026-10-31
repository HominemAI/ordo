/**
 * The People screen in Chromium (docs/plan-console.md 4.5, step 8): a
 * tenant's members, what one can call, who is in a group, an invitation, a
 * role changed, a person added to a group and removed, who can call an
 * op; and (step 8b) suspend and reinstate, groups made, emptied and
 * deleted, invitations listed and revoked, an op's access, who may join, a
 * member's recovery and the recovery settings, each through the owner's touch at the deployment relayed by the
 * console. The door's answers are stand-ins over memory
 * (crates/ordo-console/tests/support/people_door.rs); the door's check is
 * the real one.
 */

import { expect, test } from '@playwright/test';
import { fits, person, recoverCode, savedAndContinue, shownCodes, linkAsNamedOwner } from './support';

test('see and change the people of a tenant', async ({ browser }) => {
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

  // Only the tenants where the owner holds a role are offered: Globex,
  // where the owner holds none, is not.
  await page.getByRole('link', { name: 'People' }).click();
  await expect(page.getByRole('heading', { name: 'People', level: 1 })).toBeVisible();
  const tenant = page.getByLabel('Tenant');
  await expect(tenant.getByRole('option', { name: 'Acme (you: owner)' })).toHaveCount(1);
  await expect(tenant.getByRole('option', { name: /Globex/ })).toHaveCount(0);
  await tenant.selectOption({ label: 'Acme (you: owner)' });

  // Reading asks for the touch at the deployment.
  await page.getByRole('button', { name: 'Show people' }).click();
  const prompt = page.getByRole('dialog', { name: 'Touch your passkey for the deployment' });
  await expect(prompt).toBeVisible();
  await prompt.getByRole('button', { name: 'Use my passkey' }).click();
  const members = page.getByRole('region', { name: '4 members' });
  await expect(members.getByRole('row', { name: /Bea Lund/ })).toContainText('manager');
  await expect(members.getByRole('row', { name: /Cal Ortiz/ })).toContainText('Support');

  // What one person can call, and how.
  await page.getByRole('button', { name: 'What Cal Ortiz can call' }).click();
  const can = page.getByRole('region', { name: 'What Cal Ortiz can call' });
  await expect(can).toContainText('notes.create as editor');
  await expect(can).toContainText("by an app's grant");

  // Who is in a group, and the grants given to apps.
  await page.getByRole('button', { name: 'Who is in Support' }).click();
  await expect(page.getByRole('region', { name: 'In Support' })).toContainText('Cal Ortiz');
  await expect(page.getByRole('region', { name: 'Live grants' })).toContainText('Mail helper');

  // An invitation: the ticket is shown once.
  await page.getByLabel('Role', { exact: true }).selectOption('editor');
  await page.getByRole('button', { name: 'Make invitation' }).click();
  await expect(page.getByRole('status')).toContainText('Invitation made for the role editor');
  await expect(page.getByLabel('Invitation', { exact: true })).toContainText('inv_');

  // Change a role.
  await page.getByRole('button', { name: 'Change the role of Dee Park' }).click();
  const role = page.getByRole('dialog', { name: 'Role of Dee Park' });
  await role.getByLabel('Role').selectOption('editor');
  await role.getByRole('button', { name: 'Save role' }).click();
  await expect(page.getByRole('status')).toContainText('Dee Park is now editor.');
  await expect(members.getByRole('row', { name: /Dee Park/ })).toContainText('editor');

  // Add to a group.
  await page.getByLabel('Group', { exact: true }).selectOption({ label: 'Support' });
  await page.getByLabel('Person', { exact: true }).selectOption({ label: 'Dee Park' });
  await page.getByRole('button', { name: 'Add to group' }).click();
  await expect(page.getByRole('status')).toContainText('Added Dee Park to Support.');

  // Remove, after a confirmation.
  await page.getByRole('button', { name: 'Remove Dee Park' }).click();
  const remove = page.getByRole('dialog', { name: 'Remove Dee Park' });
  await remove.getByRole('button', { name: 'Yes, remove' }).click();
  await expect(page.getByRole('status')).toContainText('Removed Dee Park.');
  await expect(page.getByRole('region', { name: '3 members' })).toBeVisible();

  // Who can call one op.
  await page.getByLabel('Op name').fill('notes.delete');
  await page.getByRole('button', { name: 'Find callers' }).click();
  const callers = page.getByRole('region', { name: 'Who can call notes.delete' });
  await expect(callers).toContainText('Bea Lund as manager');
  await expect(callers).toContainText('You as owner');

  // Step 8b. Suspend, after a confirmation, and reinstate.
  await page.getByRole('button', { name: 'Suspend Bea Lund' }).click();
  await page.getByRole('dialog', { name: 'Suspend Bea Lund' }).getByRole('button', { name: 'Yes, suspend' }).click();
  await expect(page.getByRole('status')).toContainText('Bea Lund is suspended.');
  await expect(page.getByRole('region', { name: '3 members' }).getByRole('row', { name: /Bea Lund/ })).toContainText(
    'suspended',
  );
  await page.getByRole('button', { name: 'Reinstate Bea Lund' }).click();
  await expect(page.getByRole('status')).toContainText('Bea Lund is active again.');

  // Take a person out of a group; make a group and delete it.
  await page.getByRole('button', { name: 'Who is in Support' }).click();
  await page.getByRole('button', { name: 'Take Cal Ortiz out of Support' }).click();
  await expect(page.getByRole('status')).toContainText('Took Cal Ortiz out of Support.');
  await page.getByLabel('Group name').fill('Night shift');
  await page.getByRole('button', { name: 'Make group' }).click();
  await expect(page.getByRole('status')).toContainText('Made the group Night shift.');
  await page.getByRole('button', { name: 'Delete the group Night shift' }).click();
  await page
    .getByRole('dialog', { name: 'Delete the group Night shift' })
    .getByRole('button', { name: 'Yes, delete' })
    .click();
  await expect(page.getByRole('status')).toContainText('Deleted the group Night shift.');

  // Invitations are listed by id, never by ticket, and one is revoked.
  await page.getByRole('button', { name: 'Show invitations' }).click();
  const invites = page.getByRole('region', { name: '1 invitation' });
  await expect(invites).toContainText('editor');
  await expect(invites).not.toContainText('inv_e2e_ticket');
  await invites.getByRole('button', { name: /Revoke the invitation/ }).click();
  await expect(page.getByRole('status')).toContainText('Revoked the invitation');
  await expect(page.getByText('No open invitations.')).toBeVisible();

  // Who may call an op: only owners may delete notes here.
  await page.getByRole('button', { name: 'Show access' }).click();
  await page.getByLabel('Op to change').selectOption('notes.delete');
  await page.getByRole('checkbox', { name: 'manager' }).uncheck();
  await page.getByRole('button', { name: 'Save access' }).click();
  await expect(page.getByRole('status')).toContainText('Saved who may call notes.delete.');
  await expect(page.getByRole('region', { name: 'Access by op' }).getByRole('row', { name: /notes\.delete/ })).toContainText(
    'yes',
  );

  // Who may join: open the tenant to anyone who signs in.
  await page.getByRole('button', { name: 'Show who may join' }).click();
  await page.getByLabel('Anyone who signs in may join').check();
  await page.getByRole('button', { name: 'Save who may join' }).click();
  await expect(page.getByRole('status')).toContainText('Saved who may join.');

  // A member's recovery: the ticket is shown once.
  await page.getByLabel('Person to recover').selectOption({ label: 'Cal Ortiz' });
  await page.getByRole('button', { name: 'Start a recovery' }).click();
  await expect(page.getByLabel('Recovery ticket')).toContainText('rtk_');

  // The tenant's recovery settings: stricter only.
  await page.getByLabel('People may use recovery codes').selectOption('off');
  await page.getByRole('button', { name: 'Save recovery settings' }).click();
  await expect(page.getByRole('status')).toContainText('Saved. Recovery waits');

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
