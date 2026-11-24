/**
 * A rename on one tenant under live writes, through the door, with the page
 * open (plan-versions.md increment 11a; plan-backfill-dual-writes.md §9).
 *
 * The new version is the one production serves with `notes.title` renamed
 * to `heading` with `prev(title)`. Its ops still answer `title`, so the v2
 * page reads the same notes. After `rehearsal`, production serves 0.3.4
 * (notes.body sealed, notes off the reader line) and the new version is
 * 0.3.6. Run alone (`--no-deps`), v3 is applied and it is 0.3.2. The
 * developer, signed in with `ordo login` (tests/developer.ts), stores it and
 * its step, then moves Ada's tenant onto it:
 *
 * 1. `ordo evolve --tenant <Ada> --budget 1 --chunk-rows 2`: the expand and
 *    one chunk of the copy. The move stops in `backfilling`, Ada still on
 *    the version before, and every write she makes keeps both columns.
 * 2. Ada retitles a note and adds one, on the page.
 * 3. `ordo evolve --tenant <Ada>`: the rest of the copy, verify, the switch.
 *    Ada is on the new version in a 7-day soak. She retitles another note.
 * 4. `ordo evolve --tenant <Ada> --revert`: back inside the soak. After a
 *    reload, every note holds the title she typed in steps 2 and 3.
 * 5. The move switches again and `--close-soak` ends it: `notes.title` is
 *    orphaned, and the notes still hold what she typed.
 *
 * A regression looks like a typed title missing after a reload, a refused
 * step, a move that does not stop at its budget, or a revert that loses
 * what was typed on the new version.
 *
 * It stores 0.3.6, so it runs after `rehearsal` and before `branch`
 * (0.5.x). Chromium only.
 */

import { expect, test } from '@playwright/test';
import { door, developer } from '../developer.js';
import { owner } from '../people.js';
import { addNote, applyProgram, ensureServing, servingVersion, signedIn, stack, unique } from '../support.js';
import { holds, openV2, renamedTitle, retitle, watchTenant } from './moves.js';

test.describe.configure({ mode: 'serial' });

test('a rename on one tenant under live writes: nothing typed is lost, and a revert in the soak keeps it', async ({
  browser,
}, testInfo) => {
  testInfo.setTimeout(420_000);
  // After `rehearsal`, production serves 0.3.4. Run alone, v3 is applied.
  if (!['0.3.0', '0.3.4'].includes(servingVersion() ?? '')) {
    ensureServing(stack().programV2, '0.2.0');
    applyProgram(stack().programV3);
  }
  const from = servingVersion() ?? '';
  const sealed = from === '0.3.4';
  const to = sealed ? '0.3.6' : '0.3.2';
  const ada = await owner('Ada');
  const dev = await developer(browser);
  try {
    const page = ada.page;
    const tenant = watchTenant(page);
    await openV2(page);
    const titles = [0, 1, 2, 3, 4, 5].map((i) => unique(`moved-${i}-`));
    for (const t of titles) await addNote(page, t, `Written before the move: ${t}.`);
    const space = tenant();

    await test.step(`${to} is stored beside ${from}, and its step with the rename`, () => {
      const stored = dev.ordo(['apply', renamedTitle(to, sealed), '--no-activate', ...door()]);
      expect(stored.code, `${stored.out}\n${stored.err}`).toBe(0);
      expect(stored.out).toContain(to);
      const plan = dev.ordo(['evolve', '--from', from, '--to', to, ...door()]);
      expect(plan.code, `${plan.out}\n${plan.err}`).toBe(0);
      expect(plan.out).toContain('B4');
      expect(plan.out).toMatch(/notes\.title.*notes\.heading/);
    });

    await test.step(`the move starts and stops at its budget, Ada still on ${from}`, () => {
      const ran = dev.ordo(['evolve', '--tenant', space, '--to', to, '--budget', '1', '--chunk-rows', '2', ...door()]);
      expect(ran.code, `${ran.out}\n${ran.err}`).toBe(0);
      expect(ran.out).toContain(`move       ${from} to ${to}: backfilling`);
      expect(ran.out).toContain('every write keeps both shapes');
    });

    const during = unique('retitled-during-');
    const added = unique('added-during-');
    await test.step('Ada retitles a note and adds one during the backfill', async () => {
      await retitle(page, titles[5]!, during);
      await addNote(page, added, 'Added while the backfill ran.');
    });

    await test.step('the move finishes: verify, the switch, a 7-day soak', () => {
      const ran = dev.ordo(['evolve', '--tenant', space, '--to', to, ...door()]);
      expect(ran.code, `${ran.out}\n${ran.err}`).toBe(0);
      expect(ran.out).toContain(`pinned     ${to} (was ${from})`);
      expect(ran.out).toContain(`move       ${from} to ${to}: soaking`);
      expect(ran.out).toMatch(/^soak +until /m);
    });

    const soaked = unique('retitled-in-soak-');
    await test.step(`on ${to}, Ada reads every note and retitles another`, async () => {
      await page.reload();
      await signedIn(page);
      for (const t of titles.slice(0, 5)) await holds(page, t);
      await holds(page, during);
      await holds(page, added);
      await retitle(page, titles[0]!, soaked);
    });

    await test.step('reverted inside the soak', () => {
      const back = dev.ordo(['evolve', '--tenant', space, '--revert', ...door()]);
      expect(back.code, `${back.out}\n${back.err}`).toBe(0);
      expect(back.out).toContain(`pinned     ${from} (was ${to})`);
      expect(back.out).toContain('gated');
    });

    await test.step(`after a reload on ${from}, every note holds what Ada typed`, async () => {
      await page.reload();
      await signedIn(page);
      await holds(page, added);
      await holds(page, during);
      await holds(page, soaked);
      await expect(page.getByRole('alert')).toHaveCount(0);
    });

    await test.step('switched again and the soak closed: title is orphaned, the text kept', async () => {
      const again = dev.ordo(['evolve', '--tenant', space, '--to', to, ...door()]);
      expect(again.code, `${again.out}\n${again.err}`).toBe(0);
      expect(again.out).toContain('soaking');
      const closed = dev.ordo(['evolve', '--tenant', space, '--close-soak', ...door()]);
      expect(closed.code, `${closed.out}\n${closed.err}`).toBe(0);
      expect(closed.out).toContain('closed     the soak is over; orphaned: notes.title');
      await page.reload();
      await signedIn(page);
      await holds(page, soaked);
      await holds(page, during);
    });
  } finally {
    await dev.close();
    await ada.close();
  }
});
