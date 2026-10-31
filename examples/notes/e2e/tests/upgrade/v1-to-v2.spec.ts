/**
 * The upgrade check: people sign up and write notes under v1, v2 is applied
 * over the running deployment, and they find everything under the v2 page.
 * Then they use what v2 adds: pins, the archive, files on a note and the
 * download of everything.
 *
 * v2's schema changes are all additive (examples/notes-v2/notes.ordo): a
 * field with a constant default (`pinned`), an optional field (`archived`)
 * and a new entity (`attachments`). So every v1 note reads unpinned and not
 * archived, and nothing about it changes.
 *
 * This spec changes the program for the whole stack, so it runs alone and
 * last (the `upgrade` project depends on all the others, playwright.config.ts).
 * The v2 page is served by the same server as v1, under /v2/, so it is the
 * same origin: what the browser stored for v1 is there for v2.
 * Chromium only: sharing asks for a passkey touch.
 */

import { readFileSync } from 'node:fs';
import { expect, test, type Locator, type Page } from '@playwright/test';
import { acceptInvitation, invitationFor, owner, sharedList, signInByEmail } from '../people.js';
import {
  addNote,
  applyProgram,
  mailFor,
  noteInList,
  signIn,
  signedIn,
  stack,
  unique,
} from '../support.js';

// The flows in this file run one after the other, after the upgrade that
// brings v2.
test.describe.configure({ mode: 'serial' });

/** A 1 by 1 PNG, the image attached under v2. */
const PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
  'base64',
);

/** The list of the person's own notes, top to bottom. */
const yourNotes = (page: Page) => page.getByRole('region', { name: 'Your notes' }).locator('ul.notes > li');

/** The entries of a zip whose entries are stored, as the page writes it: name to bytes. */
function unzip(data: Buffer): Map<string, Buffer> {
  const out = new Map<string, Buffer>();
  let at = 0;
  while (data.readUInt32LE(at) === 0x04034b50) {
    const size = data.readUInt32LE(at + 18);
    const nameLength = data.readUInt16LE(at + 26);
    const extra = data.readUInt16LE(at + 28);
    const name = data.toString('utf8', at + 30, at + 30 + nameLength);
    const start = at + 30 + nameLength + extra;
    out.set(name, data.subarray(start, start + size));
    at = start + size;
  }
  return out;
}

/** An image is drawn: its bytes came back from the blob door and decoded. */
async function imageShown(page: Page | Locator, name: string): Promise<void> {
  const img = page.getByRole('img', { name });
  await expect(img).toBeVisible({ timeout: 30_000 });
  await expect.poll(() => img.evaluate((i: HTMLImageElement) => i.complete && i.naturalWidth > 0)).toBe(true);
}

const chip = (page: Page, tag: string) =>
  page.getByRole('region', { name: 'Your notes' }).getByRole('button', { name: tag, exact: true });

/** Open the v2 page. Staying signed in should carry the person; if not, sign in. */
async function openV2(page: Page): Promise<void> {
  await page.goto(stack().pageV2);
  const notes = page.getByLabel('New note', { exact: true });
  const settingUp = page.getByRole('heading', { name: 'Setting up your account' });
  const passkey = page.getByRole('button', { name: 'Sign in with a passkey' });
  await expect(notes.or(settingUp).or(passkey)).toBeVisible({ timeout: 30_000 });
  if (await passkey.isVisible()) await signIn(page);
  await signedIn(page);
}

async function expectV2(page: Page): Promise<void> {
  await page.getByText('About this account').click();
  await expect(page.getByTestId('page-version')).toHaveText('Page version: v2');
}

test('two people with notes, tags and a share under v1 find all of it under v2, then pin, archive, attach and download', async ({}, testInfo) => {
  testInfo.setTimeout(300_000);
  const ada = await owner('Ada');
  const bo = await owner('Bo');
  try {
    const tag = unique('work').toLowerCase();
    const plain = unique('plain-');
    const tagged = unique('tagged-');
    const shared = unique('shared-');
    const boOwn = unique('bo-own-');
    await addNote(ada.page, plain, 'A plain note.');
    await addNote(ada.page, tagged, 'A tagged note.', tag);
    await addNote(ada.page, shared, 'Bo may read this.');
    await addNote(bo.page, boOwn, 'Bo wrote this.');
    const link = await invitationFor(ada.page, shared);
    await acceptInvitation(bo.page, link, 'Bo', shared);

    // v2 over the running v1. The page was not rebuilt; v2's page is served beside it.
    applyProgram(stack().programV2);

    await openV2(ada.page);
    await expectV2(ada.page);
    await expect(noteInList(ada.page, plain)).toBeVisible();
    await expect(noteInList(ada.page, tagged)).toBeVisible();
    await expect(noteInList(ada.page, shared)).toBeVisible();
    await expect(chip(ada.page, tag)).toBeVisible();
    await noteInList(ada.page, shared).click();
    await ada.page.getByRole('button', { name: 'Share', exact: true }).click();
    // The share panel names each reader by the name they agreed to show
    // (f9cbd5ab); Bo accepted as 'Bo'.
    await expect(ada.page.getByText('Bo', { exact: true })).toBeVisible({ timeout: 30_000 });

    await openV2(bo.page);
    await expectV2(bo.page);
    await expect(noteInList(bo.page, boOwn)).toBeVisible();
    await bo.page.getByRole('button', { name: 'Shared with me' }).click();
    await expect(sharedList(bo.page).getByText(shared)).toBeVisible({ timeout: 30_000 });
    await expect(sharedList(bo.page).getByText('Bo may read this.')).toBeVisible();

    // New writes work under v2, and the share still carries them.
    const fresh = unique('after-v2-');
    await ada.page.getByRole('button', { name: 'Close', exact: true }).click();
    await bo.page.getByRole('button', { name: 'Mine', exact: true }).click();
    await addNote(ada.page, fresh, 'Written under v2.', tag);
    await expect(chip(ada.page, tag).first()).toBeVisible();
    const boFresh = unique('bo-after-v2-');
    await addNote(bo.page, boFresh, 'Bo too.');
    await expect(noteInList(ada.page, fresh)).toBeVisible();

    // ── What v2 adds ──────────────────────────────────────────────────────
    // v1's notes gained the new defaults: not pinned, not archived.
    const page = ada.page;
    await expect(page.getByRole('button', { name: `Pin ${plain}` })).toBeVisible();
    await expect(page.getByRole('button', { name: `Archive ${tagged}` })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Archived (0)' })).toBeVisible();

    // Pin a v1 note: it goes to the top, above the newer ones.
    await page.getByRole('button', { name: `Pin ${plain}` }).click();
    await expect(page.getByRole('button', { name: `Unpin ${plain}` })).toBeVisible();
    await expect(yourNotes(page).first()).toContainText(plain);

    // Archive another: it leaves the list and waits behind the filter.
    await page.getByRole('button', { name: `Archive ${tagged}` }).click();
    await expect(noteInList(page, tagged)).toBeHidden();
    await expect(page.getByRole('button', { name: 'Archived (1)' })).toBeVisible();

    // Both work offline, from the device copy.
    await ada.context.setOffline(true);
    await page.getByRole('button', { name: 'Archived (1)' }).click();
    await expect(noteInList(page, tagged)).toBeVisible();
    await expect(noteInList(page, plain)).toBeHidden();
    await page.getByRole('button', { name: 'Notes', exact: true }).click();
    await expect(yourNotes(page).first()).toContainText(plain);
    await expect(noteInList(page, tagged)).toBeHidden();
    await ada.context.setOffline(false);

    // Attach an image and a file to the shared v1 note. The image shows in the page.
    const image = `${unique('photo-')}.png`;
    const textFile = `${unique('list-')}.txt`;
    await noteInList(page, shared).click();
    const attach = page.getByLabel('Attach images or files');
    await attach.setInputFiles([
      { name: image, mimeType: 'image/png', buffer: PNG },
      { name: textFile, mimeType: 'text/plain', buffer: Buffer.from('milk, eggs\n') },
    ]);
    await imageShown(page, image);
    await expect(page.getByRole('button', { name: `Download ${textFile}` })).toBeVisible({ timeout: 30_000 });
    // A file comes back as a download with the bytes it was sent with.
    const [fileDownload] = await Promise.all([
      page.waitForEvent('download'),
      page.getByRole('button', { name: `Download ${textFile}` }).click(),
    ]);
    expect(fileDownload.suggestedFilename()).toBe(textFile);
    expect(readFileSync(await fileDownload.path()).toString()).toBe('milk, eggs\n');
    await page.getByRole('button', { name: 'Close', exact: true }).click();

    // The v1 share listed only the notes, so Bo sees the note and no file. Shared
    // again under v2, it carries the files and the pins, and Bo sees the image.
    const again = await invitationFor(page, shared);
    await page.getByRole('button', { name: 'Close', exact: true }).click();
    await acceptInvitation(bo.page, again, 'Bo', shared);
    await imageShown(sharedList(bo.page), image);
    await expect(sharedList(bo.page).getByRole('button', { name: `Download ${textFile}` })).toBeVisible();

    // Everything as one file: every note, the archived one too, and the files.
    // Ordo's own download (`GET /download`): v2 marks `allNotes` and `myFiles`
    // with `download`, so the zip holds `notes.json`, `attachments.json` and
    // each file's bytes, read as Ada under the row guard.
    const [bundle] = await Promise.all([
      page.waitForEvent('download'),
      page.getByRole('button', { name: 'Download all my notes' }).click(),
    ]);
    expect(bundle.suggestedFilename()).toMatch(/^notes-\d{4}-\d{2}-\d{2}\.zip$/);
    const entries = unzip(readFileSync(await bundle.path()));
    const manifest = JSON.parse(entries.get('manifest.json')!.toString('utf8')) as { format: string; complete: boolean };
    expect(manifest.format).toBe('ordo.download');
    expect(manifest.complete).toBe(true);
    const notes = JSON.parse(entries.get('notes.json')!.toString('utf8')) as {
      id: string;
      title: string;
      pinned: boolean;
      archived: string | null;
    }[];
    const byTitle = new Map(notes.map((n) => [n.title, n]));
    for (const t of [plain, tagged, shared, fresh]) expect(byTitle.has(t), t).toBe(true);
    expect(byTitle.get(plain)!.pinned).toBe(true);
    expect(byTitle.get(tagged)!.archived).not.toBeNull();
    const attachments = JSON.parse(entries.get('attachments.json')!.toString('utf8')) as {
      note: string;
      name: string;
      file: { path?: string; missing?: string } | null;
    }[];
    const files = attachments.filter((a) => a.note === byTitle.get(shared)!.id);
    expect(files.map((f) => f.name).sort()).toEqual([image, textFile].sort());
    const png = files.find((f) => f.name === image)!;
    expect(entries.get(png.file!.path!)?.equals(PNG)).toBe(true);

    // Taking a file off a note: it leaves the note and the reader's view.
    // Its bytes are then collected by the blob worker once no row names them.
    await noteInList(page, shared).click();
    await page.getByRole('button', { name: `Remove ${textFile}` }).click();
    await expect(page.getByRole('button', { name: `Download ${textFile}` })).toBeHidden({ timeout: 30_000 });
    await expect(sharedList(bo.page).getByRole('button', { name: `Download ${textFile}` })).toBeHidden({ timeout: 30_000 });
  } finally {
    await ada.close();
    await bo.close();
  }
});

/** A `datetime-local` value in this machine's time zone, which the browser shares. */
function localInput(d: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
}

const SUBJECT = 'Subject: A reminder on one of your notes is due';

/**
 * Reminders (owner ruling 2026-10-22): a note gets a due time, and when it
 * comes the page says so, live, and the person gets one email. v2's
 * `fireReminders` routine is added by the apply above. The door starts it at
 * the version switch, with no restart, and `ordo apply` returns only after
 * that.
 */
test('a reminder set a moment ahead fires once: a notice in the page with no reload, and one email', async ({ browser }, testInfo) => {
  testInfo.setTimeout(300_000);
  // Run alone (`-g reminder`), v2 is not applied yet. After the flow above it
  // is, and applying it again changes nothing.
  applyProgram(stack().programV2);

  const context = await browser.newContext();
  try {
    const page = await context.newPage();
    const address = `${unique('cy')}@example.org`;
    await page.goto(stack().pageV2);
    await signInByEmail(page, address, 0);
    await expectV2(page);
    const title = unique('dentist-');
    await addNote(page, title, 'Call before noon.');

    // Due at the next whole minute: up to a minute ahead, what the input can say.
    const due = new Date(Math.floor(Date.now() / 60_000) * 60_000 + 60_000);
    await noteInList(page, title).click();
    await page.getByLabel('Remind me at').fill(localInput(due));
    await page.getByRole('button', { name: 'Set reminder' }).click();
    await expect(page.getByText('A reminder is set for')).toBeVisible();
    await page.getByRole('button', { name: 'Close', exact: true }).click();
    await expect(noteInList(page, title)).toContainText('Reminder ');

    // The routine runs every 30 seconds. The notice arrives live, with no reload.
    const notice = page.getByRole('status').getByText(`Reminder: ${title} was due`, { exact: false });
    await expect(notice).toBeVisible({ timeout: 150_000 });
    await expect(noteInList(page, title)).toContainText('Reminder due');

    // One email, and nothing from the note in it.
    const notices = async () => (await mailFor(address)).filter((m) => m.data.includes(SUBJECT));
    await expect.poll(async () => (await notices()).length, { timeout: 30_000 }).toBe(1);
    expect((await notices())[0]?.data).not.toContain('Call before noon');
    // A later run finds it fired and sends nothing more.
    await page.waitForTimeout(35_000);
    expect((await notices()).length).toBe(1);

    // Done takes the reminder off.
    await page.getByRole('button', { name: `Done with the reminder on ${title}` }).click();
    await expect(notice).toBeHidden();
    await expect(noteInList(page, title)).not.toContainText('Reminder');
  } finally {
    await context.close();
  }
});
