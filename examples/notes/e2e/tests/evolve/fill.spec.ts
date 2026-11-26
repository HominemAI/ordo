/**
 * A `--fill` backfill on one tenant under live writes, through the door,
 * with the page open (plan-versions.md increment 10b; its §4 table).
 *
 * The new version adds `notes.excerpt`, a required field with no default.
 * Its ops set it with the title and answer `title` from it, so once Fay's
 * tenant serves it, the v2 page shows a note's title only if its row was
 * filled. After `rollout-moves`, production serves 0.3.4 (notes.body
 * sealed); run alone (`--no-deps`), v3 is applied and it serves 0.3.0.
 * Either way the new version is 0.3.8. The developer, signed in with
 * `ordo login` (tests/developer.ts):
 *
 * 1. Stores 0.3.8 and its step with `--fill notes.excerpt=title`.
 * 2. `ordo evolve --tenant <Fay> --budget 1 --chunk-rows 2`: the expand and
 *    one chunk of the fill. The move stops in `backfilling`, Fay still on
 *    the version before.
 * 3. Fay retitles a note and adds one, on the page, while the fill runs.
 * 4. `ordo evolve --tenant <Fay>`: the rest of the fill, verify, the
 *    switch. After a reload on 0.3.8 every note shows its title, read from
 *    `excerpt`: the ones written before, and the two she typed during the
 *    fill. She adds another.
 * 5. `--close-soak`: the `NOT NULL` lands, which it cannot over an empty
 *    row, and the notes still show.
 *
 * A regression looks like a note listed with no title after the switch (a
 * row the fill or the dual write missed), a retitle during the fill shown
 * with its old title, or a close refused over an empty row.
 *
 * It stores 0.3.8, above `rollout-moves`' 0.3.7 and below `branch`'s 0.5.x,
 * so it runs between them. Chromium only.
 */

import { expect, test } from '@playwright/test';
import { door, developer } from '../developer.js';
import { owner } from '../people.js';
import { addNote, applyProgram, ensureServing, servingVersion, signedIn, stack, unique } from '../support.js';
import { filledExcerpt, holds, openV2, retitle, watchTenant } from './moves.js';

test.describe.configure({ mode: 'serial' });

test('a fill on one tenant under live writes: every row ends filled, what was typed included', async ({
  browser,
}, testInfo) => {
  testInfo.setTimeout(420_000);
  // After `rollout-moves`, production serves 0.3.4. Run alone, v3 is applied.
  if (!['0.3.0', '0.3.4'].includes(servingVersion() ?? '')) {
    ensureServing(stack().programV2, '0.2.0');
    applyProgram(stack().programV3);
  }
  const from = servingVersion() ?? '';
  const sealed = from === '0.3.4';
  const to = '0.3.8';
  const fay = await owner('Fay');
  const dev = await developer(browser);
  const ordo = (args: string[]) => {
    const r = dev.ordo([...args, ...door()]);
    expect(r.code, `ordo ${args.join(' ')}\n${r.out}\n${r.err}`).toBe(0);
    return r.out;
  };
  try {
    const page = fay.page;
    const tenant = watchTenant(page);
    await openV2(page);
    const titles = [0, 1, 2, 3, 4, 5].map((i) => unique(`filled-${i}-`));
    for (const t of titles) await addNote(page, t, `Written before the fill: ${t}.`);
    const space = tenant();

    await test.step(`${to} is stored beside ${from}, and its step with the fill`, () => {
      const stored = ordo(['apply', filledExcerpt(to, sealed), '--no-activate']);
      expect(stored).toContain(to);
      const plan = ordo(['evolve', '--from', from, '--to', to, '--fill', 'notes.excerpt=title']);
      expect(plan).toContain('backfill: notes.excerpt = title');
    });

    await test.step(`the fill starts and stops at its budget, Fay still on ${from}`, () => {
      const ran = ordo(['evolve', '--tenant', space, '--to', to, '--budget', '1', '--chunk-rows', '2']);
      expect(ran).toContain(`move       ${from} to ${to}: backfilling`);
      expect(ran).toContain('every write keeps both shapes');
    });

    const during = unique('retitled-during-fill-');
    const added = unique('added-during-fill-');
    await test.step('Fay retitles a note and adds one while the fill runs', async () => {
      await retitle(page, titles[5]!, during);
      await addNote(page, added, 'Added while the fill ran.');
    });

    await test.step('the fill finishes: verify, the switch, a 7-day soak', () => {
      const ran = ordo(['evolve', '--tenant', space, '--to', to]);
      expect(ran).toContain(`pinned     ${to} (was ${from})`);
      expect(ran).toContain(`move       ${from} to ${to}: soaking`);
    });

    const after = unique('added-on-new-');
    await test.step(`on ${to}, every title is read from the filled field`, async () => {
      await page.reload();
      await signedIn(page);
      for (const t of titles.slice(0, 5)) await holds(page, t);
      await holds(page, during);
      await holds(page, added);
      await addNote(page, after, 'Added on the new version.');
      await expect(page.getByRole('alert')).toHaveCount(0);
    });

    await test.step('the soak closes: the field is required over every row', async () => {
      const closed = ordo(['evolve', '--tenant', space, '--close-soak']);
      expect(closed).toContain('closed     the soak is over');
      await page.reload();
      await signedIn(page);
      for (const t of [...titles.slice(0, 5), during, added, after]) await holds(page, t);
    });
  } finally {
    await dev.close();
    await fay.close();
  }
});
