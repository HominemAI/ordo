/**
 * Two deployments joined by shared sign-in and shared data (owner rulings
 * 2026-11-16 to 2026-11-18, `example-apps-notes-v2-plus-note_viewer`).
 *
 * A person signs up at the identity service from notes v2, with a passkey and
 * no invite, and writes a public note and a private one. note_viewer shows
 * the public note to a visitor who is signed out. The same person signs in
 * to note_viewer with the same passkey and sees the private note too. A
 * second person signs in to note_viewer and never sees the first person's
 * private note.
 *
 * A regression looks like a private note shown signed out or to another
 * person, a public note missing for a visitor, a sign-in that does not reach
 * note_viewer, or the same person not matched across the two deployments.
 */

import { expect, test } from '@playwright/test';
import { addNote, joinNotes, list, makeAccount, person, shows, stack, unique } from '../support.js';

test.describe.configure({ mode: 'serial' });

const open = unique('Open note');
const closed = unique('Closed note');

test('a person’s public note is shown to anyone, and their private one only to them after sign-in', async ({ browser }) => {
  const ada = await person(browser);
  const { page } = ada;

  await joinNotes(page, 'Ada');
  await addNote(page, open, 'Anyone may read this.', 'public');
  await addNote(page, closed, 'Only Ada reads this.');

  // Signed out of everything: a visitor. The cookies of all three servers are
  // on `localhost`, so this also ends Ada's session at the service, and the
  // sign-in below asks for her passkey again.
  await ada.context.clearCookies();
  await page.goto(stack().viewerPage);
  await expect(page.getByRole('button', { name: 'Sign in' })).toBeVisible();
  await shows(page, 'Public notes', open);
  await expect(page.getByText(closed)).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Your notes' })).toHaveCount(0);

  // Sign in through the identity service with the same passkey. She is not
  // yet a member of note_viewer, which takes anyone (open sign-up), so she
  // joins it with the account she holds.
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page).toHaveURL(new RegExp(`^${stack().issuer}/`));
  await page.getByRole('button', { name: 'Sign in with a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'Join this app' })).toBeVisible();
  await page.getByRole('button', { name: 'Join' }).click();

  await expect(page).toHaveURL(new RegExp(`^${stack().viewerPage}`));
  await expect(page.getByRole('button', { name: 'Sign out' })).toBeVisible({ timeout: 30_000 });
  await expect(list(page, 'Your notes').getByRole('heading', { name: closed })).toBeVisible();
  await expect(list(page, 'Your notes').getByRole('heading', { name: open })).toBeVisible();
  expect(await ada.passkeys(), 'one passkey made, used in both apps').toBe(1);

  // A reload stays signed in.
  await page.reload();
  await expect(list(page, 'Your notes').getByRole('heading', { name: closed })).toBeVisible();
  await ada.context.close();
});

test('another person sees their own notes and the public ones, never the first person’s private note', async ({
  browser,
}) => {
  const bo = await person(browser);
  const { page } = bo;
  const own = unique('Bo note');

  await joinNotes(page, 'Bo');
  await addNote(page, own, 'Only Bo reads this.');

  // Still signed in at the service: note_viewer's sign-in joins him at once.
  await page.goto(stack().viewerPage);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { name: 'Join this app' })).toBeVisible();
  await page.getByRole('button', { name: 'Join' }).click();
  await expect(page.getByRole('button', { name: 'Sign out' })).toBeVisible({ timeout: 30_000 });

  await shows(page, 'Your notes', own);
  await expect(list(page, 'Public notes').getByRole('heading', { name: open })).toBeVisible();
  await expect(page.getByText(closed)).toHaveCount(0);
  await bo.context.close();
});

test('a person who signs up at note_viewer first and holds nothing in notes v2 sees no private notes', async ({
  browser,
}) => {
  const cy = await person(browser);
  const { page } = cy;
  await page.goto(stack().viewerPage);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await makeAccount(page, 'Cy');
  await expect(page.getByRole('button', { name: 'Sign out' })).toBeVisible({ timeout: 30_000 });
  await expect(page.getByText('You have no notes in notes v2 yet.')).toBeVisible();
  await expect(page.getByText(closed)).toHaveCount(0);
  await cy.context.close();
});
