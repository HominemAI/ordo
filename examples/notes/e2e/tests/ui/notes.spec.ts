/**
 * Writing, editing, deleting, tagging and sharing notes, in the browser the
 * project runs (Chromium, Firefox, Safari/WebKit). A person comes in by
 * passkey in Chromium and by emailed code elsewhere (tests/people.ts), so none
 * of these needs the virtual passkey except the owner who shares a note.
 */

import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, test, type Page } from '@playwright/test';
import { acceptInvitation, invitationFor, join, joinByEmail, owner, sharedList, signInByEmail } from '../people.js';
import { addNote, fitsWidth, noteInList, stack, unique } from '../support.js';

const titleBox = (page: Page) => page.getByLabel('Title', { exact: true });
const bodyBox = (page: Page) => page.getByRole('textbox', { name: 'Note', exact: true });
const tagsBox = (page: Page) => page.getByLabel('Tags', { exact: true });
const save = (page: Page) => page.getByRole('button', { name: 'Save', exact: true });

/** Open a note from the list, and wait for the editor to show its text. */
async function open(page: Page, title: string): Promise<void> {
  await noteInList(page, title).click();
  await expect(titleBox(page)).toHaveValue(title);
}

/** A tag chip under a note in the list. */
const chip = (page: Page, tag: string) =>
  page.getByRole('region', { name: 'Your notes' }).getByRole('button', { name: tag, exact: true });

test('a person edits a note, saves it, and sees the change in the list and after a reload', async ({ browser }) => {
  const ada = await join(browser, 'Ada');
  const before = unique('draft-');
  await addNote(ada.page, before, 'First words.');

  await open(ada.page, before);
  const after = unique('final-');
  await titleBox(ada.page).fill(after);
  await bodyBox(ada.page).fill('Second words.\nA second line.');
  await save(ada.page).click();

  // The editor closes and the list shows the new title and the first line.
  await expect(noteInList(ada.page, after)).toBeVisible();
  await expect(noteInList(ada.page, after)).toContainText('Second words.');
  await expect(noteInList(ada.page, before)).toHaveCount(0);

  // Still there after a reload, which keeps the person signed in, with the whole body.
  await ada.page.reload();
  await expect(noteInList(ada.page, after)).toBeVisible();
  await expect(noteInList(ada.page, before)).toHaveCount(0);
  await open(ada.page, after);
  await expect(bodyBox(ada.page)).toHaveValue('Second words.\nA second line.');
  await ada.context.close();
});

test('a note written in another tab of a new person shows in the first tab with no reload', async ({ browser }) => {
  // The person is new, so their tenant was made after the door booted. The
  // first tab holds an open feed and nothing else: no timer and no reload.
  const kim = await joinByEmail(browser, 'Kim');
  const second = await kim.anotherTab();
  const title = unique('live-');
  await addNote(second, title, 'Written in the second tab.');
  await expect(noteInList(kim.page, title)).toBeVisible();
  await kim.context.close();
});

test('the SDK can tell a blocked origin from a dead door: a no-cors probe is answered by a door that refuses the origin', async ({
  browser,
}) => {
  // The SDK's `OrdoNetworkError` kind rests on this: a browser hides what a
  // refused cross-origin call got, but a `no-cors` request to a server that is
  // up still resolves (opaque), and one to a dead port rejects.
  const context = await browser.newContext();
  const page = await context.newPage();
  // A page on a local origin the door does not list. It is served by a real
  // server: a page Playwright fulfils itself makes Chromium refuse every
  // cross-origin call, which would hide what this test is about.
  const site = createServer((_request, response) => {
    response.setHeader('content-type', 'text/html');
    response.end('<!doctype html><title>x</title>');
  });
  await new Promise<void>((resolve) => site.listen(0, 'localhost', resolve));
  await page.goto(`http://localhost:${(site.address() as AddressInfo).port}/`);
  const door = stack().door;
  const result = await page.evaluate(async (door) => {
    const outcome = async (url: string, init: RequestInit) => {
      try {
        const r = await fetch(url, init);
        return `answered:${r.type}`;
      } catch (e) {
        return `failed:${e instanceof TypeError ? 'TypeError' : String(e)}`;
      }
    };
    return {
      // The door refuses this origin's preflight, so the call fails.
      blocked: await outcome(`${door}/op/myNotes`, { headers: { 'X-Ordo-Space': 'spc_x', Authorization: 'Bearer x' } }),
      probe: await outcome(`${door}/health/live`, { mode: 'no-cors', cache: 'no-store' }),
      dead: await outcome('http://127.0.0.1:1/health/live', { mode: 'no-cors', cache: 'no-store' }),
    };
  }, door);
  expect(result.blocked).toBe('failed:TypeError');
  expect(result.probe).toBe('answered:opaque');
  expect(result.dead).toBe('failed:TypeError');
  await context.close();
  site.close();
});

test('after the network returns, the page catches up by itself', async ({ browser }) => {
  // The first page's feed cannot connect (the test refuses it), so its retries
  // back off by seconds. Another browser profile of the same person writes a
  // note. When the network returns, the page reconnects at once: a note that
  // shows within 5 s was fetched because the network returned, not because a
  // retry came due.
  const context = await browser.newContext();
  let refuse = true;
  await context.route(/[?&]live=1/, (route) => (refuse ? route.abort('failed') : route.continue()));
  const page = await context.newPage();
  const address = `${unique('kim')}@example.org`;
  await page.goto('/');
  await signInByEmail(page, address, 0);

  const other = await browser.newContext();
  const elsewhere = await other.newPage();
  await elsewhere.goto('/');
  await signInByEmail(elsewhere, address, 1);
  const title = unique('while-away-');
  await addNote(elsewhere, title, 'Written while the first page had no feed.');

  await page.waitForTimeout(10_000);
  await expect(noteInList(page, title)).toHaveCount(0);
  refuse = false;
  await context.setOffline(true);
  await context.setOffline(false);
  await expect(noteInList(page, title)).toBeVisible({ timeout: 5_000 });
  await other.close();
  await context.close();
});

test('two tabs edit the same note: the second save is refused, nothing is lost, and the person keeps their text', async ({
  browser,
}) => {
  // In Firefox the second tab's feed once stayed pending: both tabs' feeds
  // share a URL, and the first held its HTTP cache entry. Feeds now skip
  // the browser cache (fixed 2026-10-20), and this case runs everywhere.
  const kim = await joinByEmail(browser, 'Kim');
  const title = unique('shared-desk-');
  await addNote(kim.page, title, 'Original.');
  await open(kim.page, title); // tab 1 opens the note at its first revision

  const second = await kim.anotherTab();
  await open(second, title);
  const theirs = unique('tab-two-');
  await titleBox(second).fill(theirs);
  await bodyBox(second).fill('Saved from the second tab.');
  await save(second).click();
  await expect(noteInList(second, theirs)).toBeVisible();

  // Tab 1 saves from the old revision. The page refuses and shows what is stored.
  const mine = unique('tab-one-');
  await titleBox(kim.page).fill(mine);
  await bodyBox(kim.page).fill('Saved from the first tab.');
  await save(kim.page).click();
  const alert = kim.page.getByRole('alert').filter({ hasText: 'Changed elsewhere.' });
  await expect(alert).toBeVisible();
  await expect(alert).toContainText('Nothing was overwritten.');
  await expect(alert).toContainText(theirs);
  await expect(alert).toContainText('Saved from the second tab.');
  // The person's own text is still in the boxes, and Save waits for a choice.
  await expect(titleBox(kim.page)).toHaveValue(mine);
  await expect(save(kim.page)).toBeDisabled();

  // Keep my text: it saves over what is stored now.
  await kim.page.getByRole('button', { name: 'Keep my text' }).click();
  await expect(noteInList(kim.page, mine)).toBeVisible();
  await expect(noteInList(kim.page, theirs)).toHaveCount(0);
  await kim.context.close();
});

test('two tabs edit the same note: the person can take the stored text instead of their own', async ({ browser }) => {
  const kim = await joinByEmail(browser, 'Kim');
  const title = unique('either-or-');
  await addNote(kim.page, title, 'Original.');
  await open(kim.page, title);

  const second = await kim.anotherTab();
  await open(second, title);
  const theirs = unique('stored-');
  await titleBox(second).fill(theirs);
  await save(second).click();
  await expect(noteInList(second, theirs)).toBeVisible();

  await titleBox(kim.page).fill('lost on purpose');
  await save(kim.page).click();
  await expect(kim.page.getByRole('alert').filter({ hasText: 'Changed elsewhere.' })).toBeVisible();
  await kim.page.getByRole('button', { name: 'Use theirs' }).click();
  await expect(titleBox(kim.page)).toHaveValue(theirs);
  await expect(save(kim.page)).toBeEnabled();
  await kim.page.getByRole('button', { name: 'Close' }).click();
  await expect(noteInList(kim.page, theirs)).toBeVisible();
  await expect(noteInList(kim.page, 'lost on purpose')).toHaveCount(0);
  await kim.context.close();
});

test('a person deletes a note, it leaves the list, and it stays gone after a reload', async ({ browser }) => {
  const ada = await join(browser, 'Ada');
  const gone = unique('gone-');
  const kept = unique('kept-');
  await addNote(ada.page, gone, 'Delete me.');
  await addNote(ada.page, kept, 'Keep me.');

  await open(ada.page, gone);
  await ada.page.getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(noteInList(ada.page, gone)).toHaveCount(0);
  await expect(noteInList(ada.page, kept)).toBeVisible();

  // A reload keeps the person signed in (owner ruling 2026-10-21).
  await ada.page.reload();
  await expect(noteInList(ada.page, kept)).toBeVisible();
  await expect(noteInList(ada.page, gone)).toHaveCount(0);
  await ada.context.close();
});

test('a person shares a note by invitation, the reader accepts and reads it, and the reader loses it when it is deleted', async ({
  browser,
}) => {
  const ada = await owner('Ada');
  const title = unique('for-bo-');
  await addNote(ada.page, title, 'Bo may read this.');
  const link = await invitationFor(ada.page, title);

  const bo = await join(browser, 'Bo');
  await acceptInvitation(bo.page, link, 'Bo', title);
  await expect(sharedList(bo.page).getByText('Bo may read this.')).toBeVisible();
  await expect(sharedList(bo.page).getByRole('textbox')).toHaveCount(0);

  // Ada deletes the note. Bo's open feed carries the change: no reload, no
  // focus, no timer on the page.
  await expect(ada.page.getByText('Reader 1')).toBeVisible({ timeout: 30_000 });
  await ada.page.getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(noteInList(ada.page, title)).toHaveCount(0);
  await expect(sharedList(bo.page).getByText(title)).toHaveCount(0, { timeout: 60_000 });
  await ada.close();
  await bo.context.close();
});

test('tags typed when writing a note show on it, and a tag filters the list to its notes', async ({ browser }) => {
  const ada = await join(browser, 'Ada');
  const tag = unique('work').toLowerCase();
  const other = unique('home').toLowerCase();
  const a = unique('tagged-a-');
  const b = unique('tagged-b-');
  const c = unique('untagged-');
  // Tags are trimmed and lower-cased: "  TAG , other " becomes two tags.
  await addNote(ada.page, a, '', `  ${tag.toUpperCase()} , ${other} `);
  await addNote(ada.page, b, '', tag);
  await addNote(ada.page, c);

  const row = (title: string) =>
    ada.page.locator('li').filter({ has: ada.page.getByRole('button', { name: new RegExp(title) }) });
  await expect(row(a).getByRole('button', { name: tag, exact: true })).toBeVisible();
  await expect(row(a).getByRole('button', { name: other, exact: true })).toBeVisible();
  await expect(row(b).getByRole('button', { name: tag, exact: true })).toBeVisible();
  await expect(row(c).locator('.tags')).toHaveCount(0);

  // Pressing a tag shows only the notes that carry it.
  await row(a).getByRole('button', { name: tag, exact: true }).click();
  await expect(ada.page.getByText(`Notes tagged ${tag}`)).toBeVisible();
  await expect(noteInList(ada.page, a)).toBeVisible();
  await expect(noteInList(ada.page, b)).toBeVisible();
  await expect(noteInList(ada.page, c)).toHaveCount(0);

  // The other tag has one note.
  await ada.page.getByRole('button', { name: 'Show all' }).click();
  await row(a).getByRole('button', { name: other, exact: true }).click();
  await expect(noteInList(ada.page, a)).toBeVisible();
  await expect(noteInList(ada.page, b)).toHaveCount(0);

  // Show all brings every note back.
  await ada.page.getByRole('button', { name: 'Show all' }).click();
  await expect(noteInList(ada.page, c)).toBeVisible();
  await expect(noteInList(ada.page, b)).toBeVisible();
  await fitsWidth(ada.page);
  await ada.context.close();
});

test('tags are added and taken off when editing, and a tag no note carries shows the empty state', async ({
  browser,
}) => {
  const ada = await join(browser, 'Ada');
  const tag = unique('solo').toLowerCase();
  const title = unique('tag-me-');
  await addNote(ada.page, title, 'No tags yet.');
  await expect(chip(ada.page, tag)).toHaveCount(0);

  // Add a tag in the editor.
  await open(ada.page, title);
  await expect(tagsBox(ada.page)).toHaveValue('');
  await tagsBox(ada.page).fill(tag);
  await save(ada.page).click();
  await expect(chip(ada.page, tag)).toBeVisible();

  // It is still there after a reload.
  // A reload keeps the person signed in (owner ruling 2026-10-21).
  await ada.page.reload();
  await expect(chip(ada.page, tag)).toBeVisible();

  // Filter by it, open the one note, take the tag off, and save.
  await chip(ada.page, tag).click();
  await expect(noteInList(ada.page, title)).toBeVisible();
  await open(ada.page, title);
  await expect(tagsBox(ada.page)).toHaveValue(tag);
  await tagsBox(ada.page).fill('');
  await save(ada.page).click();

  // The filter is still on, no note carries the tag, and the page says so.
  await expect(ada.page.getByText('No note carries this tag.')).toBeVisible();
  await expect(noteInList(ada.page, title)).toHaveCount(0);
  await ada.page.getByRole('button', { name: 'Show all' }).click();
  await expect(noteInList(ada.page, title)).toBeVisible();
  await expect(chip(ada.page, tag)).toHaveCount(0);
  await ada.context.close();
});

test('notes stay on screen with the network off in any browser, and writing waits for it', async ({ browser }) => {
  const ada = await join(browser, 'Ada');
  const title = unique('offline-any-');
  await addNote(ada.page, title, 'Kept on the device.');

  await ada.context.setOffline(true);
  await expect(ada.page.getByText('You are offline. These are the notes this device holds.')).toBeVisible();
  await expect(noteInList(ada.page, title)).toBeVisible();
  await ada.page.getByLabel('New note', { exact: true }).fill('not while offline');
  await expect(ada.page.getByRole('button', { name: 'Add note' })).toBeDisabled();
  await ada.page.getByLabel('New note', { exact: true }).fill('');
  await open(ada.page, title);
  await expect(save(ada.page)).toBeDisabled();
  await expect(ada.page.getByRole('button', { name: 'Delete', exact: true })).toBeDisabled();
  await ada.page.getByRole('button', { name: 'Close' }).click();

  await ada.context.setOffline(false);
  await expect(ada.page.getByText('You are offline.', { exact: false })).toHaveCount(0);
  await addNote(ada.page, unique('back-online-'));
  await ada.context.close();
});
