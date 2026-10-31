/**
 * The notes page in a real browser, signed in with a virtual passkey.
 * Each test is one use case, named so a failure says which one broke.
 */

import { expect, test } from '@playwright/test';
import {
  accountId,
  addNote,
  noteInList,
  person,
  signIn,
  signOut,
  signUp,
  unique,
} from '../support.js';

test('a person signs up with a passkey, writes a note, reloads, and is still signed in', async ({ browser }) => {
  const ada = await person(browser);
  await signUp(ada.page, 'Ada');
  const title = unique('first-');
  await addNote(ada.page, title, 'Written in the browser.');

  // The token lives in memory, and the renewal cookie brings the sign-in
  // back on load (owner ruling 2026-10-21). No touch is asked for.
  await ada.page.reload();
  await expect(noteInList(ada.page, title)).toBeVisible();
  await expect(ada.page.getByRole('button', { name: 'Sign out' })).toBeVisible();
  // The cookie is out of the page's reach.
  expect(await ada.page.evaluate(() => document.cookie)).not.toContain('ordo_renew');

  // Signed out, a reload stays signed out.
  await signOut(ada.page);
  await ada.page.reload();
  await expect(ada.page.getByRole('heading', { name: 'Sign up or sign in' })).toBeVisible();
  await signIn(ada.page);
  await expect(noteInList(ada.page, title)).toBeVisible();
  await ada.context.close();
});

test("a second person signs up in another browser and cannot see the first person's note", async ({ browser }) => {
  const ada = await person(browser);
  await signUp(ada.page, 'Ada');
  const secret = unique('ada-only-');
  await addNote(ada.page, secret);

  const bo = await person(browser);
  await signUp(bo.page, 'Bo');
  await expect(bo.page.getByText('No notes yet. Write your first one above.')).toBeVisible();
  const mine = unique('bo-only-');
  await addNote(bo.page, mine);
  await expect(noteInList(bo.page, secret)).toHaveCount(0);
  await expect(noteInList(ada.page, mine)).toHaveCount(0);
  expect(await accountId(ada.page)).not.toEqual(await accountId(bo.page));
  await ada.context.close();
  await bo.context.close();
});

test('a shared note is read-only for the reader and disappears when the share is taken back', async ({ browser }) => {
  const ada = await person(browser);
  await signUp(ada.page, 'Ada');
  const title = unique('shared-');
  await addNote(ada.page, title, 'Bo may read this.');

  const bo = await person(browser);
  await signUp(bo.page, 'Bo');

  // Ada makes an invitation. It is a platform op and asks for a fresh passkey
  // touch, which the virtual device gives.
  await noteInList(ada.page, title).click();
  await ada.page.getByRole('button', { name: 'Share', exact: true }).click();
  await ada.page.getByRole('button', { name: 'Make an invitation for a person' }).click();
  const invitation = ada.page.getByLabel('Invitation to send');
  await expect(invitation).toHaveValue(/#\/accept\//);
  const link = await invitation.inputValue();

  // Bo opens it while signed in, agrees, and reads the note.
  await bo.page.goto(link);
  await expect(bo.page.getByRole('heading', { name: 'Someone shared a note with you' })).toBeVisible();
  await bo.page.getByLabel('The name to show them').fill('Bo');
  await bo.page.getByRole('button', { name: 'Agree and read it' }).click();
  const shared = bo.page.getByRole('region', { name: 'Shared with you' });
  await expect(shared.getByText(title)).toBeVisible({ timeout: 30_000 });
  await expect(shared.getByText('Bo may read this.')).toBeVisible();

  // Read-only: nothing to edit, save or delete, and the note is not Bo's own.
  await expect(shared.getByRole('textbox')).toHaveCount(0);
  await expect(shared.getByRole('button', { name: /^(Save|Delete|Share)$/ })).toHaveCount(0);
  await bo.page.getByRole('button', { name: 'Mine' }).click();
  await expect(noteInList(bo.page, title)).toHaveCount(0);
  await bo.page.getByRole('button', { name: 'Shared with me' }).click();
  await expect(shared.getByText(title)).toBeVisible();

  // Ada takes it back. Bo's open feed carries the change.
  await expect(ada.page.getByText('Reader 1')).toBeVisible({ timeout: 30_000 });
  await ada.page.getByRole('button', { name: 'Stop sharing this note' }).click();
  await expect(ada.page.getByText('Nobody yet.')).toBeVisible();
  await expect(shared.getByText(title)).toHaveCount(0, { timeout: 45_000 });
  await ada.context.close();
  await bo.context.close();
});

test('notes stay on screen with the network off, and writing waits for it', async ({ browser }) => {
  const ada = await person(browser);
  await signUp(ada.page, 'Ada');
  const title = unique('offline-');
  await addNote(ada.page, title, 'Kept on the device.');

  await ada.context.setOffline(true);
  await expect(ada.page.getByText('You are offline. These are the notes this device holds.')).toBeVisible();
  await expect(noteInList(ada.page, title)).toBeVisible();
  await ada.page.getByLabel('New note', { exact: true }).fill('not while offline');
  await expect(ada.page.getByRole('button', { name: 'Add note' })).toBeDisabled();
  await ada.page.getByLabel('New note', { exact: true }).fill('');
  await noteInList(ada.page, title).click();
  await expect(ada.page.getByRole('textbox', { name: 'Note', exact: true })).toHaveValue('Kept on the device.');
  await expect(ada.page.getByRole('button', { name: 'Save' })).toBeDisabled();
  await ada.page.getByRole('button', { name: 'Close' }).click();

  await ada.context.setOffline(false);
  await expect(ada.page.getByText('You are offline.', { exact: false })).toHaveCount(0);
  const after = unique('back-online-');
  await addNote(ada.page, after);
  await ada.context.close();
});

test('signing out and in again with the same passkey lands in the same tenant with the notes', async ({ browser }) => {
  const ada = await person(browser);
  await signUp(ada.page, 'Ada');
  const before = await accountId(ada.page);
  const title = unique('kept-');
  await addNote(ada.page, title);

  await signOut(ada.page);
  await expect(noteInList(ada.page, title)).toHaveCount(0);
  await signIn(ada.page);
  await expect(noteInList(ada.page, title)).toBeVisible();
  expect(await accountId(ada.page)).toEqual(before);
  await ada.context.close();
});
