/**
 * An op outside the grant, refused on the page by name
 * (plan-federation-grants.md §3 and §5, steps G3 and G8; replaces the
 * shared-database `reader` line test, plan-evolution-federation.md §7,
 * test 5).
 *
 * notes moves to 0.3.5, whose grant to note_viewer opens `oneNote` and no
 * longer `myNotes`. Its apply goes on and names the consumer
 * it breaks. From that apply on, notes' door refuses the viewer's call to
 * `myNotes`, naming the consumer, the op and the line that would
 * allow it, and the viewer's page shows that refusal in the list's place.
 *
 * A regression looks like the viewer still reading the list, a refusal the
 * page does not show, or one that does not say what to add.
 *
 * It narrows the grant for the whole stack, so it runs last.
 */

import { cpSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { addNote, applyNotes, joinNotes, person, refusal, shows, signInToViewer, stack, unique } from '../support.js';

/** 0.3.5: the sealed 0.3.4 with `myNotes` off the grant. */
function narrowed(): string {
  const dir = mkdtempSync(join(tmpdir(), 'notes-narrowed-'));
  let source = readFileSync(
    fileURLToPath(new URL('../../fixtures/notes-no-tags.ordo.fixture', import.meta.url)),
    'utf8',
  );
  for (const [from, to] of [
    ['domain notes @ 0.3.3', 'domain notes @ 0.3.5'],
    ['  body:  text = ""\n', '  body:  text sealed\n'],
    ['op createNote($title: text, $body: text, $created: datetime) =', 'op createNote($title: text, $body: text, $tags: json, $created: datetime) ='],
    ['grant note_viewer as itself {\n  myNotes\n}\n', 'grant note_viewer as itself {\n  oneNote\n}\n'],
  ]) {
    expect(source, `the fixture still holds ${from}`).toContain(from);
    source = source.replace(from, to);
  }
  writeFileSync(join(dir, 'notes.ordo'), source);
  cpSync(join(stack().programV3, 'fn'), join(dir, 'fn'), { recursive: true });
  return dir;
}

test('notes takes `myNotes` off its grant: the viewer’s page shows the refusal, naming the op and the fix', async ({
  browser,
}) => {
  test.setTimeout(300_000);
  const gil = await person(browser);
  try {
    await joinNotes(gil.page, 'Gil');
    const mine = unique('Gil before the narrowing');
    await addNote(gil.page, mine, 'Only Gil reads this.');
    const viewer = await gil.context.newPage();
    await signInToViewer(viewer);
    await shows(viewer, 'Your notes', mine);

    const applied = applyNotes(narrowed());
    expect(applied).toContain('0.3.5');
    // A warning, never a refusal: the holder names the consumer it breaks.
    expect(applied).toContain('breaks note_viewer: note_viewer calls myNotes');

    await expect(async () => {
      await viewer.reload();
      const said = refusal(viewer, 'Your notes');
      await expect(said).toBeVisible({ timeout: 3_000 });
      await expect(said).toContainText('peer_refused');
      await expect(said).toContainText('myNotes');
      await expect(said).toContainText('grant');
    }).toPass({ timeout: 90_000 });
    await expect(viewer.getByText(mine)).toHaveCount(0);
  } finally {
    await gil.context.close();
  }
});
