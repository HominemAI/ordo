/**
 * Notes v3 splits a field (plan-versions.md increment 15b; forward-gaps
 * asks 8 and 18). The new version splits `notes.title` into `heading` and
 * `subtitle`, each `prev(title)`: "Trip: Lisbon" becomes heading "Trip" and
 * subtitle "Lisbon". Its ops still answer `title`, joined again, so the v2
 * page reads the same notes. The developer, signed in with `ordo login`:
 *
 * 1. Stores the version and asks for its step with no mapping. It is
 *    refused, naming `--fill` with an example.
 * 2. Gives each new field's mapping (`--fill`) and the old field's reverse
 *    (`--reverse`). The step stores as B12, with a preview on 3 rows.
 * 3. `ordo evolve --tenant <Ivy>`: the copy, verify, the switch, a soak.
 *    On the new version Ivy reads every note and adds one.
 * 4. `--revert` in the soak: after a reload on the version before, the note
 *    she added is there, its title written back by the reverse.
 * 5. The move switches again and `--close-soak` ends it: `notes.title` is
 *    orphaned, and every note still reads.
 *
 * A regression looks like a refused step that names no option, a note
 * missing or misread after the split, or a revert that loses the note added
 * on the new version.
 *
 * It stores 0.3.8, above `rollout-moves`' 0.3.7 and below `branch`'s 0.5.x,
 * so it runs between them. Chromium only.
 */

import { cpSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, test } from '@playwright/test';
import { door, developer } from '../developer.js';
import { owner } from '../people.js';
import { addNote, applyProgram, ensureServing, servingVersion, signedIn, stack } from '../support.js';
import { holds, openV2, watchTenant } from './moves.js';

test.describe.configure({ mode: 'serial' });

const TO = '0.3.8';

/** Notes v3 at 0.3.8 with `title` split, made from v3's own file. */
function splitTitle(sealed: boolean): string {
  const v3 = readFileSync(join(stack().programV3, 'notes.ordo'), 'utf8');
  const swaps: [string, string][] = [
    ['domain notes @ 0.3.0', `domain notes @ ${TO}`],
    ['reader note_viewer reads notes, notesAccess', 'reader note_viewer reads notesAccess'],
    ['  title: text\n', '  heading: text = "" prev(title)\n  subtitle: text? prev(title)\n'],
    ['set notes { title: $title,', 'set notes { heading: $title, subtitle: null,'],
    [
      'set notes { id: $id, rev: $rev, title: $title',
      'set notes { id: $id, rev: $rev, heading: $title, subtitle: null',
    ],
    ['[id, rev, title,', '[id, rev, title: coalesce(heading + ": " + subtitle, heading),'],
  ];
  if (sealed) swaps.push(['  body:  text = ""\n', '  body:  text sealed\n']);
  let source = v3;
  for (const [from, to] of swaps) {
    expect(source, `notes v3 still holds \`${from}\``).toContain(from);
    source = source.split(from).join(to);
  }
  const dir = mkdtempSync(join(tmpdir(), `notes-${TO}-`));
  writeFileSync(join(dir, 'notes.ordo'), source);
  cpSync(join(stack().programV3, 'fn'), join(dir, 'fn'), { recursive: true });
  return dir;
}

const FILL = 'notes.heading=coalesce(before(title, ": "), title);notes.subtitle=after(title, ": ")';
const REVERSE = 'notes.title=coalesce(heading + ": " + subtitle, heading)';

test('notes v3 splits a field: the mapping is asked for, every note reads, and a revert in the soak keeps a new one', async ({
  browser,
}, testInfo) => {
  testInfo.setTimeout(420_000);
  // After `rollout-moves`, production serves 0.3.4. Run alone, v3 is applied.
  if (!['0.3.0', '0.3.4'].includes(servingVersion() ?? '')) {
    ensureServing(stack().programV2, '0.2.0');
    applyProgram(stack().programV3);
  }
  const from = servingVersion() ?? '';
  const ivy = await owner('Ivy');
  const dev = await developer(browser);
  try {
    const page = ivy.page;
    const tenant = watchTenant(page);
    await openV2(page);
    const titles = ['Trip: Lisbon', 'Groceries', 'Work: the plan: draft'];
    for (const t of titles) await addNote(page, t, `Written before the split: ${t}.`);
    const space = tenant();

    await test.step(`${TO} is stored, and its step is refused until the mapping is given`, () => {
      const stored = dev.ordo(['apply', splitTitle(from === '0.3.4'), '--no-activate', ...door()]);
      expect(stored.code, `${stored.out}\n${stored.err}`).toBe(0);
      const refused = dev.ordo(['evolve', '--from', from, '--to', TO, ...door()]);
      expect(refused.code, refused.out).not.toBe(0);
      expect(refused.err).toContain('map_fill_required');
      expect(refused.err).toContain('--fill notes.heading=');
    });

    await test.step('with each mapping and the reverse, the step stores as a split, with a preview', () => {
      const plan = dev.ordo(['evolve', '--from', from, '--to', TO, '--fill', FILL, '--reverse', REVERSE, ...door()]);
      expect(plan.code, `${plan.out}\n${plan.err}`).toBe(0);
      expect(plan.out).toContain('B12');
      expect(plan.out).toMatch(/notes\.title.*split into.*notes\.heading.*notes\.subtitle/);
      expect(plan.out).toContain('PREVIEW');
    });

    await test.step(`Ivy's tenant moves to ${TO}, in a soak`, () => {
      const ran = dev.ordo(['evolve', '--tenant', space, '--to', TO, ...door()]);
      expect(ran.code, `${ran.out}\n${ran.err}`).toBe(0);
      expect(ran.out).toContain(`pinned     ${TO} (was ${from})`);
      expect(ran.out).toContain(`move       ${from} to ${TO}: soaking`);
    });

    const added = 'Packing: socks';
    await test.step(`on ${TO}, every note reads whole, and Ivy adds one`, async () => {
      await page.reload();
      await signedIn(page);
      for (const t of titles) await holds(page, t);
      await addNote(page, added, 'Added on the split version.');
      await holds(page, added);
    });

    await test.step('reverted inside the soak, the new note is there', async () => {
      const back = dev.ordo(['evolve', '--tenant', space, '--revert', ...door()]);
      expect(back.code, `${back.out}\n${back.err}`).toBe(0);
      expect(back.out).toContain(`pinned     ${from} (was ${TO})`);
      await page.reload();
      await signedIn(page);
      await holds(page, added);
      for (const t of titles) await holds(page, t);
      await expect(page.getByRole('alert')).toHaveCount(0);
    });

    await test.step('switched again and the soak closed: title is orphaned, every note reads', async () => {
      const again = dev.ordo(['evolve', '--tenant', space, '--to', TO, ...door()]);
      expect(again.code, `${again.out}\n${again.err}`).toBe(0);
      const closed = dev.ordo(['evolve', '--tenant', space, '--close-soak', ...door()]);
      expect(closed.code, `${closed.out}\n${closed.err}`).toBe(0);
      expect(closed.out).toContain('orphaned: notes.title');
      await page.reload();
      await signedIn(page);
      for (const t of [...titles, added]) await holds(page, t);
    });
  } finally {
    await dev.close();
    await ivy.close();
  }
});
