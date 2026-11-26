/**
 * The release-to-release case (docs/plan-versions.md §4, increment 6): Ordo
 * itself is upgraded under a signed-in person, the app does not change.
 *
 * The stack is the one in playwright.ordo-upgrade.config.ts. It starts the
 * previous Ordo release's `ordo-host` (built from its tag by
 * scripts/previous-ordo.sh), applies notes with the previous release's
 * `ordo`, and swaps this build's `ordo-host` in on the same database, with
 * the same keys, when this test creates `swap` in the stack's folder.
 *
 * Ada signs up and writes notes under the previous release. Her page stays
 * open on the open feed. After the swap, with no reload, a note written in
 * another tab reaches the open page; a reload keeps her signed in with every
 * note; and with the network off the page still shows every note, the ones
 * from before the upgrade among them, from the copy on her device.
 *
 * This is the upgrade that `crates/ordo-host/tests/upgrade_continuity.rs`
 * checks at the door; here it is a person's page.
 */

import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';
import { owner } from '../people.js';
import { addNote, noteInList, stack, unique } from '../support.js';

test('a signed-in person keeps their notes, their open feed and their device copy across an Ordo upgrade', async ({}, testInfo) => {
  testInfo.setTimeout(420_000);
  const s = stack();
  expect(s.previousRelease, 'the stack runs the previous release first (scripts/previous-ordo.sh)').toMatch(/^v\d/);
  const doorLog = `${s.logs}/door.log`;
  expect(readFileSync(doorLog, 'utf8')).not.toContain('swapped to this build');

  const ada = await owner('Ada');
  try {
    const before = [unique('before-a-'), unique('before-b-'), unique('before-c-')];
    for (const title of before) await addNote(ada.page, title, `Written under ${s.previousRelease}.`);
    // The feed is open on this page; the page does nothing else from here.
    await expect(ada.page.getByText('You are offline.', { exact: false })).toHaveCount(0);

    // Ordo is upgraded: the previous door stops, this build's starts.
    writeFileSync(`${s.logs}/swap`, '');
    await expect.poll(() => existsSync(`${s.logs}/swapped`), { timeout: 120_000, intervals: [500] }).toBe(true);
    expect(readFileSync(doorLog, 'utf8')).toContain('swapped to this build');

    // The open feed: another tab of Ada writes, and the first page, never
    // reloaded, shows it.
    const second = await ada.context.newPage();
    await second.goto('/');
    await expect(second.getByLabel('New note', { exact: true })).toBeVisible({ timeout: 60_000 });
    const after = unique('after-upgrade-');
    await addNote(second, after, 'Written under this build.');
    await expect(noteInList(ada.page, after)).toBeVisible({ timeout: 90_000 });
    for (const title of before) await expect(noteInList(ada.page, title)).toBeVisible();
    await second.close();

    // Signed in across a reload, with every note.
    await ada.page.reload();
    await expect(ada.page.getByLabel('New note', { exact: true })).toBeVisible({ timeout: 60_000 });
    for (const title of [...before, after]) await expect(noteInList(ada.page, title)).toBeVisible();

    // The device copy: the network off, every note is still on the page.
    await ada.context.setOffline(true);
    await expect(ada.page.getByText('You are offline. These are the notes this device holds.')).toBeVisible();
    for (const title of [...before, after]) await expect(noteInList(ada.page, title)).toBeVisible();
    await ada.context.setOffline(false);
    await expect(ada.page.getByText('You are offline.', { exact: false })).toHaveCount(0);
    await addNote(ada.page, unique('back-online-'));
  } finally {
    await ada.close();
  }
});
