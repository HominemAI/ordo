/**
 * The console's step 1 flows in Chromium, in order: one console, one
 * deployment, at most two owners. Each test is one use case.
 */

import { existsSync } from 'node:fs';
import { expect, test, type BrowserContext, type Page } from '@playwright/test';
import { person, savedAndContinue, setupCode, shownCodes, signedIn, signOut, stack } from './support';

test.describe.configure({ mode: 'serial' });

let ctx: BrowserContext;
let page: Page;
let device: Awaited<ReturnType<typeof person>>['device'];
let problems: string[];
let codes: string[] = [];

test.beforeAll(async ({ browser }) => {
  ({ context: ctx, page, device, problems } = await person(browser));
});

test.afterAll(async () => {
  await ctx.close();
});

test.afterEach(() => {
  expect(problems, 'CSP refusals or page errors').toEqual([]);
});

test('first run: the setup code enrols a passkey and shows 10 recovery codes once', async () => {
  const res = await page.goto('/');
  expect(new URL(page.url()).pathname).toBe('/d/main/');
  expect(res?.headers()['content-security-policy']).toContain("default-src 'self'");
  await expect(page.getByRole('heading', { name: 'Set up the console' })).toBeVisible();

  await page.getByLabel('Code').fill('0000-0000-0000-0000-0000-0000');
  await page.getByRole('button', { name: 'Continue with a passkey' }).click();
  await expect(page.getByRole('alert')).toContainText('That code did not work');

  await page.getByLabel('Code').fill(setupCode());
  await page.getByLabel(/Name for this passkey/).fill('Desk');
  await page.getByRole('button', { name: 'Continue with a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'You are the console owner' })).toBeVisible();
  codes = await shownCodes(page);
  expect(new Set(codes).size).toBe(10);
  await savedAndContinue(page);
  await expect(page.getByRole('heading', { name: 'Health' })).toBeVisible();

  // The code dies with its use, and its file goes with it.
  expect(existsSync(`${stack().data}/setup-code`)).toBe(false);
  // A reload stays signed in, and no token sits in the page's storage.
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Health' })).toBeVisible();
  const stored = await page.evaluate(() => [localStorage.length, sessionStorage.length]);
  expect(stored).toEqual([0, 0]);
  const cookie = (await ctx.cookies()).find((c) => c.name === 'ordo-console');
  expect(cookie?.httpOnly).toBe(true);
  expect(cookie?.sameSite).toBe('Strict');
});

test('signs out, then back in with the passkey', async () => {
  await signOut(page);
  await expect(page.getByText('You are signed out.')).toBeVisible();
  await page.getByRole('button', { name: 'Sign in with a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'Health' })).toBeVisible();
});

test('invites a second owner, who joins with the code, then removes them', async ({ browser }) => {
  await page.getByRole('link', { name: 'Owners' }).click();
  expect(new URL(page.url()).pathname).toBe('/d/main/owners');
  await expect(page.getByRole('heading', { name: 'Owners' })).toBeVisible();
  await page.getByRole('button', { name: 'Make an invite code' }).click();
  const invite = (await page.getByLabel('Invite code').innerText()).trim();

  const second = await person(browser);
  await second.page.goto('/');
  await expect(second.page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
  await second.page.getByRole('button', { name: 'Have an invite code?' }).click();
  await second.page.getByLabel('Code').fill(invite);
  await second.page.getByLabel(/Name for this passkey/).fill('Second');
  await second.page.getByRole('button', { name: 'Continue with a passkey' }).click();
  await expect(second.page.getByRole('heading', { name: 'You are now a console owner' })).toBeVisible();
  await shownCodes(second.page);
  await savedAndContinue(second.page);

  await page.getByRole('button', { name: 'Done' }).click();
  const owners = page.locator('ul.owners > li');
  await expect(owners).toHaveCount(2);
  await expect(page.getByRole('button', { name: 'Make an invite code' })).toBeHidden();
  const row = owners.filter({ hasText: 'Second' });
  await row.getByRole('button', { name: 'Remove' }).click();
  await row.getByRole('button', { name: 'Yes, remove' }).click();
  await expect(page.getByText('Owner removed.')).toBeVisible();
  await expect(owners).toHaveCount(1);

  // The removed owner's session ends with them.
  await second.page.reload();
  await expect(second.page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
  expect(second.problems).toEqual([]);
  await second.context.close();
});

test('a recovery code signs in once, then forces a new passkey and new codes', async () => {
  await signOut(page);
  await device.lose();

  await page.getByRole('button', { name: /Use a recovery code/ }).click();
  await page.getByLabel('Recovery code').fill(codes[0] ?? '');
  await page.getByRole('button', { name: 'Use this code' }).click();
  await expect(page.getByRole('heading', { name: 'Add a new passkey' })).toBeVisible();
  // Nothing else opens until the new passkey is made.
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Add a new passkey' })).toBeVisible();

  await page.getByRole('button', { name: 'Add a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'Your new passkey is ready' })).toBeVisible();
  const fresh = await shownCodes(page);
  expect(fresh.filter((c) => codes.includes(c))).toEqual([]);
  await savedAndContinue(page);
  expect(await device.count()).toBe(1);

  // The old set is gone.
  await signOut(page);
  await page.getByRole('button', { name: /Use a recovery code/ }).click();
  await page.getByLabel('Recovery code').fill(codes[1] ?? '');
  await page.getByRole('button', { name: 'Use this code' }).click();
  await expect(page.getByRole('alert')).toContainText('That code did not work');
  await page.getByRole('button', { name: 'Back to sign in' }).click();

  // The new passkey signs in, back on the screen it left.
  await page.getByRole('button', { name: 'Sign in with a passkey' }).click();
  await signedIn(page);
  await expect(page.getByRole('heading', { name: 'Owners' })).toBeVisible();
  codes = fresh;
});
