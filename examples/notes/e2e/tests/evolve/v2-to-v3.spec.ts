/**
 * `ordo evolve` from notes v2 to notes v3, through the door, with the v2
 * page open (plan-versions.md increment 8).
 *
 * The developer signs in as a real one does: the identity service's first
 * invite, then `ordo login` approved with their passkey (tests/developer.ts).
 * Every command below goes to the door with that session. Nothing opens a
 * database.
 *
 * Ada has notes v2 open with 3 notes. The developer stores v3 beside v2
 * (`ordo apply --no-activate`), renders and stores the step (`ordo evolve
 * --from 0.2.0 --to 0.3.0`), which prints the plan, its digest and the
 * pre-checks of every database, then runs it (`--run`). The page is never
 * reloaded and shows every note, with no error.
 *
 * A regression looks like `ordo login` failing, a door refusing the
 * developer's token, a plan with no digest, a pre-check issue on a v2 to
 * v3 step (v3 only adds), a refused run, or a note missing from the page.
 *
 * It stores v3 on the stack, so it runs after `v2` and before `v3`
 * (playwright.config.ts). Chromium only.
 */

import { expect, test, type Page } from '@playwright/test';
import { door, developer } from '../developer.js';
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

test('ordo evolve from v2 to v3 through the door, with the page open: the plan, its digest, a run, every note kept', async ({
  browser,
}, testInfo) => {
  testInfo.setTimeout(300_000);
  // Run alone (`--project=evolve --no-deps`), v2 is not applied yet.
  applyProgram(stack().programV2);
  const ada = await owner('Ada');
  const dev = await developer(browser);
  try {
    const page = ada.page;
    await openV2(page);
    const titles = [unique('first-'), unique('second-'), unique('pinned-')];
    for (const t of titles) await addNote(page, t, 'Written under v2.');
    await page.getByRole('button', { name: `Pin ${titles[2]}` }).click();
    await expect(page.getByRole('button', { name: `Unpin ${titles[2]}` })).toBeVisible();
    const reloads: string[] = [];
    page.on('framenavigated', (f) => {
      if (f === page.mainFrame()) reloads.push(f.url());
    });

    await test.step('who the CLI acts as', () => {
      const who = dev.ordo(['whoami']);
      expect(who.code, who.err).toBe(0);
    });

    await test.step('v3 is stored beside v2, and v2 still serves', () => {
      const stored = dev.ordo(['apply', stack().programV3, '--no-activate', ...door()]);
      expect(stored.code, `${stored.out}\n${stored.err}`).toBe(0);
      expect(stored.out).toContain('0.3.0');
      expect(stored.out).toContain('activated  no');
    });

    let digest = '';
    await test.step('the step is rendered and stored: the plan, its digest, the pre-checks', () => {
      const plan = dev.ordo(['evolve', '--from', '0.2.0', '--to', '0.3.0', ...door()]);
      expect(plan.code, `${plan.out}\n${plan.err}`).toBe(0);
      expect(plan.out).toContain('plan       0.2.0 to 0.3.0');
      digest = /^digest +(sha256:[0-9a-f]+)$/m.exec(plan.out)?.[1] ?? '';
      expect(digest, plan.out).not.toBe('');
      // What v3 adds, in the receipt.
      expect(plan.out).toContain('starred');
      // v3 only adds: every database asked, none with an issue.
      expect(plan.out).toMatch(/^prechecks +\d+ database\(s\) asked, 0 issue\(s\)$/m);
      expect(plan.out).not.toContain('ISSUE');
    });

    await test.step('the stored plan runs, with the digest it was stored under', () => {
      const ran = dev.ordo(['evolve', '--from', '0.2.0', '--to', '0.3.0', '--digest', digest, '--run', ...door()]);
      expect(ran.code, `${ran.out}\n${ran.err}`).toBe(0);
      expect(ran.out).toContain(`digest     ${digest}`);
      expect(ran.out).toMatch(/^ran +/m);
      const list = dev.ordo(['evolve', '--list', ...door()]);
      expect(list.code, list.err).toBe(0);
      expect(list.out).toContain(digest);
    });

    await test.step('the page shows every note, with no reload and no error', async () => {
      for (const t of titles) await expect(noteInList(page, t)).toBeVisible();
      await expect(page.getByRole('button', { name: `Unpin ${titles[2]}` })).toBeVisible();
      const after = unique('after-');
      await addNote(page, after, 'Written after the run.');
      expect(reloads, 'the page did not reload').toEqual([]);
      await expect(page.getByRole('alert')).toHaveCount(0);
    });
  } finally {
    await dev.close();
    await ada.close();
  }
});
