/**
 * Key screens, compared with a stored picture (owner ruling, 2026-10-21).
 * Chromium at desktop size. They block nothing: CI runs this project in its
 * own step that may fail without failing the job (docs/howto-e2e.md).
 *
 * Each person signs up under a fixed name and writes fixed titles, so the
 * content is the same every run. Only the account id, the invitation link and
 * the like are masked.
 */

import { expect, test } from '@playwright/test';
import { acceptInvitation, invitationFor, owner, sharedList } from '../people.js';
import { passkeyDevice, signUp } from '../support.js';
import { matches, NOTES, onlyWhereThePicturesAreMade, writeNotes } from './steady.js';

onlyWhereThePicturesAreMade();

test('screen: sign-in', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Sign up or sign in' })).toBeVisible();
  await expect(page.getByLabel('Your name')).toBeVisible();
  await matches(page, 'sign-in');
});

test('screen: notes list with tags', async ({ page }) => {
  await passkeyDevice(page);
  await signUp(page, 'Ada');
  await writeNotes(page);
  await expect(page.getByRole('button', { name: 'ideas', exact: true }).first()).toBeVisible();
  await matches(page, 'notes-list');
});

test('screen: a note open in the editor', async ({ page }) => {
  await passkeyDevice(page);
  await signUp(page, 'Ada');
  await writeNotes(page, [NOTES[1]]);
  await page.getByRole('region', { name: 'Your notes' }).getByRole('button', { name: /Meeting notes/ }).click();
  await expect(page.getByLabel('Title', { exact: true })).toHaveValue('Meeting notes');
  await expect(page.getByRole('textbox', { name: 'Note', exact: true })).toHaveValue(/Agree the plan/);
  await matches(page, 'note-editor');
});

test('screen: the share panel with an invitation', async ({ page }) => {
  await passkeyDevice(page);
  await signUp(page, 'Ada');
  await writeNotes(page, [NOTES[1]]);
  await invitationFor(page, 'Meeting notes');
  await expect(page.getByText('Nobody yet.')).toBeVisible();
  await expect(page.getByText('No links yet.')).toBeVisible();
  // The invitation link is a fresh secret each run.
  await matches(page, 'share-panel', [page.getByLabel('Invitation to send')]);
});

test('screen: Shared with you, for a reader', async ({ page }) => {
  const ada = await owner('Ada');
  await writeNotes(ada.page, [NOTES[1]]);
  const link = await invitationFor(ada.page, 'Meeting notes');

  await passkeyDevice(page);
  await signUp(page, 'Bo');
  await acceptInvitation(page, link, 'Bo', 'Meeting notes');
  await expect(sharedList(page).getByText(/Agree the plan/)).toBeVisible();
  await matches(page, 'shared-with-you');
  await ada.close();
});
