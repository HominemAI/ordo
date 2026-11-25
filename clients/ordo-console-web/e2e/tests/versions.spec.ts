/**
 * The Rollouts, Moves and Tenant builds screens in Chromium
 * (docs/plan-versions.md 3.7, 3.8 and 2.1; increments 7, 10, 10c, 11a and
 * 16). Every action is a door op, `ordo-host`'s own over the stack's app
 * database (crates/ordo-console/examples/e2e_door.rs), where three tenants
 * are registered and one node ticks the rollouts every second.
 *
 * Rollouts: a group naming no tenant is refused in the door's words; a
 * rolling rollout starts, moves its first batch, a tenant is held and one
 * skipped, and it is aborted with a reason; a rollout of every tenant at
 * once finishes. Moves: a refused move is tried again; ending a soak the
 * deployment cannot end shows the door's refusal; the dry run of a
 * rollback lists each tenant. Tenant builds: a bad name is refused, a build
 * is set, a tenant joins, removing a build that holds one is refused
 * naming the fix, the tenant leaves, the build moves and is removed.
 *
 * Runs after `windows`: the versions door.spec stored (#1 active, #2
 * stored) are the ones rolled out.
 */

import { expect, test, type Page } from '@playwright/test';
import { fits, linkAsNamedOwner, open, person, recoverCode, savedAndContinue, shownCodes, stack } from './support';

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

/** The page fits a phone, then back to a desktop window. */
async function fitsAPhone(page: Page): Promise<void> {
  await page.setViewportSize({ width: 390, height: 844 });
  await fits(page);
  await page.setViewportSize({ width: 1280, height: 800 });
}

/** Choose the stored version whose name starts `#n` in a select. */
async function chooseVersion(page: Page, label: string, n: number, scope = page.locator('body')): Promise<void> {
  const select = scope.getByLabel(label, { exact: true });
  const value = await select.locator('option', { hasText: new RegExp(`^#${n} `) }).getAttribute('value');
  expect(value, `version #${n} is offered`).toBeTruthy();
  await select.selectOption(value!);
}

/** Record an open move at the stand-in door, as an activation does for a tenant with its own database. */
async function seedMove(page: Page, tenant: string, to: string, phase: string, reason: string, soakSecs?: number) {
  const r = await page.request.post(`${stack().door}/e2e/move`, {
    data: { tenant, to, phase, reason, ...(soakSecs ? { soakSecs } : {}) },
  });
  expect(r.ok(), await r.text()).toBeTruthy();
}

test('roll out, hold, skip, abort and finish; retry a move, check a rollback; set and use a tenant build', async ({
  browser,
}) => {
  test.setTimeout(180_000);
  const { context, page, device, problems } = await person(browser);
  await page.goto('/');
  await page.getByRole('button', { name: 'Have an invite code?' }).click();
  await page.getByLabel('Code').fill(recoverCode());
  await page.getByRole('button', { name: 'Continue with a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'Your new passkey is ready' })).toBeVisible();
  await shownCodes(page);
  await savedAndContinue(page);
  await open(page, 'Connection');
  await linkAsNamedOwner(page, device);

  // ── Rollouts ────────────────────────────────────────────────────────
  await open(page, 'Rollouts');
  await expect(page.getByRole('heading', { name: 'Rollouts', level: 1 })).toBeVisible();
  await expect(page.getByText('No rollout has run yet.')).toBeVisible();

  // A group naming a tenant the deployment does not serve: the door's
  // refusal, as given, names where the tenants are listed.
  await chooseVersion(page, 'Version', 2);
  await page.getByLabel('Strategy').selectOption('group');
  await page.getByLabel('Tenants first (ids, comma separated)').fill('spc_nobody');
  await page.getByRole('button', { name: 'Start the rollout' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('alert')).toContainText('spc_nobody');
  await expect(page.getByRole('alert')).toContainText('std.deployment.tenants');

  // A rolling rollout, one tenant an hour: the first batch moves, the rest wait.
  await page.getByLabel('Strategy').selectOption('rolling');
  await page.getByText('Batches and limits').click();
  await page.getByLabel('Batch every (like 5m)').fill('1h');
  await page.getByLabel('Tenants per batch').fill('1');
  await page.getByRole('button', { name: 'Start the rollout' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Started rollout #');
  const rollout = page.getByRole('region', { name: /^Rollout #/ });
  await expect(rollout.getByText('running', { exact: true })).toBeVisible();
  const tenants = rollout.getByRole('list', { name: 'Rollout tenants' });
  // The leading worker moves the first batch within a tick or two.
  await expect(async () => {
    await page.getByRole('button', { name: 'Refresh' }).click();
    await expect(tenants.getByText('moved', { exact: true })).toHaveCount(1, { timeout: 1000 });
  }).toPass({ timeout: 20_000 });
  await expect(tenants.getByText('queued', { exact: true })).toHaveCount(2);

  // Hold one queued tenant with a reason, skip the other.
  const queued = tenants.getByRole('listitem').filter({ hasText: 'queued' });
  const first = (await queued.first().locator('strong').innerText()).trim();
  await queued.first().getByRole('button', { name: 'Hold' }).click();
  await page.getByLabel('Reason', { exact: true }).fill('customer demo today');
  await page.getByRole('button', { name: 'Hold this tenant' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText(`Holding ${first}.`);
  const second = tenants.getByRole('listitem').filter({ hasText: 'queued' }).filter({ hasNotText: first });
  const secondId = (await second.locator('strong').innerText()).trim();
  await second.getByRole('button', { name: 'Skip' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText(`Skipped ${secondId}.`);
  await expect(tenants.getByRole('listitem').filter({ hasText: secondId }).getByText('skipped', { exact: true })).toBeVisible();

  // Abort, with a reason.
  await rollout.getByRole('button', { name: 'Abort' }).click();
  await page.getByLabel('Reason', { exact: true }).fill('wrong build');
  await page.getByRole('button', { name: 'Abort the rollout' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Aborted rollout #');
  await expect(rollout.getByText('aborted', { exact: true })).toBeVisible();
  await expect(rollout).toContainText('wrong build');

  // Every tenant at once: it finishes, and #2 becomes the active version.
  await chooseVersion(page, 'Version', 2);
  await page.getByLabel('Strategy').selectOption('all');
  await page.getByRole('button', { name: 'Start the rollout' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Started rollout #');
  // The held tenant waits, saying why and what lifts it; lifting it lets it move.
  const held = tenants.getByRole('listitem').filter({ hasText: first });
  await expect(async () => {
    await page.getByRole('button', { name: 'Refresh' }).click();
    await expect(held.getByText('waiting', { exact: true })).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 20_000 });
  await expect(held).toContainText('customer demo today');
  await held.getByRole('button', { name: 'Lift the hold' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText(`Lifted the hold on ${first}.`);
  await expect(async () => {
    await page.getByRole('button', { name: 'Refresh' }).click();
    await expect(rollout.getByText('finished', { exact: true })).toBeVisible({ timeout: 1000 });
  }).toPass({ timeout: 30_000 });
  await fitsAPhone(page);

  // ── Moves ───────────────────────────────────────────────────────────
  await seedMove(page, 'spc_mine', '#1', 'refused', '`task.title` is unique, and 2 rows break it');
  await seedMove(page, 'spc_theirs', '#1', 'soaking', '', 86_400);
  await open(page, 'Moves');
  await expect(page.getByRole('heading', { name: 'Moves', level: 1 })).toBeVisible();
  const moves = page.getByRole('list', { name: 'Open moves' });
  const refused = moves.getByRole('listitem').filter({ hasText: 'Acme' });
  await expect(refused.getByText('refused', { exact: true })).toBeVisible();
  await expect(refused).toContainText('2 rows break it');
  await refused.getByRole('button', { name: 'Try the move again' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Acme moves to #1');
  await expect(refused.getByText('expanding', { exact: true })).toBeVisible();

  // Ending a soak the deployment holds no dual writes for: the door refuses, in its own words.
  const soaking = moves.getByRole('listitem').filter({ hasText: 'Globex' });
  await expect(soaking.getByText('soaking', { exact: true })).toBeVisible();
  await expect(soaking).toContainText('Its soak ends');
  await soaking.getByRole('button', { name: 'End the soak now' }).click();
  await page.getByRole('group', { name: /Confirm end the soak/ }).getByRole('button', { name: 'End the soak' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('alert')).toContainText('has no move with dual writes open');
  await expect(page.getByRole('alert')).toContainText('ordo evolve --tenant spc_theirs');

  // Each tenant's version, and the dry run of a rollback to #1.
  await expect(page.getByRole('list', { name: 'Tenant versions' }).getByRole('listitem')).not.toHaveCount(0);
  await chooseVersion(page, 'Version to return to', 1);
  await page.getByRole('button', { name: 'Check' }).click();
  const check = page.getByRole('region', { name: 'Rollback check' });
  await expect(check).toContainText('A rollback to #1');
  await expect(check.getByRole('list', { name: 'What happens to each tenant' })).toContainText('spc_held');
  await fitsAPhone(page);

  // ── Tenant builds ───────────────────────────────────────────────────
  await open(page, 'Tenant builds');
  await expect(page.getByRole('heading', { name: 'Tenant builds', level: 1 })).toBeVisible();
  await expect(page.getByText('No build is set.')).toBeVisible();

  // A name the door does not take: its refusal shows the shape it wants.
  await page.getByLabel('Name', { exact: true }).fill('Beta');
  await chooseVersion(page, 'Version', 1);
  await page.getByLabel('Reason', { exact: true }).fill('try the old shape');
  await page.getByRole('button', { name: 'Add the build' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('alert')).toContainText('like `beta`');

  await page.getByLabel('Name', { exact: true }).fill('beta');
  await page.getByRole('button', { name: 'Add the build' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Build beta is on #1');
  const beta = page.getByRole('list', { name: 'Builds' }).getByRole('listitem').filter({ hasText: 'beta' }).first();

  // A tenant joins, and serves #1.
  await beta.getByRole('button', { name: 'Add a tenant' }).click();
  await beta.getByLabel('Tenant', { exact: true }).selectOption('spc_held');
  await beta.getByRole('button', { name: 'Add it' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('joined beta and serves #1');
  await expect(beta.getByRole('list', { name: 'Tenants in beta' })).toContainText('spc_held');

  // A build that holds a tenant is not removed: the refusal names the fix.
  await beta.getByRole('button', { name: 'Remove' }).click();
  await beta.getByLabel('Reason', { exact: true }).fill('done');
  await beta.getByRole('button', { name: 'Remove the build' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('alert')).toContainText('ordo tenant-builds leave');
  await beta.getByRole('button', { name: 'Cancel' }).click();

  // The tenant leaves, back on the active version; the build moves, then goes.
  await beta.getByRole('button', { name: 'Leave' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('left beta');
  await expect(beta.getByText('It holds no tenant.')).toBeVisible();
  await beta.getByRole('button', { name: 'Change its version' }).click();
  await chooseVersion(page, 'Version', 2, beta);
  await beta.getByLabel('Reason', { exact: true }).fill('the new shape');
  await beta.getByRole('button', { name: 'Move the build' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Build beta is on #2');
  await beta.getByRole('button', { name: 'Remove' }).click();
  await beta.getByLabel('Reason', { exact: true }).fill('done');
  await beta.getByRole('button', { name: 'Remove the build' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Removed build beta.');
  await expect(page.getByText('No build is set.')).toBeVisible();
  await fitsAPhone(page);

  // Unlink, so a later spec links the door with its own passkey.
  await open(page, 'Connection');
  await page.getByRole('button', { name: 'Unlink' }).click();
  await page.getByRole('button', { name: 'Yes, unlink' }).click();
  await touchIfAsked(page);
  await expect(page.getByRole('status')).toContainText('Unlinked.');

  expect(problems).toEqual([]);
  await context.close();
});
