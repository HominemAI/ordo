/**
 * notes renames a field note_viewer reads, through the door
 * (plan-versions.md increment 7b; plan-evolution-federation.md F1;
 * plan-federation-grants.md §4, step G8).
 *
 * notes moves from 0.3.0 to 0.3.1 (fixtures/notes-heading), which renames
 * `notes.title` to `heading` with `prev(title)`. Before it runs, the
 * developer stores 0.3.1 and renders the step with `ordo evolve` through
 * notes' door: it names `grant note_viewer`, which opens `notes`, and the
 * field.
 *
 * Through the door the two lists behave as their ops do:
 *
 * - The public list reads notes' op `publicNotes`, which answers
 *   `title: heading`. An op is a contract, so the viewer reads through the
 *   rename.
 * - The person's list reads `std.notes.list`, which answers the entity's
 *   own fields. The viewer still asks it for `title`, and notes refuses by
 *   name; the page shows the refusal in that list's place.
 *
 * The viewer maps `heading` (0.2.1) and every note shows again.
 *
 * A regression looks like the public list lost across the rename, a
 * person's list that shows notes with no title instead of a named refusal,
 * or a plan that does not name the grant.
 *
 * It changes notes' program for the whole stack, so it runs after `v3` and
 * before `orphans` (playwright.config.ts).
 */

import { copyFileSync, cpSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
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
  ordoAs,
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

/** note_viewer at 0.2.1: the person's list reads `title` from `heading`. */
function viewerOnHeading(): string {
  const dir = mkdtempSync(join(tmpdir(), 'viewer-heading-'));
  const source = readFileSync(fileURLToPath(new URL('../../../viewer.ordo', import.meta.url)), 'utf8');
  const entries = 'external entries from notes.notes shared {\n  id: text <- id\n  title: text <- title\n';
  expect(source).toContain(entries);
  const mapped = source
    .replace('domain noteviewer @ 0.2.0', 'domain noteviewer @ 0.2.1')
    .replace(entries, entries.replace('title: text <- title', 'title: text <- heading'));
  writeFileSync(join(dir, 'viewer.ordo'), mapped);
  return dir;
}

test('notes renames `title`: the public op reads through it, and the person’s list is refused by name until the viewer maps it', async ({
  browser,
}) => {
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

    // F1: the step is rendered through notes' door before it runs, and
    // names the grant that opens the renamed field's table.
    const program = withHeading();
    const stored = dev.ordo('notes', ['apply', program, '--no-activate', ...door('notes')]);
    expect(stored.code, `${stored.out}\n${stored.err}`).toBe(0);
    expect(stored.out).toContain('activated  no');
    const plan = dev.ordo('notes', ['evolve', '--from', '0.3.0', '--to', '0.3.1', ...door('notes')]);
    expect(plan.code, `${plan.out}\n${plan.err}`).toBe(0);
    expect(plan.out).toContain('`grant note_viewer` opens `notes` to another deployment, field `title`');

    const applied = applyNotes(program);
    expect(applied).toContain('0.3.1');

    // The public op answers `title: heading`: the viewer reads through it.
    await shows(viewer, 'Public notes', open);
    await expect(refusal(viewer, 'Public notes')).toHaveCount(0);
    // `std.notes.list` has no `title` now: refused by name, in its place.
    await expect(async () => {
      await viewer.reload();
      const said = refusal(viewer, 'Your notes');
      await expect(said).toBeVisible({ timeout: 3_000 });
      await expect(said).toContainText('peer_refused');
      await expect(said).toContainText('title');
    }).toPass({ timeout: 60_000 });

    // The viewer maps `heading`: every note shows again, with its title.
    const r = ordoAs('viewer', ['apply', viewerOnHeading(), '--direct', '--as', 'fed-e2e', '--wait-secs', '240']);
    expect(r.code, `${r.out}\n${r.err}`).toBe(0);
    expect(r.out).toContain('0.2.1');
    await shows(viewer, 'Your notes', mine);
    await expect(list(viewer, 'Your notes').getByRole('heading', { name: open })).toBeVisible();
    await expect(refusal(viewer, 'Your notes')).toHaveCount(0);
  } finally {
    await dev.close();
    await di.context.close();
  }
});
