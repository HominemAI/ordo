/**
 * The console on the identity service (docs/plan-identity-service.md 5.4,
 * 5.6, 5.7, step 6), against stack-identity.sh: the real identity service,
 * the real console with ORDO_IDENTITY_URL set, and the e2e door verifying
 * the service's tokens with an 8 second touch window.
 *
 * - Sign up by the first invite link, then sign in to the console at the
 *   service and come back.
 * - Pause a tenant: the door asks for a fresh touch, the page takes it at
 *   the service, and the pause goes through.
 * - Deployment people: invite a second admin, who joins; end their access,
 *   and their console sends them back to sign in.
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { expect, test, type Page } from '@playwright/test';
import { fits, person } from './support';

interface IdentityStack {
  console: string;
  identity: string;
  identityData: string;
}

function stack(): IdentityStack {
  const dir = process.env['E2E_ID_STATE'] ?? fileURLToPath(new URL('../.stack-identity', import.meta.url));
  return JSON.parse(readFileSync(`${dir}/stack.json`, 'utf8')) as IdentityStack;
}

/** Sign up at the identity service by an invite link, as `name`. */
async function join(page: Page, link: string, name: string): Promise<void> {
  await page.goto(link);
  await expect(page.getByRole('heading', { name: 'Join this deployment' })).toBeVisible();
  await page.getByLabel('Your name').fill(name);
  await page.getByRole('button', { name: 'Create a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'Save your recovery codes' })).toBeVisible();
  await page.getByLabel('I saved these codes somewhere safe').check();
  await page.getByRole('button', { name: 'Continue' }).click();
  await expect(page.getByText(`Signed in as ${name}`)).toBeVisible();
}

async function signedIn(page: Page): Promise<void> {
  await expect(page.getByRole('navigation', { name: 'Console' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Sign out' })).toBeVisible();
}

test('sign in at the identity service, touch for a tenant op, invite and end an admin', async ({ browser }) => {
  const s = stack();
  const dana = await person(browser);
  const { page } = dana;
  const touches: string[] = [];
  page.on('request', (r) => {
    if (r.url().startsWith(`${s.identity}/touch/finish`)) touches.push(r.url());
  });

  await test.step('sign up by the first invite, then open the console', async () => {
    await join(page, readFileSync(`${s.identityData}/first-invite`, 'utf8').trim(), 'Dana');
    await page.goto(`${s.console}/d/main/`);
    await signedIn(page);
  });

  await test.step('sign out, and sign in again at the identity service', async () => {
    await page.getByRole('button', { name: 'Sign out' }).click();
    await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
    await page.getByRole('button', { name: 'Sign in' }).click();
    await expect(page).toHaveURL(new RegExp(`^${s.identity}/sign-in`));
    await page.getByRole('button', { name: 'Sign in with a passkey' }).click();
    await expect(page).toHaveURL(new RegExp(`^${s.console}/d/main/`));
    await signedIn(page);
  });

  await test.step('a tenant op asks for a fresh touch, takes it, and goes through', async () => {
    // The sign-in was a touch; let it age past the door's 8 second window.
    await page.waitForTimeout(9_000);
    await page.getByRole('link', { name: 'Tenants' }).click();
    const table = page.getByRole('region', { name: 'Tenants, newest first' });
    const acme = table.getByRole('row', { name: /Acme/ });
    await expect(acme).toContainText('Active');
    const before = touches.length;
    await page.getByRole('button', { name: 'Pause Acme' }).click();
    const pause = page.getByRole('dialog', { name: 'Pause Acme' });
    await pause.getByLabel('Reason').selectOption('billing');
    await pause.getByRole('button', { name: 'Pause tenant' }).click();
    await expect(page.getByRole('status')).toContainText('Paused Acme.');
    await expect(acme).toContainText('Paused');
    expect(touches.length, 'the page took a touch at the identity service').toBe(before + 1);
  });

  let link = '';
  await test.step('invite a second admin', async () => {
    await page.getByRole('link', { name: 'Deployment people' }).click();
    await expect(page.getByRole('heading', { name: 'Deployment people', level: 1 })).toBeVisible();
    await expect(page.getByRole('list', { name: 'People' })).toContainText('Dana');
    await page.getByLabel('Who it is for (optional)').fill('Eli');
    await page.getByRole('button', { name: 'Make an invite link' }).click();
    link = (await page.getByLabel('Invite link').innerText()).trim();
    expect(link).toMatch(/\/join#/);
    await expect(page.getByRole('list', { name: 'Open invites' })).toContainText('Eli');
    await fits(page);
  });

  const eli = await person(browser);
  await test.step('Eli joins and reaches the console', async () => {
    await join(eli.page, link, 'Eli');
    await eli.page.goto(`${s.console}/d/main/`);
    await signedIn(eli.page);
  });

  await test.step("end Eli's access; Eli is sent back to sign in", async () => {
    await page.getByRole('button', { name: 'Done' }).click();
    await page.getByRole('link', { name: 'Health' }).click();
    await page.getByRole('link', { name: 'Deployment people' }).click();
    const row = page.getByRole('list', { name: 'People' }).getByRole('listitem').filter({ hasText: 'Eli' });
    await row.getByRole('button', { name: 'End access' }).click();
    const confirm = page.getByRole('form', { name: 'End access for Eli' });
    await expect(confirm.getByRole('button', { name: 'End access' })).toBeDisabled();
    await confirm.getByLabel('Name').fill('Eli');
    await confirm.getByRole('button', { name: 'End access' }).click();
    await expect(page.getByRole('status')).toContainText('Eli can no longer sign in.');
    await expect(page.getByRole('list', { name: 'People' })).not.toContainText('Eli');
    await expect(page.getByRole('list', { name: 'Audit log, newest first' })).toContainText('person.ended');

    await eli.page.goto(`${s.console}/d/main/`);
    await expect(eli.page).toHaveURL(new RegExp(`^${s.identity}/sign-in`));
  });

  expect(dana.problems).toEqual([]);
  await eli.context.close();
  await dana.context.close();
});
