/**
 * Notes v3 over a running notes v2, with the page open (plan-versions.md,
 * increments 7 and 7a).
 *
 * v3 (examples/notes-v3) adds a field with a constant default
 * (`notes.starred`) and stops declaring one (`attachments.size`). Ada has
 * notes v2 open, with notes, a pin, a tag and a file. v3 is applied under
 * her. Her tenant has a database of its own, so it keeps serving v2 until
 * that database takes v3, and then its pin moves (the seamless move). The
 * page is never reloaded by the move: every note is still listed, the file
 * is still on its note, a note added after the move lands, and no error is
 * shown. A reload after it shows the same.
 *
 * A regression looks like a note missing after the move, an error in the
 * page, the page reloading itself, or `ordo apply` reporting a tenant held.
 *
 * It changes the program for the whole stack, so it runs last and alone
 * (the `v3` project depends on `v2`, playwright.config.ts). Chromium only.
 */

import { expect, test, type Page } from '@playwright/test';
import { owner } from '../people.js';
import { addNote, applyProgram, noteInList, signIn, signedIn, stack, unique } from '../support.js';

test.describe.configure({ mode: 'serial' });

async function openV2(page: Page): Promise<void> {
  await page.goto(stack().pageV2);
  const notes = page.getByLabel('New note', { exact: true });
  const settingUp = page.getByRole('heading', { name: 'Setting up your account' });
  const passkey = page.getByRole('button', { name: 'Sign in with a passkey' });
  await expect(notes.or(settingUp).or(passkey)).toBeVisible({ timeout: 30_000 });
  if (await passkey.isVisible()) await signIn(page);
  await signedIn(page);
}

const chip = (page: Page, tag: string) =>
  page.getByRole('region', { name: 'Your notes' }).getByRole('button', { name: tag, exact: true });

test('notes v3 applied with the v2 page open: every note and file stays, a new note lands, no reload and no error', async ({}, testInfo) => {
  testInfo.setTimeout(300_000);
  // Run alone (`--project=v3 --no-deps`), v2 is not applied yet.
  applyProgram(stack().programV2);
  const ada = await owner('Ada');
  try {
    const page = ada.page;
    await openV2(page);
    const tag = unique('work').toLowerCase();
    const plain = unique('plain-');
    const tagged = unique('tagged-');
    const pinned = unique('pinned-');
    const withFile = unique('with-file-');
    await addNote(page, plain, 'A plain note.');
    await addNote(page, tagged, 'A tagged note.', tag);
    await addNote(page, pinned, 'A pinned note.');
    await addNote(page, withFile, 'A note with a file.');
    await page.getByRole('button', { name: `Pin ${pinned}` }).click();
    await expect(page.getByRole('button', { name: `Unpin ${pinned}` })).toBeVisible();

    // A file on a note, under v2: its row has a size, which v3 stops declaring.
    const file = `${unique('list-')}.txt`;
    await noteInList(page, withFile).click();
    await page.getByLabel('Attach images or files').setInputFiles([
      { name: file, mimeType: 'text/plain', buffer: Buffer.from('milk, eggs\n') },
    ]);
    await expect(page.getByRole('button', { name: `Download ${file}` })).toBeVisible({ timeout: 30_000 });
    await page.getByRole('button', { name: 'Close', exact: true }).click();

    // The move, with the page open. From here on it is not reloaded by anything
    // but the test.
    const reloads: string[] = [];
    page.on('framenavigated', (f) => {
      if (f === page.mainFrame()) reloads.push(f.url());
    });
    const applied = applyProgram(stack().programV3);
    // Every tenant was brought to v3: none held on v2.
    expect(applied).toContain('0.3.0');
    expect(applied).not.toContain('held ');

    // Every note is still listed, with its pin and its tag.
    for (const t of [plain, tagged, pinned, withFile]) await expect(noteInList(page, t)).toBeVisible();
    await expect(page.getByRole('button', { name: `Unpin ${pinned}` })).toBeVisible();
    await expect(chip(page, tag)).toBeVisible();
    // A note written after the move lands and shows.
    const after = unique('after-v3-');
    await addNote(page, after, 'Written under v3.');
    await expect(noteInList(page, after)).toBeVisible({ timeout: 30_000 });
    // The file is still on its note.
    await noteInList(page, withFile).click();
    await expect(page.getByRole('button', { name: `Download ${file}` })).toBeVisible({ timeout: 30_000 });
    await page.getByRole('button', { name: 'Close', exact: true }).click();

    expect(reloads, 'the page did not reload').toEqual([]);
    await expect(page.getByRole('alert')).toHaveCount(0);

    // A reload after the move shows the same notes and no error.
    await page.reload();
    await signedIn(page);
    for (const t of [plain, tagged, pinned, withFile, after]) {
      await expect(noteInList(page, t)).toBeVisible({ timeout: 30_000 });
    }
    await expect(page.getByRole('alert')).toHaveCount(0);
  } finally {
    await ada.close();
  }
});
