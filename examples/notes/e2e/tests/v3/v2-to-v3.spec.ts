/**
 * Notes v3 over a running notes v2, with the page open (plan-versions.md,
 * increments 7 and 7a).
 *
 * v3 (examples/notes-v3) adds a field with a constant default
 * (`notes.starred`), stops declaring one (`attachments.size`), and renames
 * `attachments.kind` to `mediaType` with `prev(kind)` (increment 15). Ada
 * has notes v2 open, with notes, a pin, a tag, a file and an image. v3 is applied under
 * her. Her tenant has a database of its own, so it keeps serving v2 until
 * that database takes v3, and then its pin moves (the seamless move). The
 * page is never reloaded by the move: every note is still listed, the file
 * is still on its note, a note added after the move lands, and no error is
 * shown. The image is still drawn after a reload, which it is only while
 * its media type reads `image/png`: the rename copied every v2 value. An
 * image attached after the move is drawn too.
 *
 * A regression looks like a note missing after the move, an error in the
 * page, the page reloading itself, `ordo apply` reporting a tenant held, or
 * the image offered as a download (its media type lost in the rename).
 *
 * After the `evolve` project, the database already holds v3's columns and
 * v3 is stored, not active: v2 still serves, and the file and the image are
 * attached under v2 after the rename's copy. The host's dual writes
 * (increment 11a) carry their media type to `mediaType` in the same write,
 * and the apply here activates v3.
 *
 * It changes the program for the whole stack, so it runs last and alone
 * (the `v3` project depends on `evolve`, playwright.config.ts). Chromium only.
 */

import { expect, test, type Page } from '@playwright/test';
import { owner } from '../people.js';
import { addNote, applyProgram, ensureServing, noteInList, signIn, signedIn, stack, unique } from '../support.js';

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

/** A 1 by 1 PNG. */
const PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
  'base64',
);

/** An image is drawn: its bytes came back from the blob door and decoded. */
async function imageShown(page: Page, name: string): Promise<void> {
  const img = page.getByRole('img', { name });
  await expect(img).toBeVisible({ timeout: 30_000 });
  await expect.poll(() => img.evaluate((i: HTMLImageElement) => i.complete && i.naturalWidth > 0)).toBe(true);
}

const chip = (page: Page, tag: string) =>
  page.getByRole('region', { name: 'Your notes' }).getByRole('button', { name: tag, exact: true });

test('notes v3 applied with the v2 page open: every note and file stays, a new note lands, no reload and no error', async ({}, testInfo) => {
  testInfo.setTimeout(300_000);
  // v2 serves after the `v2` project. Run alone (`--no-deps`), it is
  // applied here. Never applied again over v3's schema, which `evolve`
  // leaves in place: v3 renames a column v2 requires, so that is refused.
  ensureServing(stack().programV2, '0.2.0');
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

    // A file and an image on a note, under v2: each row has a size, which v3
    // stops declaring, and a kind, which v3 renames to `mediaType`.
    const file = `${unique('list-')}.txt`;
    const image = `${unique('photo-')}.png`;
    await noteInList(page, withFile).click();
    await page.getByLabel('Attach images or files').setInputFiles([
      { name: file, mimeType: 'text/plain', buffer: Buffer.from('milk, eggs\n') },
      { name: image, mimeType: 'image/png', buffer: PNG },
    ]);
    await expect(page.getByRole('button', { name: `Download ${file}` })).toBeVisible({ timeout: 30_000 });
    await imageShown(page, image);
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
    // The file is still on its note, and the image is still drawn: its media
    // type came across the rename. An image added under v3 is drawn too.
    await noteInList(page, withFile).click();
    await expect(page.getByRole('button', { name: `Download ${file}` })).toBeVisible({ timeout: 30_000 });
    await imageShown(page, image);
    const later = `${unique('photo-v3-')}.png`;
    await page.getByLabel('Attach images or files').setInputFiles([
      { name: later, mimeType: 'image/png', buffer: PNG },
    ]);
    await imageShown(page, later);
    await page.getByRole('button', { name: 'Close', exact: true }).click();

    expect(reloads, 'the page did not reload').toEqual([]);
    await expect(page.getByRole('alert')).toHaveCount(0);

    // A reload after the move shows the same notes and no error.
    await page.reload();
    await signedIn(page);
    for (const t of [plain, tagged, pinned, withFile, after]) {
      await expect(noteInList(page, t)).toBeVisible({ timeout: 30_000 });
    }
    // Read from v3 after the reload: both images are still images.
    await noteInList(page, withFile).click();
    await imageShown(page, image);
    await imageShown(page, later);
    await expect(page.getByRole('button', { name: `Download ${image}` })).toHaveCount(0);
    await page.getByRole('button', { name: 'Close', exact: true }).click();
    await expect(page.getByRole('alert')).toHaveCount(0);
  } finally {
    await ada.close();
  }
});
