/**
 * `ordo evolve` stops on bad data in a tenant, names it with its fix, and
 * runs once the data is fixed (plan-versions.md increments 8 and 9).
 *
 * 0.3.1 (fixtures/notes-v3-1) is notes v3 with `notes.title` made unique.
 * Ada, on v3, writes 2 notes with one title from the page. The developer
 * stores 0.3.1 and renders the step from 0.3.0. Its pre-checks ask every
 * database and list Ada's 2 notes with the fix. `--run` is refused while
 * the issue is open. Ada renames one note on the page; the pre-checks are
 * clean, and `--run` goes through.
 *
 * A regression looks like a pre-check that misses the duplicate, an issue
 * with no fix, a run that goes through over it, or a run still refused
 * once the title is changed.
 *
 * It needs v3 serving, so it runs after `v3`, last. It leaves 0.3.1 stored
 * and not active. Chromium only.
 */

import { cpSync, copyFileSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test, type Page } from '@playwright/test';
import { door, developer } from '../developer.js';
import { owner } from '../people.js';
import { addNote, applyProgram, noteInList, signIn, signedIn, stack, unique } from '../support.js';

test.describe.configure({ mode: 'serial' });

/** The 0.3.1 program: the fixture as `notes.ordo`, beside notes v3's `fn` code. */
function uniqueTitles(): string {
  const dir = mkdtempSync(join(tmpdir(), 'notes-v3-1-'));
  copyFileSync(fileURLToPath(new URL('../../fixtures/notes-v3-1.ordo.fixture', import.meta.url)), join(dir, 'notes.ordo'));
  cpSync(join(stack().programV3, 'fn'), join(dir, 'fn'), { recursive: true });
  return dir;
}

async function openV2(page: Page): Promise<void> {
  await page.goto(stack().pageV2);
  const notes = page.getByLabel('New note', { exact: true });
  const settingUp = page.getByRole('heading', { name: 'Setting up your account' });
  const passkey = page.getByRole('button', { name: 'Sign in with a passkey' });
  await expect(notes.or(settingUp).or(passkey)).toBeVisible({ timeout: 30_000 });
  if (await passkey.isVisible()) await signIn(page);
  await signedIn(page);
}

test('a duplicate title in a tenant: ordo evolve lists it with its fix and refuses to run, then runs once it is fixed', async ({
  browser,
}, testInfo) => {
  testInfo.setTimeout(300_000);
  // Run alone (`--project=evolve-checks --no-deps`), v3 is not applied yet.
  applyProgram(stack().programV2);
  applyProgram(stack().programV3);
  const ada = await owner('Ada');
  const dev = await developer(browser);
  try {
    const page = ada.page;
    await openV2(page);
    const twice = unique('twice-');
    await addNote(page, twice, 'The first one.');
    await addNote(page, twice, 'The second one.');
    await expect(noteInList(page, twice)).toHaveCount(2);

    await test.step('0.3.1 is stored beside 0.3.0', () => {
      const stored = dev.ordo(['apply', uniqueTitles(), '--no-activate', ...door()]);
      expect(stored.code, `${stored.out}\n${stored.err}`).toBe(0);
      expect(stored.out).toContain('0.3.1');
      expect(stored.out).toContain('activated  no');
    });

    await test.step('the pre-checks list the 2 notes, with the fix', () => {
      const plan = dev.ordo(['evolve', '--from', '0.3.0', '--to', '0.3.1', ...door()]);
      expect(plan.code, `${plan.out}\n${plan.err}`).toBe(0);
      expect(plan.out).toMatch(/^digest +sha256:[0-9a-f]+$/m);
      expect(plan.out).toMatch(/^prechecks +\d+ database\(s\) asked, 1 issue\(s\)$/m);
      const issue = plan.out.slice(plan.out.indexOf('ISSUE'));
      expect(issue, plan.out).toContain(twice);
      expect(issue).toMatch(/^ {2}fix +\S/m);
      expect(issue).toContain('unique');
    });

    await test.step('--run is refused while the issue is open', () => {
      const refused = dev.ordo(['evolve', '--from', '0.3.0', '--to', '0.3.1', '--run', ...door()]);
      expect(refused.code, refused.out).not.toBe(0);
      expect(`${refused.out}${refused.err}`).toContain('precheck');
    });

    const renamed = unique('renamed-');
    await test.step('Ada renames one of the 2 notes on the page', async () => {
      await noteInList(page, twice).last().click();
      await page.getByLabel('Title', { exact: true }).fill(renamed);
      // A saved note closes its editor.
      await page.getByRole('button', { name: 'Save', exact: true }).click();
      await expect(noteInList(page, renamed)).toBeVisible();
      await expect(noteInList(page, twice)).toHaveCount(1);
    });

    await test.step('the pre-checks are clean, and the run goes through', () => {
      const plan = dev.ordo(['evolve', '--from', '0.3.0', '--to', '0.3.1', ...door()]);
      expect(plan.code, `${plan.out}\n${plan.err}`).toBe(0);
      expect(plan.out).toMatch(/^prechecks +\d+ database\(s\) asked, 0 issue\(s\)$/m);
      const ran = dev.ordo(['evolve', '--from', '0.3.0', '--to', '0.3.1', '--run', ...door()]);
      expect(ran.code, `${ran.out}\n${ran.err}`).toBe(0);
      expect(ran.out).toMatch(/^ran +/m);
    });

    await expect(noteInList(page, twice)).toBeVisible();
    await expect(noteInList(page, renamed)).toBeVisible();
    await expect(page.getByRole('alert')).toHaveCount(0);
  } finally {
    await dev.close();
    await ada.close();
  }
});
