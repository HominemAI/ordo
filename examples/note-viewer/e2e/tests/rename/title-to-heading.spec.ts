/**
 * notes renames a column note_viewer reads (plan-versions.md increment 7b;
 * plan-evolution-federation.md §7, test 1).
 *
 * notes moves from 0.3.0 to 0.3.1 (fixtures/notes-heading), which renames
 * `notes.title` to `heading` with `prev(title)`. note_viewer maps `title`.
 * The rename keeps the old column and keeps it written, so the viewer reads
 * through it: a note from before shows with its title, and so does a note
 * written in notes after the rename. The viewer's role still reads `title`,
 * and Postgres holds the same value under both names.
 *
 * A regression looks like the viewer held or empty after the rename, a
 * note shown with no title, or a note written after the rename missing at
 * the viewer.
 *
 * It changes notes' program for the whole stack, so it runs after `v3` and
 * before `orphans` (playwright.config.ts).
 */

import { copyFileSync, cpSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { addNote, applyNotes, joinNotes, list, person, shows, sqlAs, stack, unique } from '../support.js';

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

test('notes renames `title` to `heading`: the viewer reads every note through it, with its title', async ({
  browser,
}) => {
  test.setTimeout(300_000);
  const di = await person(browser);
  const notes = di.page;
  await joinNotes(notes, 'Di');
  const before = unique('Public before the rename');
  await addNote(notes, before, 'Written under 0.3.0.', 'public');

  const viewer = await di.context.newPage();
  await viewer.goto(stack().viewerPage);
  await shows(viewer, 'Public notes', before);

  const applied = applyNotes(withHeading());
  expect(applied).toContain('0.3.1');

  // Postgres: the viewer's role still reads `title`, and the new column
  // holds the same value.
  const q = `SELECT title || '|' || heading FROM notes.notes WHERE title = '${before}'`;
  await expect.poll(() => sqlAs('viewer_app', q), { timeout: 30_000 }).toBe(`${before}|${before}`);

  // The viewer reads through the rename: the note from before, by its title.
  await shows(viewer, 'Public notes', before);
  await expect(viewer.getByRole('alert')).toHaveCount(0);

  // A note written after the rename reaches the old column too, so the
  // viewer shows it with its title.
  const after = unique('Public after the rename');
  await addNote(notes, after, 'Written under 0.3.1.', 'public');
  await shows(viewer, 'Public notes', after);
  await expect(list(viewer, 'Public notes').getByRole('heading', { name: after })).toBeVisible();
  await di.context.close();
});

// plan-evolution-federation.md §7, test 3: when the last soak on `notes`
// closes, the viewer's grant on `title` is revoked (owner ruling
// 2026-11-23, Q1), and its page shows `external_changed` naming `heading`.
// Soaks and their close are increment 11a; with no soak, a rename's old
// column is never orphaned, so there is nothing to close yet.
test.fixme('the soak closes: the viewer is refused naming `heading` and the fix', () => {});
