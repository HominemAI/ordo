/**
 * A reader reads only columns the read app still writes (plan-versions.md
 * increment 14; owner ruling 2026-11-23, Q1).
 *
 * notes moves to a version that stops declaring `notes.tags`
 * (fixtures/notes-no-tags, 0.3.2). The column is an orphan: kept, never
 * written. notes takes it out of note_viewer's grant: Postgres refuses the
 * viewer's role `tags` and still serves `title`. note_viewer maps `tags`,
 * so its check holds the notes it reads, and its page shows the named
 * error: `external_changed`, the column, the app that stopped writing it,
 * and the fix.
 *
 * A regression looks like the viewer still listing notes with frozen tags,
 * a grant that still covers `tags`, or an error that does not name the
 * column.
 *
 * It changes notes' program for the whole stack, so it runs after `v3`, last.
 */

import { copyFileSync, cpSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { applyNotes, sqlAs, stack } from '../support.js';

/** The 0.3.2 program: the fixture as `notes.ordo`, beside notes v3's `fn`. */
function withoutTags(): string {
  const dir = mkdtempSync(join(tmpdir(), 'notes-no-tags-'));
  copyFileSync(
    fileURLToPath(new URL('../../fixtures/notes-no-tags.ordo.fixture', import.meta.url)),
    join(dir, 'notes.ordo'),
  );
  cpSync(join(stack().programV3, 'fn'), join(dir, 'fn'), { recursive: true });
  return dir;
}

test('notes stops writing `tags`: the viewer loses the column and its page names it', async ({ page }) => {
  test.setTimeout(300_000);
  // Before: the viewer's role reads `tags`, and the public list shows.
  sqlAs('viewer_app', 'SELECT tags FROM notes.notes LIMIT 1');
  await page.goto(stack().viewerPage);
  await expect(page.getByRole('heading', { name: 'Public notes' })).toBeVisible();

  const applied = applyNotes(withoutTags());
  expect(applied).toContain('0.3.2');
  expect(applied).toContain('orphaned    `notes.tags`');

  // Postgres is the control: `tags` is refused, the rest still reads.
  expect(() => sqlAs('viewer_app', 'SELECT tags FROM notes.notes LIMIT 1')).toThrow(/permission denied/);
  sqlAs('viewer_app', 'SELECT id, title FROM notes.notes LIMIT 1');

  // The page names the column, the app that stopped writing it, and the fix.
  await expect(async () => {
    await page.reload();
    const alert = page.getByRole('alert');
    await expect(alert).toBeVisible({ timeout: 3_000 });
    await expect(alert).toContainText('external_changed');
    await expect(alert).toContainText('column `tags` is no longer written by `notes`');
    await expect(alert).toContainText('ordo introspect --external');
  }).toPass({ timeout: 90_000 });
});
