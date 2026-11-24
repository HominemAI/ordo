/**
 * Notes v3 applied over the running notes v2, with both pages open
 * (plan-versions.md, increment 7a).
 *
 * v3 (examples/notes-v3) adds `notes.starred` and stops declaring
 * `attachments.size`. Nothing note_viewer reads changes, so the viewer reads
 * through the move: every note is still shown there, and a note written in
 * notes after the move shows too. The notes page stays open across the move
 * with no reload and no error.
 *
 * A regression looks like the viewer refused or empty after the apply, a
 * note missing on either page, or the notes page reloading or showing an
 * error.
 *
 * It changes notes' program for the whole stack, so it runs last (the `v3`
 * project depends on `api` and `browser`, playwright.config.ts).
 */

import { expect, test } from '@playwright/test';
import { addNote, applyNotes, joinNotes, list, person, shows, stack, unique } from '../support.js';

test('notes v3 over a running notes v2: the viewer reads every note through it, and the notes page keeps them', async ({
  browser,
}) => {
  test.setTimeout(300_000);
  const cy = await person(browser);
  const notes = cy.page;
  await joinNotes(notes, 'Cy');
  const open = unique('Public before v3');
  const closed = unique('Private before v3');
  await addNote(notes, open, 'Anyone may read this.', 'public');
  await addNote(notes, closed, 'Only Cy reads this.');

  // The same person at the viewer, signed in, in a second tab.
  const viewer = await cy.context.newPage();
  await viewer.goto(stack().viewerPage);
  await viewer.getByRole('button', { name: 'Sign in' }).click();
  await expect(viewer).toHaveURL(new RegExp(`^${stack().issuer}/`));
  const join = viewer.getByRole('button', { name: 'Join' });
  const signOut = viewer.getByRole('button', { name: 'Sign out' });
  const passkey = viewer.getByRole('button', { name: 'Sign in with a passkey' });
  await expect(join.or(signOut).or(passkey)).toBeVisible({ timeout: 30_000 });
  if (await passkey.isVisible()) await passkey.click();
  await expect(join.or(signOut)).toBeVisible({ timeout: 30_000 });
  if (await join.isVisible()) await join.click();
  await expect(signOut).toBeVisible({ timeout: 30_000 });
  await shows(viewer, 'Your notes', closed);
  await shows(viewer, 'Your notes', open);

  // The move, with both pages open. The notes page is not reloaded from here.
  const reloads: string[] = [];
  notes.on('framenavigated', (f) => {
    if (f === notes.mainFrame()) reloads.push(f.url());
  });
  const applied = applyNotes(stack().programV3);
  expect(applied).toContain('0.3.0');

  // notes: both notes are still listed, and a note written now lands.
  const yours = notes.getByRole('region', { name: 'Your notes' });
  await expect(yours.locator('button.note').filter({ hasText: open })).toBeVisible();
  await expect(yours.locator('button.note').filter({ hasText: closed })).toBeVisible();
  const after = unique('Private after v3');
  await addNote(notes, after, 'Written under v3.');
  expect(reloads, 'the notes page did not reload').toEqual([]);
  await expect(notes.getByRole('alert')).toHaveCount(0);

  // The viewer reads through the move: the notes from before, and the new one.
  await shows(viewer, 'Your notes', closed);
  await shows(viewer, 'Your notes', open);
  await shows(viewer, 'Your notes', after);
  await shows(viewer, 'Public notes', open);
  await expect(list(viewer, 'Public notes').getByRole('heading', { name: closed })).toHaveCount(0);
  await cy.context.close();
});
