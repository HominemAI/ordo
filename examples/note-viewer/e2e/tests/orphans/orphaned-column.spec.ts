/**
 * notes stops declaring a field note_viewer reads, through the door
 * (plan-versions.md increment 14; owner ruling 2026-11-23, Q1;
 * plan-federation-grants.md §4, step G8).
 *
 * notes moves to 0.3.3 (fixtures/notes-no-tags), which stops declaring
 * `notes.tags`. Its column is an orphan: kept, never written, and never
 * answered by notes' door. `myNotes` no longer answers `tags`, and without
 * tags notes has no public list, so its grant opens `myNotes` only.
 *
 * - The person's list still asks `myNotes` for `tags`: notes refuses by
 *   name, and the page shows the refusal in its place.
 * - The public list calls `publicNotes`, which notes no longer has: the
 *   refusal names the op.
 *
 * The viewer stops reading `tags` (0.2.2), and the person's list shows
 * again. A regression looks like the viewer listing notes with frozen
 * tags, or a refusal that does not name the field or the op.
 *
 * It changes notes' program for the whole stack, so it runs after `rename`.
 */

import { copyFileSync, cpSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { addNote, applyNotes, joinNotes, ordoAs, person, refusal, shows, signInToViewer, stack, unique } from '../support.js';

/** The 0.3.3 program: the fixture as `notes.ordo`, beside notes v3's `fn`. */
function withoutTags(): string {
  const dir = mkdtempSync(join(tmpdir(), 'notes-no-tags-'));
  copyFileSync(
    fileURLToPath(new URL('../../fixtures/notes-no-tags.ordo.fixture', import.meta.url)),
    join(dir, 'notes.ordo'),
  );
  cpSync(join(stack().programV3, 'fn'), join(dir, 'fn'), { recursive: true });
  return dir;
}

/** note_viewer at 0.2.2: `tags` no longer read. */
function viewerWithoutTags(): string {
  const dir = mkdtempSync(join(tmpdir(), 'viewer-no-tags-'));
  let source = readFileSync(fileURLToPath(new URL('../../../viewer.ordo', import.meta.url)), 'utf8');
  const swaps: [string, string][] = [
    ['domain noteviewer @ 0.2.0', 'domain noteviewer @ 0.2.2'],
    ['  tags: json <- tags\n', ''],
    ['entries[id, title, body, tags, created]', 'entries[id, title, body, created]'],
  ];
  for (const [from, to] of swaps) {
    expect(source, `viewer.ordo still holds ${from}`).toContain(from);
    source = source.replace(from, to);
  }
  writeFileSync(join(dir, 'viewer.ordo'), source);
  return dir;
}

test('notes stops declaring `tags`: each list the viewer reads is refused by name, until it stops reading it', async ({
  browser,
}) => {
  test.setTimeout(300_000);
  const ed = await person(browser);
  try {
    await joinNotes(ed.page, 'Ed');
    const mine = unique('Ed before no tags');
    await addNote(ed.page, mine, 'Only Ed reads this.');
    const viewer = await ed.context.newPage();
    await signInToViewer(viewer);
    await shows(viewer, 'Your notes', mine);

    const applied = applyNotes(withoutTags());
    expect(applied).toContain('0.3.3');
    expect(applied).toContain('orphaned    `notes.tags`');
    // The holder's plan names what the change breaks for the viewer.
    expect(applied).toContain('breaks note_viewer: note_viewer calls publicNotes');

    await expect(async () => {
      await viewer.reload();
      const mineSaid = refusal(viewer, 'Your notes');
      await expect(mineSaid).toBeVisible({ timeout: 3_000 });
      await expect(mineSaid).toContainText('peer_refused');
      await expect(mineSaid).toContainText('tags');
      const openSaid = refusal(viewer, 'Public notes');
      await expect(openSaid).toBeVisible({ timeout: 3_000 });
      await expect(openSaid).toContainText('peer_refused');
      await expect(openSaid).toContainText('publicNotes');
    }).toPass({ timeout: 90_000 });

    const r = ordoAs('viewer', ['apply', viewerWithoutTags(), '--direct', '--as', 'fed-e2e', '--wait-secs', '240']);
    expect(r.code, `${r.out}\n${r.err}`).toBe(0);
    await shows(viewer, 'Your notes', mine);
    await expect(refusal(viewer, 'Your notes')).toHaveCount(0);
  } finally {
    await ed.context.close();
  }
});
