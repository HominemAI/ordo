/**
 * The ways in besides a passkey, as the page offers them when the deployment
 * turns them on: an emailed code (read from the mail catcher), Discord (the
 * stand-in provider), and a guest account. Real Discord is checked by hand.
 */

import { expect, test, type Page } from '@playwright/test';
import { accountId, addNote, codeFor, noteInList, person, signedIn, signOut, unique } from '../support.js';

async function signInByEmail(page: Page, address: string, n: number): Promise<void> {
  await page.getByLabel('Email').fill(address);
  await page.getByRole('button', { name: 'Email me a code' }).click();
  await expect(page.getByText(`If ${address} can sign in here, a code is on its way.`)).toBeVisible();
  await page.getByLabel('Code').fill(await codeFor(address, n));
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await signedIn(page);
}

async function continueWithDiscord(page: Page, username: string, decision: 'Allow' | 'Deny' = 'Allow') {
  await page.getByRole('button', { name: 'Continue with Discord' }).click();
  await expect(page.getByRole('heading', { name: 'Stand-in Discord' })).toBeVisible();
  await page.getByLabel('User name').fill(username);
  await page.getByRole('button', { name: decision }).click();
}

test('a person signs in with an emailed code, and a later code finds the same notes', async ({ browser }) => {
  const kim = await person(browser);
  const address = `${unique('kim')}@example.org`;
  await kim.page.goto('/');
  await signInByEmail(kim.page, address, 0);
  const id = await accountId(kim.page);
  const title = unique('by-email-');
  await addNote(kim.page, title);

  await signOut(kim.page);
  await signInByEmail(kim.page, address, 1);
  await expect(noteInList(kim.page, title)).toBeVisible();
  expect(await accountId(kim.page)).toEqual(id);
  await kim.context.close();
});

test('a person continues with Discord (stand-in), and comes back to the same notes', async ({ browser }) => {
  const dee = await person(browser);
  const username = unique('dee');
  await dee.page.goto('/');
  await continueWithDiscord(dee.page, username);
  await signedIn(dee.page);
  await expect(dee.page).toHaveURL(/^http:\/\/localhost:\d+\/$/);
  const id = await accountId(dee.page);
  const title = unique('by-discord-');
  await addNote(dee.page, title);

  await signOut(dee.page);
  await continueWithDiscord(dee.page, username);
  await signedIn(dee.page);
  await expect(noteInList(dee.page, title)).toBeVisible();
  expect(await accountId(dee.page)).toEqual(id);
  await dee.context.close();
});

test('a person who denies Discord is told so and stays signed out', async ({ browser }) => {
  const dee = await person(browser);
  await dee.page.goto('/');
  await continueWithDiscord(dee.page, unique('nope'), 'Deny');
  await expect(dee.page.getByRole('heading', { name: 'Sign up or sign in' })).toBeVisible();
  await expect(dee.page.getByRole('alert')).toBeVisible();
  await expect(dee.page.getByLabel('New note', { exact: true })).toHaveCount(0);
  await dee.context.close();
});

test('a guest tries the app at once and writes a note', async ({ browser }) => {
  const guest = await person(browser);
  await guest.page.goto('/');
  await guest.page.getByRole('button', { name: 'Try it as a guest' }).click();
  await signedIn(guest.page);
  const title = unique('as-guest-');
  await addNote(guest.page, title);
  // A guest is offered a passkey, so they can get back from anywhere.
  await expect(guest.page.getByText(/passkey/i).first()).toBeVisible();
  await guest.context.close();
});
