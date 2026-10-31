/**
 * Notes v2, round 2: search, and a share link with a password.
 *
 * Search runs on the device copies, the person's own and one per share.
 * The spec proves it finds the person's own notes and the notes shared with
 * them, by title, text and tag, and never a note of the sharer's that was not
 * shared. It keeps working with the network off.
 *
 * A link with a password asks for it, refuses a wrong one, and opens with the
 * right one, in a browser with no session.
 *
 * It needs v2 served. In the full run the `v2` project runs after `upgrade`,
 * which applies it. Run alone (`--project=v2 --no-deps`), it applies v2
 * itself. Chromium only: sharing asks for a passkey touch.
 */

import { expect, test, type Page } from '@playwright/test';
import { acceptInvitation, invitationFor, owner } from '../people.js';
import { addNote, applyProgram, noteInList, signIn, signedIn, stack, unique } from '../support.js';

/**
 * v2 is served: the upgrade spec applied it, or this applies it now. Either
 * way `ordo apply` returns once the door serves it and every tenant is
 * brought to it.
 */
function v2Served(): void {
  applyProgram(stack().programV2);
}

async function openV2(page: Page): Promise<void> {
  await page.goto(stack().pageV2);
  const notes = page.getByLabel('New note', { exact: true });
  const passkey = page.getByRole('button', { name: 'Sign in with a passkey' });
  await expect(notes.or(passkey)).toBeVisible({ timeout: 30_000 });
  if (await passkey.isVisible()) await signIn(page);
  await signedIn(page);
}

const searchRegion = (page: Page) => page.getByRole('region', { name: 'Search', exact: true });

// One worker, so v2 is applied once.
test.describe.configure({ mode: 'serial' });

test.beforeAll(() => {
  test.setTimeout(300_000);
  v2Served();
});

test('search finds a person’s own notes and the notes shared with them, and never one that was not shared', async ({}, testInfo) => {
  testInfo.setTimeout(300_000);
  const ada = await owner('Ada');
  const bo = await owner('Bo');
  try {
    await openV2(ada.page);
    await openV2(bo.page);
    const word = unique('zebra');
    const shared = unique('shared-');
    const kept = unique('kept-');
    const boOwn = unique('bo-own-');
    await addNote(ada.page, shared, `Bo may read about the ${word}.`);
    await addNote(ada.page, kept, `Only Ada reads about the ${word}.`);
    await addNote(bo.page, boOwn, 'Tagged, not written in the text.', word);

    const link = await invitationFor(ada.page, shared);
    await ada.page.getByRole('button', { name: 'Close', exact: true }).click();
    await acceptInvitation(bo.page, link, 'Bo', shared);

    await bo.page.getByRole('button', { name: 'Search', exact: true }).click();
    const box = bo.page.getByLabel('Search your notes and notes shared with you');
    await box.fill(word);
    const found = searchRegion(bo.page);
    await expect(found.getByText(boOwn)).toBeVisible({ timeout: 30_000 });
    await expect(found.getByText(shared)).toBeVisible({ timeout: 30_000 });
    await expect(found.getByRole('status')).toHaveText('2 notes here match.');
    await expect(found.getByText(kept)).toHaveCount(0);

    // Results follow the typing: a title word finds only that note.
    await box.fill(shared.slice(0, 10));
    await expect(found.getByText(shared)).toBeVisible();
    await expect(found.getByText(boOwn)).toHaveCount(0);

    // With the network off, the device copies still answer.
    await bo.context.setOffline(true);
    await box.fill(word.toUpperCase());
    await expect(found.getByText(boOwn)).toBeVisible();
    await expect(found.getByText(shared)).toBeVisible();
    await expect(found.getByText(kept)).toHaveCount(0);
    await bo.context.setOffline(false);
  } finally {
    await ada.close();
    await bo.close();
  }
});

test('a share link with a password asks for it, refuses a wrong one, and opens with the right one', async ({ browser }, testInfo) => {
  testInfo.setTimeout(240_000);
  const ada = await owner('Ada');
  const reader = await browser.newContext();
  try {
    await openV2(ada.page);
    const title = unique('locked-');
    await addNote(ada.page, title, 'Behind a password.');
    await noteInList(ada.page, title).click();
    await ada.page.getByRole('button', { name: 'Share', exact: true }).click();
    await ada.page.getByLabel('Password for the link (optional)').fill('right-horse-7');
    await ada.page.getByRole('button', { name: 'Make a link anyone can read' }).click();
    const field = ada.page.getByLabel('Link anyone can read');
    await expect(field).toHaveValue(/#\/link\//);
    const url = await field.inputValue();
    await expect(ada.page.getByText('Opening it asks for the password you set.')).toBeVisible();
    await expect(ada.page.getByText(/with a password/)).toBeVisible({ timeout: 30_000 });

    const page = await reader.newPage();
    await page.goto(url);
    await expect(page.getByText('This link needs a password.')).toBeVisible({ timeout: 30_000 });
    await expect(page.getByText(title)).toHaveCount(0);

    await page.getByLabel('Password', { exact: true }).fill('wrong-horse');
    await page.getByRole('button', { name: 'Open the note' }).click();
    await expect(page.getByText('That password is not right. Try again.')).toBeVisible({ timeout: 30_000 });
    await expect(page.getByText(title)).toHaveCount(0);

    await page.getByLabel('Password', { exact: true }).fill('right-horse-7');
    await page.getByRole('button', { name: 'Open the note' }).click();
    await expect(page.getByText(title)).toBeVisible({ timeout: 30_000 });
    await expect(page.getByText('Behind a password.')).toBeVisible();
  } finally {
    await reader.close();
    await ada.close();
  }
});
