/**
 * A sealed field shown through the door (plan-federation-grants.md §6,
 * steps G7 and G8; plan-versions.md increment 13).
 *
 * notes moves from 0.3.3 to 0.3.4, which seals `notes.body` and says
 * `std sealed` on `notes`, so its `std.notes.list` carries the field. The
 * grant to note_viewer does not stop the seal: notes opens the field in its
 * own answers, under its own rules for the person, and no key leaves it.
 * The developer runs the step through notes' door and activates 0.3.4.
 *
 * The viewer's page then shows the person's note with its body, and notes'
 * table holds no plaintext.
 *
 * A regression looks like a seal refused because another deployment reads
 * the field, a page that shows ciphertext, nothing or a refusal, or a body
 * readable in the table.
 *
 * It seals a field for the whole stack, so it runs after `orphans`.
 */

import { cpSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { developer, door } from '../developer.js';
import { addNote, joinNotes, list, person, refusal, shows, signInToViewer, sqlAs, stack, unique } from '../support.js';

/** 0.3.4: the 0.3.3 fixture with `notes.body` sealed and `std sealed`. */
function sealed(): string {
  const dir = mkdtempSync(join(tmpdir(), 'notes-sealed-'));
  let source = readFileSync(
    fileURLToPath(new URL('../../fixtures/notes-no-tags.ordo.fixture', import.meta.url)),
    'utf8',
  );
  for (const [from, to] of [
    ['domain notes @ 0.3.3', 'domain notes @ 0.3.4'],
    ['  body:  text = ""\n', '  body:  text sealed\n  std sealed\n'],
  ]) {
    expect(source, `the fixture still holds ${from}`).toContain(from);
    source = source.replace(from, to);
  }
  writeFileSync(join(dir, 'notes.ordo'), source);
  cpSync(join(stack().programV3, 'fn'), join(dir, 'fn'), { recursive: true });
  return dir;
}

test('notes seals a field the viewer reads: the viewer shows it, opened by notes, and the table holds no plaintext', async ({
  browser,
}) => {
  test.setTimeout(300_000);
  const fay = await person(browser);
  const dev = await developer(browser);
  try {
    await joinNotes(fay.page, 'Fay');
    const title = unique('Fay sealed');
    const body = unique('kept-under-key-');
    await addNote(fay.page, title, body);

    const dir = sealed();
    const stored = dev.ordo('notes', ['apply', dir, '--no-activate', ...door('notes')]);
    expect(stored.code, `${stored.out}\n${stored.err}`).toBe(0);
    const plan = dev.ordo('notes', ['evolve', '--from', '0.3.3', '--to', '0.3.4', ...door('notes')]);
    expect(plan.code, `${plan.out}\n${plan.err}`).toBe(0);
    expect(plan.out).toContain('backfill: notes.body = seal');
    // Not refused: the grant is named, as a warning.
    expect(plan.out).toContain('`grant note_viewer` opens `notes` to another deployment, field `body`');
    const ran = dev.ordo('notes', ['evolve', '--from', '0.3.3', '--to', '0.3.4', '--run', ...door('notes')]);
    expect(ran.code, `${ran.out}\n${ran.err}`).toBe(0);
    const active = dev.ordo('notes', ['apply', dir, ...door('notes')]);
    expect(active.code, `${active.out}\n${active.err}`).toBe(0);
    expect(active.out).toContain('activated  0.3.4');

    // The table holds no plaintext.
    expect(
      sqlAs('notes_app', `SELECT data_type FROM information_schema.columns WHERE table_schema = 'notes' AND table_name = 'notes' AND column_name = 'body'`),
    ).toBe('bytea');
    expect(
      sqlAs('notes_app', `SELECT count(*) FROM notes.notes WHERE position(convert_to('${body}', 'UTF8') in body) > 0`),
    ).toBe('0');

    // The viewer shows the note with its body, opened by notes' door.
    const viewer = await fay.context.newPage();
    await signInToViewer(viewer);
    await shows(viewer, 'Your notes', title);
    await expect(list(viewer, 'Your notes').getByText(body)).toBeVisible();
    await expect(refusal(viewer, 'Your notes')).toHaveCount(0);
  } finally {
    await dev.close();
    await fay.context.close();
  }
});
