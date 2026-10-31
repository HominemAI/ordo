/**
 * The page on a phone: Pixel 7 in Chromium (a narrow touch screen), so the
 * virtual passkey still works. Each flow walks the screens a person uses and
 * checks that nothing needed is off screen: the page does not scroll
 * sideways, and the buttons that matter are inside the window and enabled.
 * A tap, not a click, drives each step (the project has touch on).
 */

import { devices, expect, test, type Page } from '@playwright/test';
import { acceptInvitation, invitationFor, join, owner, sharedList } from '../people.js';
import { addNote, fitsWidth, noteInList, reachable, signedIn, signOut, unique } from '../support.js';

/** Tap an element the way a finger does. */
async function tap(page: Page, name: string, exact = true): Promise<void> {
  await reachable(page, name);
  await page.getByRole('button', { name, exact }).first().tap();
}

test('on a phone, a person signs up, writes a note with tags, and nothing is off screen', async ({ browser }) => {
  const ada = await join(browser, 'Ada');
  await expect(ada.page.viewportSize()!.width).toBeLessThan(500);
  await signedIn(ada.page);
  await fitsWidth(ada.page);
  await reachable(ada.page, 'Sign out');

  const title = unique('phone-');
  await ada.page.getByLabel('New note', { exact: true }).fill(title);
  await ada.page.getByLabel('New note text').fill('Written with a thumb.');
  await ada.page.getByLabel('Tags', { exact: true }).fill('phone');
  await reachable(ada.page, 'Add note');
  await tap(ada.page, 'Add note');
  await expect(noteInList(ada.page, title)).toBeVisible();
  await reachable(ada.page, 'phone');
  await fitsWidth(ada.page);

  // The sign-in screen fits too.
  await signOut(ada.page);
  await fitsWidth(ada.page);
  await ada.page.getByLabel('Your name').fill('Ada');
  for (const name of ['Sign in with a passkey', 'Sign up with a passkey']) await reachable(ada.page, name);
  await ada.context.close();
});

test('on a phone, a person edits a note and deletes another, and every button can be tapped', async ({ browser }) => {
  const ada = await join(browser, 'Ada');
  const first = unique('phone-edit-');
  const second = unique('phone-delete-');
  await addNote(ada.page, first, 'Before.');
  await addNote(ada.page, second, 'Doomed.');

  await noteInList(ada.page, first).tap();
  await fitsWidth(ada.page);
  for (const name of ['Save', 'Delete', 'Share', 'Close']) await reachable(ada.page, name);
  const renamed = unique('phone-renamed-');
  await ada.page.getByLabel('Title', { exact: true }).fill(renamed);
  await ada.page.getByRole('textbox', { name: 'Note', exact: true }).fill('After.');
  await ada.page.getByLabel('Tags', { exact: true }).fill('thumbs, small');
  await tap(ada.page, 'Save');
  await expect(noteInList(ada.page, renamed)).toBeVisible();
  await expect(noteInList(ada.page, first)).toHaveCount(0);
  await fitsWidth(ada.page);

  await noteInList(ada.page, second).tap();
  await tap(ada.page, 'Delete');
  await expect(noteInList(ada.page, second)).toHaveCount(0);
  await expect(noteInList(ada.page, renamed)).toBeVisible();

  // A tag filters on a phone, and the filter row fits.
  await tap(ada.page, 'thumbs');
  await expect(ada.page.getByText('Notes tagged thumbs')).toBeVisible();
  await reachable(ada.page, 'Show all');
  await fitsWidth(ada.page);
  await ada.context.close();
});

test('on a phone, a reader opens an invitation, agrees, reads the note, and the share panel fits', async ({
  browser,
}) => {
  const ada = await owner('Ada', devices['Pixel 7']);
  const title = unique('phone-share-');
  await addNote(ada.page, title, 'Read on a small screen.');
  const link = await invitationFor(ada.page, title);

  const bo = await join(browser, 'Bo');
  await bo.page.goto(link);
  await expect(bo.page.getByRole('heading', { name: 'Someone shared a note with you' })).toBeVisible();
  await fitsWidth(bo.page);
  await bo.page.getByLabel('The name to show them').fill('Bo');
  for (const name of ['Agree and read it', 'Not now']) await reachable(bo.page, name);
  await tap(bo.page, 'Agree and read it');
  await expect(sharedList(bo.page).getByText(title)).toBeVisible({ timeout: 30_000 });
  await expect(sharedList(bo.page).getByText('Read on a small screen.')).toBeVisible();
  await fitsWidth(bo.page);
  for (const name of ['Mine', 'Shared with me']) await reachable(bo.page, name);

  // The owner is on a phone too: the share panel with a reader in it fits.
  await expect(ada.page.getByText('Reader 1')).toBeVisible({ timeout: 30_000 });
  await fitsWidth(ada.page);
  for (const name of ['Make an invitation for a person', 'Stop sharing this note']) await reachable(ada.page, name);
  await ada.close();
  await bo.context.close();
});
