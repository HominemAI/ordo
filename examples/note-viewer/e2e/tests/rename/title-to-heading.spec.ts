/**
 * notes renames a field note_viewer reads, through the door
 * (plan-versions.md increment 7b; plan-federation-grants.md §4, step G8).
 *
 * notes moves from 0.3.0 to 0.3.1 (fixtures/notes-heading), which renames
 * `notes.title` to `heading` with `prev(title)`. Before it runs, the
 * developer stores 0.3.1 and renders the step with `ordo evolve` through
 * notes' door.
 *
 * note_viewer reads 2 of notes' ops, `myNotes` and `publicNotes`, never its
 * tables. Both keep answering `title` (`title: heading`): an op is the
 * contract, so the viewer reads both lists through the rename with no
 * change of its own, a note from before and one written after.
 *
 * A regression looks like either list lost or refused across the rename,
 * or a note shown with no title.
 *
 * It changes notes' program for the whole stack, so it runs after `v3` and
 * before `orphans` (playwright.config.ts).
 */

import { copyFileSync, cpSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { developer, door } from '../developer.js';
import {
  addNote,
  applyNotes,
  joinNotes,
  list,
  person,
  refusal,
  shows,
  signInToViewer,
  stack,
  unique,
} from '../support.js';

/** The 0.3.1 program: the fixture as `notes.ordo`, beside notes v3's `fn`. */
function withHeading(): string {
  const dir = mkdtempSync(join(tmpdir(), 'notes-heading-'));
  copyFileSync(
    fileURLToPath(new URL('../../fixtures/notes-heading.ordo.fixture', import.meta.url)),
    join(dir, 'notes.ordo'),
  );
  cpSync(join(stack().programV3, 'fn'), join(dir, 'fn'), { recursive: true });
  return dir;
}

test('notes renames `title`: the viewer reads both lists through it, with each note’s title', async ({ browser }) => {
  test.setTimeout(300_000);
  const di = await person(browser);
  const dev = await developer(browser);
  try {
    const notes = di.page;
    await joinNotes(notes, 'Di');
    const open = unique('Public before the rename');
    const mine = unique('Private before the rename');
    await addNote(notes, open, 'Written under 0.3.0.', 'public');
    await addNote(notes, mine, 'Only Di reads this.');

    const viewer = await di.context.newPage();
    await signInToViewer(viewer);
    await shows(viewer, 'Your notes', mine);
    await shows(viewer, 'Public notes', open);

    // The step is rendered through notes' door before it runs.
    const program = withHeading();
    const stored = dev.ordo('notes', ['apply', program, '--no-activate', ...door('notes')]);
    expect(stored.code, `${stored.out}\n${stored.err}`).toBe(0);
    expect(stored.out).toContain('activated  no');
    const plan = dev.ordo('notes', ['evolve', '--from', '0.3.0', '--to', '0.3.1', ...door('notes')]);
    expect(plan.code, `${plan.out}\n${plan.err}`).toBe(0);

    const applied = applyNotes(program);
    expect(applied).toContain('0.3.1');
    // Nothing the viewer calls changed: no break is named.
    expect(applied).not.toContain('breaks note_viewer');

    // Both lists read through the rename, with each note's title.
    await shows(viewer, 'Public notes', open);
    await shows(viewer, 'Your notes', mine);
    const after = unique('Private after the rename');
    await addNote(notes, after, 'Written under 0.3.1.');
    await shows(viewer, 'Your notes', after);
    await expect(list(viewer, 'Your notes').getByRole('heading', { name: open })).toBeVisible();
    await expect(refusal(viewer, 'Your notes')).toHaveCount(0);
    await expect(refusal(viewer, 'Public notes')).toHaveCount(0);
  } finally {
    await dev.close();
    await di.context.close();
  }
});
